# AI-Native CLI: Full Design

> Status: **Design Complete — Awaiting Review**
> Created: 2026-08-09
> Scope: AI integration into oxo-flow CLI and extraction of shared AI crate

---

## 1. Design Principles

| Principle | What It Means |
|-----------|---------------|
| **AI is optional** | Zero-AI execution path unchanged, zero overhead when `[ai]` is absent |
| **One flag to activate** | `--ai` on any suitable command; `--ai-recover` for run/resume |
| **One TOML section to configure** | `[ai]` block in `.oxoflow` files or global config; nothing else required |
| **Agent-first** | All AI interactions run through Agent → Orchestrator → Tool; no raw prompt→response shortcuts |
| **Everything archived** | Every AI modification captured as before/after snapshot + session JSON log |
| **Provider-agnostic** | DeepSeek, Claude, OpenAI, Ollama — swap via config, code identical |

---

## 2. Five-Layer Architecture

```
┌─────────────────────────────────────────────────────────────┐
│              CLI / Web / IDE Plugin                          │
├─────────────────────────────────────────────────────────────┤
│  Layer 4: Command Integration                                │
│  template --ai · dry-run --ai · run --ai-recover · validate --ai │
├─────────────────────────────────────────────────────────────┤
│  Layer 3: Agent Framework                                    │
│  Agent trait · Orchestrator · Tool trait · Session/Archive  │
├─────────────────────────────────────────────────────────────┤
│  Layer 2: Knowledge & Context                                │
│  Builtin tools table · Error patterns · Prompt templates    │
│  External sources (URL/file) · Context assembler             │
├─────────────────────────────────────────────────────────────┤
│  Layer 1: AI Infrastructure (oxo-flow-ai crate)             │
│  Provider enum · Types · Config · Error · Session           │
├─────────────────────────────────────────────────────────────┤
│  Layer 0: Core Engine (oxo-flow-core) — unchanged           │
│  DAG · Executor · Config · Plugin · Environment             │
└─────────────────────────────────────────────────────────────┘
```

### Inter-layer dependency rules

- **L1 → L0**: disallowed. L1 knows nothing about DAG/Rule/Workflow. Pure AI abstraction.
- **L2 → L0**: read-only. References core types for validation context but never mutates.
- **L3 → L0**: via Tool trait only. Agent calls `ValidateWorkflow` tool, never imports core directly.
- **L4 → L0+L3**: command handlers wire Agent to deterministic CLI flows.
- **CLI/web crate → L1**: via `AI::global().is_enabled()` gate. False → everything behaves as today.

---

## 3. Layer 1: `oxo-flow-ai` Crate (Infrastructure)

### 3.1 Crate structure

```
crates/oxo-flow-ai/
├── Cargo.toml
└── src/
    ├── lib.rs              # Public API: AI::global(), AiConfig, AgentContext
    ├── provider.rs         # AiProvider enum + backends (extracted from web crate)
    ├── types.rs            # Message, ToolDef, ToolCall, AiResponse, Usage
    ├── config.rs           # AiConfig, AutoFixMode
    ├── error.rs            # AiError
    └── session.rs          # AiSession, Modification, session JSON persistence
```

### 3.2 Dependencies

```toml
[dependencies]
reqwest = { workspace = true, features = ["json", "rustls-tls"] }
serde = { workspace = true }
serde_json = { workspace = true }
tokio = { workspace = true, features = ["full"] }
anyhow = { workspace = true }
chrono = { workspace = true }
uuid = { workspace = true, features = ["v4"] }
tracing = { workspace = true }
```

**No dependency on oxo-flow-core.**

### 3.3 Provider (extracted from `ai_provider.rs`, enhanced)

```rust
pub enum AiProvider {
    Claude(ClaudeBackend),
    OpenAi(OpenAiBackend),
    DeepSeek(OpenAiBackend),  // Reuses OpenAI-compatible backend
    Ollama(OllamaBackend),
    Noop,
}

impl AiProvider {
    /// Simple chat — legacy path for web compat, wraps chat_with_tools
    pub async fn chat(&self, system: &str, user: &str) -> Result<String>;

    /// Multi-turn chat with tool calling — primary Agent path
    pub async fn chat_with_tools(
        &self,
        messages: &[Message],
        tools: &[ToolDef],
    ) -> Result<AiResponse>;

    pub fn name(&self) -> &str;
    pub fn model(&self) -> Option<String>;
}
```

**Enhancements over current `ai_provider.rs`:**
- `chat_with_tools()` translates ToolDef → DeepSeek/OpenAI native `tools` JSON, handles `tool_calls` response, and supports follow-up `tool` role messages for multi-turn.
- DeepSeek `strict` mode (Beta) for structured output when tool schemas allow.
- Unified error handling: network → retry, auth → clear message, schema → validation error.
- Richer `Usage` tracking: prompt_tokens, completion_tokens per call.

### 3.4 Core types

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Message {
    pub role: MessageRole,
    pub content: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_calls: Option<Vec<ToolCall>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_call_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,  // tool function name (for tool role messages)
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum MessageRole {
    #[serde(rename = "system")]   System,
    #[serde(rename = "user")]     User,
    #[serde(rename = "assistant")] Assistant,
    #[serde(rename = "tool")]     Tool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolDef {
    pub name: String,
    pub description: String,
    pub parameters: serde_json::Value,  // JSON Schema
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolCall {
    pub id: String,
    pub name: String,
    pub arguments: String,  // JSON string
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AiResponse {
    pub content: Option<String>,
    pub tool_calls: Option<Vec<ToolCall>>,
    pub usage: Usage,
    pub finish_reason: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Usage {
    pub prompt_tokens: u64,
    pub completion_tokens: u64,
}
```

### 3.5 Configuration

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AiConfig {
    pub enabled: bool,
    pub provider: ProviderKind,
    pub model: Option<String>,
    pub api_key: Option<String>,        // Never serialized to session logs
    pub api_url: Option<String>,
    pub max_retries: u32,
    pub auto_fix: AutoFixMode,
    pub temperature: Option<f64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum AutoFixMode {
    /// Propose changes, wait for user confirmation
    Ask,
    /// Automatically apply safe changes and continue
    Always,
    /// Only report issues, never modify
    Never,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum ProviderKind {
    Claude,
    OpenAi,
    DeepSeek,
    Ollama,
}
```

**Config resolution order** (later overrides earlier):

1. Hardcoded defaults: `ProviderKind::DeepSeek`, `AutoFixMode::Ask`, `max_retries: 3`
2. Global config: `~/.oxo-flow/ai_config.json`
3. Environment variables: `OXO_FLOW_AI_*`, `DEEPSEEK_API_KEY`, etc.
4. Workflow `[ai]` section in `.oxoflow` file
5. CLI flags: `--ai`, `--ai-recover`, `--ai-max-retries N`

```toml
# .oxoflow [ai] section
[ai]
enabled = true
max_retries = 5
auto_fix = "always"
model = "deepseek-v4-flash"      # Override model for this workflow
```

### 3.6 Session & Archive

```rust
pub struct AiSession {
    pub id: String,
    pub started_at: DateTime<Utc>,
    pub ended_at: Option<DateTime<Utc>>,
    pub command: String,
    pub workflow: Option<PathBuf>,
    pub user_intent: String,
    pub messages: Vec<SessionMessage>,
    pub tool_calls: Vec<ToolCallRecord>,
    pub modifications: Vec<Modification>,
    pub provider: String,
    pub model: String,
    pub outcome: SessionOutcome,
}

pub struct Modification {
    pub timestamp: DateTime<Utc>,
    pub file: PathBuf,
    pub before: String,
    pub after: String,
    pub reason: String,
    pub round: u32,
    pub applied: bool,
}
```

**Persistence layout:**

```
.oxo-flow/
├── ai_sessions/
│   └── {timestamp}-{command}-{uuid}.json    # Full session records
├── ai_archive/
│   └── {workflow_name}/
│       ├── {timestamp}-before.oxoflow       # Pre-modification snapshot
│       └── {timestamp}-after.oxoflow        # Post-modification snapshot
└── ai_config.json                            # Global config (already exists)
```

### 3.7 Global registry

```rust
pub struct AiRegistry {
    provider: RwLock<Option<AiProvider>>,
    config: RwLock<AiConfig>,
    session_store: RwLock<Vec<AiSession>>,
}

pub static AI: AiRegistry = AiRegistry::new();

impl AiRegistry {
    /// Initialize at startup. Reads env vars + config file.
    pub fn init(overrides: Option<AiConfig>) -> Result<()>;

    /// Gate: returns false → no AI calls anywhere.
    pub fn is_enabled(&self) -> bool;

    /// Get current provider instance.
    pub fn provider(&self) -> Result<AiProvider>;

    /// Start a new session, return session handle.
    pub fn begin_session(&self, command: &str, intent: &str) -> SessionHandle;

    /// Persist session to disk.
    pub fn commit_session(&self, session: AiSession) -> Result<()>;
}
```

---

## 4. Layer 2: Knowledge & Context System

### 4.1 Module structure

```
crates/oxo-flow-ai/src/knowledge/
├── mod.rs              # Knowledge trait + KnowledgeBase
├── builtin.rs          # Compiled-in domain knowledge (~500 lines)
├── external.rs         # Runtime sources: UrlSource, FileSource, SearchSource
├── assembler.rs        # ContextAssembler: picks + assembles context per scenario
└── templates/
    ├── mod.rs          # PromptTemplate trait
    ├── generate.rs     # Workflow generation prompt
    ├── check.rs        # Dry-run analysis prompt
    ├── diagnose.rs     # Error diagnosis prompt
    └── optimize.rs     # Parameter optimization prompt
```

### 4.2 Builtin knowledge (compiled into binary)

```rust
pub struct BuiltinKnowledge {
    pub tools: &'static [ToolRef],
    pub error_patterns: &'static [ErrorPattern],
    pub best_practices: &'static [BestPractice],
    pub workflow_patterns: &'static [WorkflowPattern],
}

pub struct ToolRef {
    pub name: &'static str,          // "STAR"
    pub domain: &'static str,        // "RNA-seq alignment"
    pub key_params: &'static str,    // "--genomeDir, --runThreadN, --outSAMtype"
    pub cpu_threads: &'static str,   // "16"
    pub memory_gb: &'static str,     // "32"
    pub input_types: &'static str,   // "fastq"
    pub output_types: &'static str,  // "bam"
    pub notes: &'static str,         // "Genome index must be pre-built"
}

pub struct ErrorPattern {
    pub pattern: &'static str,       // "exit code 137"
    pub symptom: &'static str,       // "Process killed by kernel"
    pub likely_cause: &'static str,  // "Out of memory"
    pub fix_action: &'static str,    // "Increase memory allocation, reduce threads"
}

pub struct BestPractice {
    pub id: &'static str,            // "QC-every-rule"
    pub description: &'static str,   // "Every rule must include quality control"
    pub severity: Severity,          // Error | Warning | Info
}

pub struct WorkflowPattern {
    pub name: &'static str,          // "RNA-seq standard"
    pub topology: &'static str,      // "qc → align → quantify → summarize"
    pub tools: &'static [&'static str], // ["fastp", "STAR", "featureCounts", "multiqc"]
}
```

### 4.3 Context assembler

```rust
impl ContextAssembler {
    /// Assemble context for template generation.
    pub fn for_generate(intent: &str) -> AssembledContext {
        self
            .with_tool_table()           // Always loaded
            .with_workflow_patterns()    // Match intent → relevant patterns
            .with_external_sources()     // User-provided URLs/files if present
            .build()
    }

    /// Assemble context for dry-run check.
    pub fn for_check(workflow: &ParsedWorkflow) -> AssembledContext {
        self
            .with_best_practices()       // All best-practice rules
            .with_tools_for(&workflow)   // Only tools used in this workflow
            .with_schema_rules()         // Valid TOML schema expected
            .build()
    }

    /// Assemble context for run error diagnosis.
    pub fn for_diagnose(failure: &RuleFailure) -> AssembledContext {
        self
            .with_error_patterns()       // Error pattern matching
            .with_failed_rule(failure)   // The rule that failed + its env
            .with_logs(&failure.stderr)  // Actual error output
            .build()
    }
}
```

### 4.4 Prompt templates

Each template is a function: `(domain_context) → (system_prompt, user_prompt)`.

**Generate template** (template command):

```text
## Role
You are oxo-flow's bioinformatics pipeline architect.

## Domain Knowledge (injected at runtime)
{tool_table}
{workflow_patterns}
{external_sources}

## Process
1. Analyze the user's intent
2. Select optimal tools (you can call `lookup_tool` to query tool details)
3. Design DAG: connect rules via depends_on
4. Set resources based on tool reference table
5. Generate valid .oxoflow TOML

## Safety Rules
- NEVER omit resource constraints (threads, memory)
- NEVER disable QC steps
- ALWAYS include environment specification
- NEVER use destructive shell commands

## Output
Generate the complete .oxoflow TOML inside ```toml fences.
```

**Check template** (dry-run command):

```text
## Role
You are oxo-flow's pipeline quality auditor.

## Domain Knowledge
{best_practices}
{tool_references_for_this_workflow}

## Audit Steps
1. Validate DAG structure: no cycles, correct dependencies
2. Check resource allocations against tool reference
3. Verify environment declarations
4. Inspect shell commands for safety issues
5. Cross-reference input/output file patterns

## Output
Report issues as:
{severity}: [{rule}] {finding} → {suggestion}
```

**Diagnose template** (run error recovery):

```text
## Role
You are oxo-flow's pipeline failure diagnostician.

## Knowledge
{error_patterns}
{failed_rule_context}
{error_logs}

## Process
1. Match error signature against known patterns
2. Identify root cause
3. Propose fix (specific changes)
4. Assess fix safety (can it be auto-applied?)

## Output
Root cause: ...
Proposed fix: ...
Safe to auto-apply: yes/no
Modified TOML (if applicable): ```toml...```
```

---

## 5. Layer 3: Agent Framework

### 5.1 Module structure

```
crates/oxo-flow-ai/src/agent/
├── mod.rs           # Agent trait + AgentOutcome + AgentContext
├── orchestrator.rs  # run(): plan → gather → act → validate → archive loop
├── tool.rs          # Tool trait + ToolRegistry + built-in tools
└── archive.rs       # Snapshot management (backup before modify)
```

### 5.2 Agent trait

```rust
#[async_trait]
pub trait Agent: Send + Sync {
    /// Human-readable name for logging.
    fn name(&self) -> &str;

    /// Full agent run — the only public entry point.
    /// Orchestrator handles the plan→gather→act→validate→archive loop.
    async fn run(
        &self,
        ctx: &AgentContext,
        orchestrator: &Orchestrator,
    ) -> Result<AgentOutcome>;
}

pub struct AgentContext {
    pub intent: String,
    pub command: String,
    pub workflow_path: Option<PathBuf>,
    pub workflow_content: Option<String>,
    pub external_sources: Vec<ExternalSource>,
    pub config: AiConfig,
    pub tools: Vec<Box<dyn Tool>>,
    pub session: SessionHandle,
}

pub struct AgentOutcome {
    pub success: bool,
    pub content: Option<String>,          // Generated/modified workflow TOML
    pub modifications: Vec<Modification>,
    pub summary: String,                  // Human-readable summary
    pub confidence: f64,                  // 0.0–1.0
}
```

### 5.3 Orchestrator

```rust
pub struct Orchestrator {
    provider: AiProvider,
    max_rounds: u32,
}

impl Orchestrator {
    /// Execute the standard agent loop.
    pub async fn execute<A: Agent>(
        &self,
        agent: &A,
        ctx: &AgentContext,
    ) -> Result<AgentOutcome> {
        let mut round: u32 = 0;
        let mut messages: Vec<Message> = Vec::new();
        let mut modifications: Vec<Modification> = Vec::new();

        // 1. PLAN — assemble system prompt + initial context
        messages.push(agent.plan(ctx)?);

        loop {
            round += 1;
            if round > self.max_rounds {
                break; // Return best effort
            }

            // 2. GATHER — model can request tools
            let response = self.provider.chat_with_tools(
                &messages,
                &ctx.tools_to_defs(),
            ).await?;

            if let Some(tool_calls) = &response.tool_calls {
                // Model wants to use tools → execute, append results, continue loop
                for tc in tool_calls {
                    let result = self.execute_tool(&tc, &ctx.tools)?;
                    messages.push(tool_result_message(tc, result));
                }
                continue; // Back to chat_with_tools with tool results
            }

            // 3. ACT — model produced final output
            if let Some(content) = &response.content {
                // 4. VALIDATE — agent-specific validation
                let validation = agent.validate(&content, ctx)?;
                if validation.passed {
                    // 5. ARCHIVE
                    let mods = agent.archive(&content, &validation, ctx)?;
                    modifications.extend(mods);
                    return Ok(AgentOutcome {
                        success: true,
                        content: Some(content.clone()),
                        modifications,
                        summary: validation.summary,
                        confidence: self.compute_confidence(&response, round),
                    });
                } else {
                    // Feed validation errors back
                    messages.push(Message {
                        role: MessageRole::User,
                        content: format!(
                            "Validation failed:\n{}\n\nPlease fix these issues.",
                            validation.errors.join("\n")
                        ),
                        tool_calls: None,
                        tool_call_id: None,
                        name: None,
                    });
                }
            }
        }

        // Max rounds exhausted — return last valid state
        Err(anyhow!("Agent exceeded max rounds ({})", self.max_rounds))
    }
}
```

### 5.4 Tool trait + registry

```rust
#[async_trait]
pub trait Tool: Send + Sync {
    fn def(&self) -> ToolDef;
    async fn execute(&self, args: &str) -> Result<String>;
    fn is_read_only(&self) -> bool;
}

pub struct ToolRegistry {
    tools: HashMap<String, Box<dyn Tool>>,
}

impl ToolRegistry {
    pub fn register(&mut self, tool: Box<dyn Tool>);
    pub fn get(&self, name: &str) -> Option<&dyn Tool>;
    pub fn to_defs(&self) -> Vec<ToolDef>;
}
```

**Built-in tools:**

| Tool | Read-only | Purpose |
|------|:---------:|---------|
| `read_file` | ✅ | Read local file content (user-provided references) |
| `fetch_url` | ✅ | Fetch URL content (protocol pages, documentation) |
| `validate_workflow` | ✅ | Run oxo-flow-core schema validation |
| `lookup_tool` | ✅ | Query builtin tool reference table |
| `search_best_practices` | ✅ | Search best practice rules by keyword |
| `list_error_patterns` | ✅ | List known error patterns for matching |
| `web_search` | ✅ | Web search (Phase 4, reserved interface) |
| `write_file` | ❌ | Write modified workflow content (always archived first) |

### 5.5 Agent implementations

```rust
// Phase 1
pub struct TemplateAgent;
// Phase 2
pub struct DryRunAgent;
// Phase 3
pub struct RunRecoveryAgent;
// Phase 2
pub struct ValidateAgent;
pub struct DebugAgent;
```

Each agent implements the `Agent` trait with command-specific `plan()` and `validate()` logic. The orchestrator loop is shared.

### 5.6 Archive on every modification

```rust
impl ArchiveManager {
    /// Backup a file before modifying it.
    /// Returns the backup path: .oxo-flow/ai_archive/{name}/{timestamp}-before.oxoflow
    pub fn backup_before_modify(
        workflow_path: &Path,
        content: &str,
        session_id: &str,
    ) -> Result<PathBuf>;

    /// Save the modified version.
    pub fn save_after_modify(
        workflow_path: &Path,
        content: &str,
        reason: &str,
        session_id: &str,
    ) -> Result<Modification>;
}
```

Modifications are recorded both in the session JSON and as standalone `.oxoflow` files in the archive directory.

---

## 6. Layer 4: Command Integration

### 6.1 Unified flag design

All CLI commands that support AI use a consistent flag set:

| Flag | Type | Applies to | Description |
|------|------|------------|-------------|
| `--ai` | bool | template, dry-run, validate, debug | Enable AI agent for this command |
| `--ai-recover` | bool | run, resume | Enable AI error recovery on failure |
| `--ai-max-retries N` | u32 | all | Override `[ai].max_retries` |
| `--from-url URL` | string | template | External reference URL (repeatable) |
| `--from-file PATH` | path | template | External reference file (repeatable) |

### 6.2 Per-command integration

#### `oxo-flow template`

| Invocation | Behavior |
|------------|----------|
| `oxo-flow template` | List preset templates (unchanged) |
| `oxo-flow template <name>` | Copy preset template (unchanged) |
| `oxo-flow template "<intent>" --ai` | Agent generates workflow from description |
| `oxo-flow template "<intent>" --ai --from-url <URL>` | Agent uses URL content as reference |
| `oxo-flow template "<intent>" --ai --from-file <PATH>` | Agent uses local file as reference |
| `oxo-flow template --ai` | Interactive mode: agent asks clarifying questions |

**Agent flow:**
1. `TemplateAgent.plan()` — parse intent, determine analysis type, identify needed tools
2. `Orchestrator` loop:
   - Agent may call `lookup_tool`, `read_file`, `fetch_url` to gather knowledge
   - Agent generates TOML
   - `validate_workflow` tool checks schema
   - On failure: error fed back, agent fixes (up to max_retries rounds)
3. Output: `.oxoflow` file + session log

#### `oxo-flow dry-run`

| Invocation | Behavior |
|------------|----------|
| `oxo-flow dry-run <WORKFLOW>` | Deterministic variable expansion (unchanged) |
| `oxo-flow dry-run <WORKFLOW> --ai` | Agent analyzes logic, resources, best practices |

**Agent flow:**
1. `DryRunAgent.plan()` — load workflow, identify rules and their tools
2. Agent checks each rule against:
   - Tool reference table (appropriate thread/memory?)
   - Best practice rules (QC present? env declared?)
   - DAG correctness (dependencies valid? no cycles?)
   - Shell safety (destructive commands? hardcoded paths?)
3. Output: audit report with severity ratings
4. If `auto_fix = "always"`: agent proposes modifications + can apply them

#### `oxo-flow run` (with `--ai-recover`)

| Invocation | Behavior |
|------------|----------|
| `oxo-flow run <WORKFLOW>` | Normal execution (unchanged) |
| `oxo-flow run <WORKFLOW> --ai-recover` | On failure: Agent diagnoses → fixes → retries |

**Agent flow on failure:**
1. `RunRecoveryAgent.plan()` — capture failed rule, exit code, stderr, logs
2. Agent matches error against `error_patterns` knowledge
3. Agent proposes fix (modify TOML params, add env, fix dependencies)
4. `archive.backup_before_modify()` — original file saved
5. Fix applied, workflow re-executed from checkpoint
6. If fix succeeds: summary logged, archived files preserved
7. If fix fails: original restored, diagnostic report saved

#### `oxo-flow validate`

| Invocation | Behavior |
|------------|----------|
| `oxo-flow validate <WORKFLOW>` | Schema + DAG validation (unchanged) |
| `oxo-flow validate <WORKFLOW> --ai` | Schema + DAG + semantic + best-practice check |

#### `oxo-flow debug`

| Invocation | Behavior |
|------------|----------|
| `oxo-flow debug <WORKFLOW>` | Expand variables, show commands (unchanged) |
| `oxo-flow debug <WORKFLOW> --ai` | AI explains expanded commands, flags potential issues |

---

## 7. Scope-Level AI Configuration

AI behavior is configurable at four scope levels, each overriding the level above:

```
Global (~/.oxo-flow/ai_config.json)
  └── Project (.oxo-flow/ai.toml)
       └── Workflow ([ai] section in .oxoflow)
            └── Rule ([ai] section in [[rules]])
```

```toml
# Global: ~/.oxo-flow/ai_config.json
{
  "provider": "deepseek",
  "model": "deepseek-v4-pro",
  "auto_fix": "ask",
  "max_retries": 3
}

# Project: .oxo-flow/ai.toml (new)
[ai]
enabled = true
max_retries = 5

# Workflow: workflow.oxoflow
[ai]
auto_fix = "always"

[[rules]]
name = "star_align"

[rules.ai]
enabled = false   # AI won't touch this rule
```

### Resolution logic (later wins)

```rust
impl AiConfig {
    pub fn resolve(global: &AiConfig, project: Option<&AiConfig>,
                   workflow: Option<&AiConfig>, rule: Option<&AiConfig>) -> AiConfig {
        let mut resolved = global.clone();
        if let Some(p) = project  { resolved.merge(p); }
        if let Some(w) = workflow { resolved.merge(w); }
        if let Some(r) = rule     { resolved.merge(r); }
        resolved
    }
}
```

---

## 8. Plugin System Extension (Phase 5)

### 8.1 New AI-related plugin types

Extending the existing `plugin.rs` trait system:

```rust
/// AI tool plugin — custom tools the agent can call.
pub trait AiToolPlugin: Send + Sync {
    fn tool_name(&self) -> &str;
    fn tool_def(&self) -> ToolDef;
    async fn execute(&self, args: &str) -> Result<String>;
}

/// AI knowledge plugin — custom knowledge sources.
pub trait AiKnowledgePlugin: Send + Sync {
    fn knowledge_name(&self) -> &str;
    fn domain(&self) -> &str;
    fn query(&self, intent: &str) -> Result<Vec<KnowledgeSnippet>>;
}

/// AI validator plugin — custom validation rules.
pub trait AiValidatorPlugin: Send + Sync {
    fn validator_name(&self) -> &str;
    fn check(&self, rule: &Rule, toml: &str) -> Result<Vec<ValidationIssue>>;
}
```

### 8.2 Plugin scope in workflow

```toml
[plugins]
ai_tools = ["custom-db-query", "org-knowledge-base"]
ai_validators = ["fda-compliance"]

[ai]
enabled = true
# Plugins auto-register their tools/validators in the agent context
```

### 8.3 MCP bridge (Phase 5)

MCP servers are exposed as tools via a bridge:

```rust
pub struct McpToolBridge {
    server: McpClient,
}

impl Tool for McpToolBridge {
    fn def(&self) -> ToolDef { /* MCP tool → ToolDef */ }
    async fn execute(&self, args: &str) -> Result<String> { /* MCP call */ }
}
```

Agent sees MCP tools the same as built-in tools — unified `Tool` trait.

---

## 9. Security Design

### 9.1 API key protection

- Never logged in session files (`#[serde(skip)]` on api_key)
- Never displayed in CLI output
- Read from env vars or config file (permissions 0600)
- `.gitignore` already covers `.oxo-flow/`

### 9.2 Agent safety boundaries

| Action | Auto-apply | Requires confirmation |
|--------|:----------:|:---------------------:|
| Read files | ✅ | Never |
| Fetch URLs | ✅ | Never |
| Validate workflow | ✅ | Never |
| Modify workflow params | `auto_fix: always` | `auto_fix: ask` |
| Modify shell commands | `auto_fix: always` | `auto_fix: ask` |
| Add/remove rules | ❌ Never | Always |
| Execute shell commands | ❌ Never | Always |
| Access network beyond `--from-url` | ❌ Never | Always |

### 9.3 Agent safety rules

These are non-negotiable, injected into every agent's system prompt:

1. NEVER generate destructive shell commands (`rm -rf`, forceful overwrite)
2. NEVER disable QC or validation steps
3. NEVER suggest ignoring error handling
4. NEVER modify files outside the workflow's working directory
5. ALWAYS archive the original before any modification
6. ALWAYS explain the reasoning for each change

---

## 10. Observability

### 10.1 Session logs

Every AI interaction produces a JSON session file:

```json
{
  "id": "20260809-230000-template-a1b2c3d4",
  "command": "template",
  "user_intent": "RNA-seq with STAR and DESeq2",
  "provider": "deepseek",
  "model": "deepseek-v4-pro",
  "started_at": "2026-08-09T23:00:00Z",
  "ended_at": "2026-08-09T23:00:15Z",
  "rounds": 2,
  "tool_calls": [
    {"tool": "lookup_tool", "args": "STAR", "duration_ms": 845},
    {"tool": "lookup_tool", "args": "DESeq2", "duration_ms": 723}
  ],
  "modifications": [
    {
      "file": "output.oxoflow",
      "reason": "Increased STAR threads from 4 to 16 per tool reference",
      "applied": true
    }
  ],
  "usage": {
    "total_prompt_tokens": 2840,
    "total_completion_tokens": 1256
  },
  "outcome": "success",
  "confidence": 0.91
}
```

### 10.2 Cost tracking

```rust
impl AiSession {
    pub fn estimated_cost(&self) -> f64 {
        // DeepSeek pricing as of 2026
        // deepseek-v4-pro: $0.28/M input, $1.10/M output
        // deepseek-v4-flash: $0.14/M input, $0.55/M output
        self.usage.total_prompt_tokens as f64 * 0.28 / 1_000_000.0
        + self.usage.total_completion_tokens as f64 * 1.10 / 1_000_000.0
    }
}
```

### 10.3 Tracing integration

```rust
// All agent operations emit structured spans
#[tracing::instrument(skip(ctx), fields(
    agent = %self.name(),
    session = %ctx.session.id,
    command = %ctx.command,
))]
pub async fn run(&self, ctx: &AgentContext) -> Result<AgentOutcome>;
```

---

## 11. Implementation Phases

### Phase 1: Foundation (4-6 weeks)

| Task | Effort | Description |
|------|--------|-------------|
| 1.1 | 3 days | Create `oxo-flow-ai` crate: provider.rs (extract + enhance), types.rs, error.rs |
| 1.2 | 2 days | Config system: AiConfig, resolution chain, AiRegistry |
| 1.3 | 2 days | Session system: AiSession, Modification, JSON persistence |
| 1.4 | 3 days | Tool trait + ToolRegistry + built-in tools (read_file, fetch_url, validate_workflow, lookup_tool) |
| 1.5 | 2 days | Agent trait + Orchestrator (plan→gather→act→validate→archive loop) |
| 1.6 | 3 days | Knowledge system: BuiltinKnowledge, ContextAssembler, Generate prompt template |
| 1.7 | 3 days | TemplateAgent + CLI template command integration |
| 1.8 | 2 days | Web crate migration: delete ai_provider.rs + copilot.rs, depend on oxo-flow-ai |
| 1.9 | 2 days | Tests + make ci green |

**Phase 1 deliverable:** `oxo-flow template "RNA-seq" --ai` generates a valid workflow using DeepSeek.

### Phase 2: Dry-run + Validate AI (2-3 weeks)

| Task | Effort | Description |
|------|--------|-------------|
| 2.1 | 2 days | Check prompt template + best practice rules |
| 2.2 | 3 days | DryRunAgent: logic check, resource audit, DAG analysis |
| 2.3 | 2 days | ValidateAgent: semantic validation |
| 2.4 | 2 days | `--ai` flag on dry-run and validate commands |
| 2.5 | 2 days | Tests |

**Phase 2 deliverable:** `oxo-flow dry-run wf.oxoflow --ai` produces an AI audit report.

### Phase 3: Run Error Recovery (3-4 weeks)

| Task | Effort | Description |
|------|--------|-------------|
| 3.1 | 2 days | Diagnose prompt template + error pattern knowledge |
| 3.2 | 3 days | RunRecoveryAgent: diagnose → propose fix → archive → apply → retry |
| 3.3 | 2 days | Integration with checkpoint/resume system |
| 3.4 | 2 days | `--ai-recover` flag on run and resume commands |
| 3.5 | 3 days | Tests (mock failures, verify recovery) |

**Phase 3 deliverable:** `oxo-flow run wf.oxoflow --ai-recover` recovers from failures autonomously.

### Phase 4: Scope Config + Plugin Extension (2-3 weeks)

| Task | Effort | Description |
|------|--------|-------------|
| 4.1 | 2 days | Scope-level config: project/rule-level `[ai]` resolution |
| 4.2 | 3 days | AI plugin types: AiToolPlugin, AiKnowledgePlugin, AiValidatorPlugin |
| 4.3 | 2 days | Web search tool (reserved interface) |
| 4.4 | 2 days | Interactive template mode (agent asks clarifying questions) |
| 4.5 | 2 days | Tests |

**Phase 4 deliverable:** AI configurable per-rule, external tool/knowledge plugins.

### Phase 5: MCP/Skill Ecosystem (4-6 weeks)

| Task | Effort | Description |
|------|--------|-------------|
| 5.1 | 3 days | MCP client integration (connect to external MCP servers) |
| 5.2 | 3 days | MCP → Tool bridge |
| 5.3 | 3 days | Skill system: reusable AI capability packages |
| 5.4 | 3 days | Skill marketplace design (manifest, signing, discovery) |
| 5.5 | 2 days | Community contribution guide |

**Phase 5 deliverable:** Community can create and share AI skills via MCP servers.

---

## 12. Migration Plan for Web Crate

### Files to delete from `oxo-flow-web`

```
src/ai_provider.rs           → moved to oxo-flow-ai::provider
src/domains/ai/copilot.rs    → moved to oxo-flow-ai::knowledge::templates
```

### Files to modify in `oxo-flow-web`

```
src/domains/ai/service.rs    → replace provider.chat() with oxo_flow_ai::AI::provider().chat()
src/domains/ai/handlers.rs   → adjust imports
src/server.rs                → replace AiProviderRegistry::init() with oxo_flow_ai::AI::init()
```

### Backward compatibility

- `ai_provider.rs` types marked `#[deprecated]` for one release cycle, re-exporting from `oxo-flow-ai`
- Env vars unchanged: `OXO_FLOW_AI_PROVIDER`, `ANTHROPIC_AUTH_TOKEN`, `OPENAI_API_KEY`, `DEEPSEEK_API_KEY`
- Config file location unchanged: `~/.oxo-flow/ai_config.json`

---

## 13. Testing Strategy

### 13.1 Unit tests (per Phase)

- Provider: response parsing, error handling, tool call serialization
- Config: resolution chain, merge logic, env var → config mapping
- Session: JSON roundtrip, modification tracking
- Tool: each built-in tool's execute() with mock inputs
- Agent: Orchestrator loop with mock provider
- Knowledge: assembler output for each scenario

### 13.2 Integration tests

- Template: `oxo-flow template "RNA-seq" --ai` → valid .oxoflow produced
- Dry-run: `oxo-flow dry-run known-bad.oxoflow --ai` → issues detected
- Run: simulate rule failure → AI diagnosis → recovery

### 13.3 Manually seeded tests

```rust
#[test]
fn template_generates_valid_workflow() {
    let outcome = TemplateAgent::run("RNA-seq alignment with STAR", &ctx)?;
    assert!(outcome.success);
    let validation = oxo_flow_core::validate(&outcome.content.unwrap())?;
    assert!(validation.valid);
}

#[test]
fn dry_run_detects_missing_threads() {
    let workflow = "[workflow]\n[[rules]]\nname='bad'\nshell='echo hi'";
    let outcome = DryRunAgent::run(workflow, &ctx)?;
    assert!(outcome.summary.contains("missing resource constraints"));
}
```

### 13.4 No regressions

- All 858 existing tests must pass (`make ci`)
- AI gated behind `AI::is_enabled()` → all existing tests run with AI disabled by default
- CI adds `oxo-flow-ai` to workspace build/test matrix

---

## 14. Decision Log

| Decision | Rationale |
|----------|-----------|
| L1 doesn't depend on core | Keeps AI crate reusable; prevents circular deps; enables independent testing |
| Agent trait over free functions | Each command has different plan/validate logic, shared orchestrator loop |
| Tool calling via native API format | DeepSeek, OpenAI, and Claude all support OpenAI-compatible tool calling — no need for custom protocol |
| Modified files archived as .oxoflow snapshots | Human-readable diff, easy to inspect, no binary formats |
| `auto_fix` default = `"ask"` | Safety: users should see what AI changed before it runs |
| Session JSON + archive directory | JSON for programmatic analysis + .oxoflow snapshots for quick human review |
| Phase 5 MCP deferred | Core value delivered in Phases 1-4; MCP extends reach to third-party tool ecosystems |

---

## 15. Implementation Log

### Phase 1 Complete (2026-08-09/10)

**Delivered:**
- `oxo-flow-ai` crate with full Layer 1–3 infrastructure
- `oxo-flow template "<intent>" --ai` working with DeepSeek
- `--from-url`, `--from-file`, `--ai-max-retries` flags on template command

**Files created:**
- `crates/oxo-flow-ai/` — 9 source files, 54 unit tests
- `crates/oxo-flow-cli/src/commands/ai_template.rs` — 7 tests

**Files modified:**
- `Cargo.toml` — added workspace member + dep
- `crates/oxo-flow-cli/Cargo.toml` — added oxo-flow-ai dep
- `crates/oxo-flow-cli/src/main.rs` — Template command AI flags
- `crates/oxo-flow-cli/src/commands/mod.rs` — ai_template module
- `crates/oxo-flow-cli/src/commands/project.rs` — async template_command

**Verified:**
- `make ci`: fmt + clippy + build + test all passing (audit skipped — DNS timeout)
- Manual test: `DEEPSEEK_API_KEY=<key> OXO_FLOW_AI_PROVIDER=deepseek oxo-flow template "RNA-seq" --ai` → generated valid 4-rule workflow
- All 226+ tests passing across workspace

**Deferred to Phase 2:**
- Web crate migration (delete ai_provider.rs, copilot.rs; depend on oxo-flow-ai)
- Dry-run + validate AI integration
- Run error recovery with --ai-recover
