# AI-Powered CLI

oxo-flow can use AI (DeepSeek, Claude, OpenAI, or Ollama) to help you create, validate, and fix bioinformatics workflows. AI is **optional** — no AI code runs unless explicitly enabled.

> **Design principle**: Add one line of config. AI activates automatically on all suitable commands.

---

## Quick Start

### 1. Configure Your AI Provider

DeepSeek supports **two** API formats — both verified working:

#### OpenAI-compatible format (recommended)

```bash
export DEEPSEEK_API_KEY="sk-..."
export OXO_FLOW_AI_PROVIDER=deepseek
```

#### Anthropic-compatible format

```bash
export ANTHROPIC_AUTH_TOKEN="sk-..."
export ANTHROPIC_BASE_URL="https://api.deepseek.com/anthropic"
export ANTHROPIC_MODEL="deepseek-chat"
export OXO_FLOW_AI_PROVIDER=claude
```

#### Other providers

```bash
# DeepSeek (recommended for cost + quality balance)
export DEEPSEEK_API_KEY="sk-..."
export OXO_FLOW_AI_PROVIDER=deepseek
```

Configuration persists to `~/.oxo-flow/ai_config.json`.

### 2. Enable AI in Your Workflow

Add one section to your `.oxoflow` file:

```toml
[ai]
enabled = true
```

That's it. Now **all** AI-capable commands auto-activate without extra flags:

```bash
oxo-flow dry-run workflow.oxoflow     # AI analysis runs automatically
oxo-flow validate workflow.oxoflow   # AI semantic validation
oxo-flow run workflow.oxoflow        # AI error recovery on failure
```

### 3. Generate a New Workflow

```bash
oxo-flow template "RNA-seq analysis with STAR alignment and featureCounts quantification" --ai
```

### 4. Override with CLI Flags

```bash
# Force AI on (even without [ai] section)
oxo-flow dry-run workflow.oxoflow --ai

# Force AI off (even with [ai] section) 
# (just don't pass --ai — AI is only auto-detected, never forced)
```

### 3. Review and Run

```bash
# Review the generated workflow
cat rnaseq_analysis_with.oxoflow

# Validate it
oxo-flow validate rnaseq_analysis_with.oxoflow

# Run it
oxo-flow run rnaseq_analysis_with.oxoflow
```

---

## Template Command Reference

### Basic Generation

```bash
oxo-flow template "<description>" --ai
```

**Examples:**

```bash
oxo-flow template "DNA variant calling with BWA and GATK" --ai
oxo-flow template "ChIP-seq peak calling with MACS2" --ai
oxo-flow template "metagenomics taxonomic classification with Kraken2" --ai
```

### With Reference Materials

Use `--from-url` or `--from-file` to provide protocol documents or example workflows:

```bash
# From a URL (protocol, documentation, paper methods)
oxo-flow template "scRNA-seq analysis" --ai \
    --from-url https://satijalab.org/seurat/articles/pbmc3k_tutorial.html

# From a local file
oxo-flow template "custom QC pipeline" --ai \
    --from-file my-protocol.md \
    --from-file data/example_output.txt

# Both URL and file
oxo-flow template "exome analysis" --ai \
    --from-url https://gatk.broadinstitute.org/ \
    --from-file data/cohort_info.csv
```

### Set Output Path

```bash
oxo-flow template "RNA-seq" --ai -o my-pipeline.oxoflow
```

### Control AI Behavior

```bash
# Limit correction rounds
oxo-flow template "RNA-seq" --ai --ai-max-retries 5
```

---

## Configuration

### Global Config (`~/.oxo-flow/ai_config.json`)

```json
{
  "provider": "deepseek",
  "model": "deepseek-v4-pro",
  "api_key": "sk-..."
}
```

### Workflow-Level Config (`[ai]` section in `.oxoflow`)

```toml
[ai]
enabled = true
max_retries = 5
auto_fix = "ask"    # "ask" | "always" | "never"
model = "deepseek-v4-flash"
```

### Environment Variables

| Variable | Description | Default |
|----------|-------------|---------|
| `OXO_FLOW_AI_PROVIDER` | Provider: `deepseek`, `claude`, `openai`, `ollama` | `disabled` |
| `DEEPSEEK_API_KEY` | DeepSeek API key | — |
| `DEEPSEEK_BASE_URL` | Custom DeepSeek endpoint | `https://api.deepseek.com/v1/chat/completions` |
| `DEEPSEEK_MODEL` | DeepSeek model | `deepseek-v4-pro` |
| `ANTHROPIC_AUTH_TOKEN` | Claude API key | — |
| `OPENAI_API_KEY` | OpenAI API key | — |
| `OXO_FLOW_AI_API_KEY` | Generic API key (all providers) | — |

---

## How It Works

The AI agent uses oxo-flow's built-in knowledge base to generate workflows:

1. **Tool Reference Table**: 10+ bioinformatics tools with optimal resource allocations (threads, memory)
2. **Best Practices**: Mandatory checks (QC, environment declarations, no destructive commands)
3. **Safety Rules**: Non-negotiable constraints injected into every prompt

The agent:
1. Analyzes your intent
2. Selects appropriate tools from the reference table
3. Designs a DAG with proper dependencies
4. Sets resource allocations based on tool requirements
5. Generates valid `.oxoflow` TOML
6. Validates against the schema and reports any issues

### Session Logs

Every AI interaction is logged to `.oxo-flow/ai_sessions/` for audit and debugging:

```json
{
  "id": "20260809-230000-template-a1b2c3d4",
  "command": "template",
  "user_intent": "RNA-seq with STAR and DESeq2",
  "provider": "deepseek",
  "model": "deepseek-v4-pro",
  "rounds": 1,
  "outcome": "success",
  "confidence": 0.91
}
```

---

## Provider Comparison

| Provider | Model | Cost (per 1M tokens) | Quality | Setup |
|----------|-------|----------------------|---------|-------|
| DeepSeek | v4-pro | $0.28 in / $1.10 out | High | API key |
| DeepSeek | v4-flash | $0.14 in / $0.55 out | Good | API key |
| Claude | Sonnet 4 | ~$3 in / $15 out | Best | API key |
| OpenAI | GPT-4o | ~$2.50 in / $10 out | High | API key |
| Ollama | llama3 | Free (local) | Moderate | Local install |

**Recommendation**: DeepSeek v4-pro offers the best cost-quality balance for bioinformatics workflow generation.

## Complete Command Reference

| Command | AI Flag | Auto-Detect | What It Does |
|---------|---------|:-----------:|--------------|
| `oxo-flow ai` | — | — | Show provider, model, endpoint, test connectivity, session count |
| `oxo-flow template "X" --ai` | required | ❌ | Natural language → .oxoflow file |
| `oxo-flow template "X" --ai --from-url URL` | required | ❌ | Generate with web page as reference |
| `oxo-flow template "X" --ai --from-file PATH` | required | ❌ | Generate with local file as reference |
| `oxo-flow template "X" --ai -o PATH` | required | ❌ | Write output to specific path |
| `oxo-flow template "X" --ai --ai-max-retries N` | required | ❌ | Override max correction rounds |
| `oxo-flow dry-run WORKFLOW` | optional | ✅ | AI analysis if [ai] enabled=true |
| `oxo-flow dry-run WORKFLOW --ai` | force | — | Explicit AI override |
| `oxo-flow validate WORKFLOW` | optional | ✅ | AI semantic validation |
| `oxo-flow lint WORKFLOW` | optional | ✅ | AI best-practice linting |
| `oxo-flow debug WORKFLOW` | optional | ✅ | AI command explanation |
| `oxo-flow run WORKFLOW` | optional | ✅ | AI error recovery on failure |
| `oxo-flow run WORKFLOW --ai-recover` | force | — | Explicit recovery override |
| `oxo-flow run WORKFLOW --ai-max-retries N` | optional | — | Max fix attempts |
| `oxo-flow resume CHECKPOINT --ai-recover` | required | — | Diagnose + fix on restart |

## AI Command Explanation (`debug --ai`)

Analyze expanded shell commands with AI:

```bash
oxo-flow debug workflow.oxoflow --ai
oxo-flow debug workflow.oxoflow --ai -r specific_rule
```

The AI explains what each command does and flags potential issues like resource mismatches, missing flags, or incorrect parameter usage.

---

## AI Linting (`lint --ai`)

Add semantic best-practice checks beyond the deterministic linter:

```bash
oxo-flow lint workflow.oxoflow --ai
```

---

## AI Workflow Analysis (`dry-run --ai`)

Validate workflow correctness, safety, and resource allocation before running:

```bash
oxo-flow dry-run workflow.oxoflow --ai
```

The AI checks:
- Resource allocations vs. tool recommendations
- DAG structure (missing dependencies, invalid edges)
- Safety violations (destructive commands)
- Environment declarations
- Best practice adherence

Output includes severity ratings: `[ERROR]`, `[WARNING]`, `[INFO]`.

---

## AI Semantic Validation (`validate --ai`)

Combine schema validation with AI-powered semantic analysis:

```bash
oxo-flow validate workflow.oxoflow --ai
```

---

## AI Error Recovery (`run --ai-recover`)

When a workflow fails, the AI diagnoses the error and proposes a fix:

```bash
oxo-flow run workflow.oxoflow --ai-recover
```

**Recovery flow:**
1. Rule fails → capture error signature
2. AI matches against known error patterns (OOM, segfault, missing files, etc.)
3. AI proposes root cause + specific fix
4. If safe to auto-apply: original archived, fix applied
5. Run can be retried with corrected workflow

---

## Troubleshooting

### "AI provider not configured"

```bash
# Check your environment variables
echo $OXO_FLOW_AI_PROVIDER
echo $DEEPSEEK_API_KEY

# Or set them explicitly
export OXO_FLOW_AI_PROVIDER=deepseek
export DEEPSEEK_API_KEY="sk-..."
```

### "AI response did not contain valid TOML"

The AI returned a response without valid `.oxoflow` TOML content. This is rare — try again with a more specific description.

### Rate Limiting

If you see "Rate limited by deepseek", wait a few seconds and retry. For production use, consider upgrading your DeepSeek API plan.

---

## Roadmap

| Phase | Features | Status |
|-------|----------|--------|
| Phase 1 | AI template generation | ✅ Complete |
| Phase 2 | AI dry-run + validate analysis | ✅ Complete |
| Phase 3 | AI error recovery (`--ai-recover`) | ✅ Complete |
| Phase 4 | Scope-level AI config, AI plugin types | ✅ Complete |
| Phase 5 | MCP/Skill ecosystem | ✅ Complete |

See the [full design spec](../../../../docs/superpowers/specs/2026-08-09-ai-native-cli-design.md) for architecture details.
