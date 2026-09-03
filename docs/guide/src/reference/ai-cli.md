# AI-Powered CLI

oxo-flow can use AI (DeepSeek, Claude, OpenAI, or Ollama) to help you create, validate, and fix bioinformatics workflows. AI is **optional** — no AI code runs unless explicitly enabled.

> **Design principle**: Add one line of config. AI activates automatically on all suitable commands.

---

## Quick Start

### 1. Configure Your AI Provider

oxo-flow supports any AI provider with an OpenAI-compatible or Anthropic-compatible API.
Set the provider and your API key via environment variables:

```bash
# DeepSeek (OpenAI-compatible)
export OXO_FLOW_AI_PROVIDER=deepseek
export DEEPSEEK_API_KEY="sk-<YOUR-KEY>"

# Claude (Anthropic)
export OXO_FLOW_AI_PROVIDER=claude
export ANTHROPIC_AUTH_TOKEN="sk-ant-<YOUR-KEY>"

# OpenAI
export OXO_FLOW_AI_PROVIDER=openai
export OPENAI_API_KEY="sk-<YOUR-KEY>"

# Any OpenAI-compatible service (Groq, Together, Azure, Fireworks, etc.)
export OXO_FLOW_AI_PROVIDER=openai
export OPENAI_BASE_URL="https://api.groq.com/openai/v1"
export OPENAI_API_KEY="gsk_<YOUR-KEY>"
export OPENAI_MODEL="llama-3.1-70b"

# Any Anthropic-compatible service
export OXO_FLOW_AI_PROVIDER=claude
export ANTHROPIC_BASE_URL="https://your-custom-endpoint"
export ANTHROPIC_AUTH_TOKEN="<YOUR-KEY>"

# Local Ollama (no API key needed)
export OXO_FLOW_AI_PROVIDER=ollama
export OXO_FLOW_AI_API_URL="http://localhost:11434"
export OXO_FLOW_AI_MODEL="llama3"
```

Configuration persists to `~/.oxo-flow/ai_config.json` after first use.

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

## Workflow Explanation (`ai explain`)

Explain a workflow in plain language — for newcomers reading an inherited
pipeline, and for experienced users evaluating one. Three layers:

1. **Overview** — what the workflow does, its assay domains, entry and final rules
2. **Step-by-step** — each rule's tool, purpose, inputs/outputs, and resources
3. **Scientific review** — deterministic, evidence-backed findings (GATK best
   practices etc.) explained in plain language

```bash
oxo-flow ai explain workflow.oxoflow              # beginner-level prose (default)
oxo-flow ai explain workflow.oxoflow --step bwa_mem2_align   # focus one rule
oxo-flow ai explain workflow.oxoflow --level expert          # parameter-level, efficiency-focused
oxo-flow ai explain workflow.oxoflow --json       # machine-readable output
```

`ai explain` complements `dry-run`: **dry-run shows the mechanism** (what will
execute), **explain shows the principle** (why each step exists). Every fact
in the explanation is computed deterministically from the workflow definition
plus the embedded knowledge bases (tool table, bioSkills, pipeline graph,
scientific preflight) — the model only writes prose over those verified
facts, so it cannot invent parameters, versions, or caveats. The deterministic
findings are always printed separately (stderr, or the `review` array in
`--json`), so nothing critical is silently dropped even if the model omits it.

`--json` emits the deterministic skeleton (rule metadata, I/O, resources,
matched knowledge, findings) with model-written prose in dedicated fields —
safe for documentation generators. `--level` defaults to `beginner` because
the audience cannot be inferred from the workflow itself; experts opt in.

### Degraded mode (no model)

`ai explain` exits 0 with the deterministic skeleton whenever the model
layer is unavailable — provider errors, quota limits, dead endpoints, or
explicit disable:

- `OXO_FLOW_AI_PROVIDER=disabled` overrides any saved provider
  configuration and produces the skeleton with a "disabled" note on stderr.
- A failed provider call degrades the same way, with the error summarized
  in the note; the skeleton and its knowledge-base grounding (bioSkills,
  tool table, pipeline graph) are still emitted in full, and `--json`
  consumers still get the JSON contract (prose fields empty).

```json
{
  "workflow_name": "wgs-germline-calling",
  "level": "beginner",
  "domains": ["read-alignment", "variant-calling"],
  "steps": [
    {
      "order": 1,
      "name": "fastp_qc",
      "depends_on": [],
      "inputs": ["raw/{sample}_R1.fastq.gz"],
      "outputs": ["trimmed/{sample}_R1.fq.gz"],
      "threads": 4,
      "tools": ["fastp"],
      "explanation": "Trims adapters and low-quality bases..."
    }
  ],
  "review": [
    {
      "code": "SCI-VQSR-COHORT",
      "rule": "vqsr_snps",
      "message": "VariantRecalibrator trains on the cohort, but only 3 sample(s) are in this run...",
      "suggestion": "stop the pilot before VQSR (-t <rule>) or use hard filtering..."
    }
  ],
  "provenance": {"model": "deepseek-v4-pro", "bio_skills": 562, "pipeline_graph_nodes": 78}
}
```

---

## AI Robustness

The provider layer treats model failures explicitly — nothing is silently dropped:

- **Broken tool-call arguments are repaired**: if a model (e.g. DeepSeek under
  load) truncates a tool call's arguments JSON, it is repaired to `{}` and the
  call still executes; structurally broken calls (missing id/name) raise a
  `ToolError` instead of being ignored.
- **Context overflows are classified and recovered**: HTTP 400/413 responses
  with context-window markers map to `ContextOverflow`; the transcript is
  compressed (system + recent turns kept, middle turns replaced by a marker)
  and retried **once**. A second overflow returns readable guidance instead of
  burning quota.
- **Tool results are bounded**: every tool result entering a transcript is
  capped at 16 KiB with a UTF-8-safe head+tail truncation marker, so long
  lookups cannot silently blow up the context window.
- **Offline replay for tests**: a scripted provider replays serialized
  completions (tool calls, errors, delays) through the real code path, so CI
  covers multi-turn tool-calling and error recovery without an API key.

---

## Configuration

### Global Config (`~/.oxo-flow/ai_config.json`)

```json
{
  "provider": "deepseek",
  "model": "deepseek-v4-pro",
  "api_key": "sk-<YOUR-KEY>"
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
| `OXO_FLOW_AI_API_KEY` | Generic API key (fallback for all providers) | — |
| `OXO_FLOW_AI_API_URL` | Custom API endpoint URL | (provider default) |
| `OXO_FLOW_AI_MODEL` | Model name override | (provider default) |
| `DEEPSEEK_API_KEY` | DeepSeek (OpenAI-compatible) | — |
| `DEEPSEEK_BASE_URL` | Custom DeepSeek endpoint | `https://api.deepseek.com/v1/chat/completions` |
| `ANTHROPIC_AUTH_TOKEN` | Anthropic-compatible API key | — |
| `CLAUDE_API_KEY` | Alias for `ANTHROPIC_AUTH_TOKEN` | — |
| `ANTHROPIC_MODEL` | Claude model override | `claude-sonnet-4-20250514` |
| `ANTHROPIC_BASE_URL` | Custom Anthropic endpoint | `https://api.anthropic.com/v1/messages` |
| `OPENAI_API_KEY` | OpenAI-compatible API key | — |
| `OPENAI_MODEL` | OpenAI model override | `gpt-4o` |
| `OPENAI_BASE_URL` | Custom OpenAI-compatible endpoint | `https://api.openai.com/v1/chat/completions` |
| `OLLAMA_HOST` | Ollama server address (web service only; the CLI reads `OXO_FLOW_AI_API_URL`) | `http://localhost:11434` |

---

## How It Works

The AI agent combines four embedded knowledge sources (all compiled into the binary at build time):

1. **Tool Reference Table**: 40 curated bioinformatics tools with resource allocations (threads, memory)
2. **Bioconda Tool Database**: 6,487 curated CLI tools with current versions and descriptions (filtered from 12,679 raw registry entries; see `knowledge_meta.json`) — queried on demand via `lookup_tool`
3. **bioSkills Library**: 562 curated Agent Skills (the emerging SKILL.md standard) with domain procedures, commands, and caveats — matched by assay type and injected into generation prompts, or queried via `lookup_skill`
4. **Pipeline Knowledge Graph**: 78 workflow skills and 465 literature-backed data-flow transitions (BAM → VCF → annotated VCF chains) — queried via `lookup_pipeline` to design correct multi-step topologies

Token efficiency: embedded data is **never added to the LLM context wholesale**. Only on-demand tool queries (~1 KB per result) and domain-matched skill summaries (≤3 per domain) are injected — the rest stays in the binary until needed.

The agent:
1. Analyzes your intent (assay type, tools mentioned)
2. Matches relevant bioSkills domains and injects curated expertise
3. Selects tools from the reference table, verifying current versions via the Bioconda database
4. Designs a DAG with proper dependencies (optionally consulting the pipeline graph for topology)
5. Sets resource allocations based on tool requirements
6. Generates valid `.oxoflow` TOML
7. Validates against the schema and reports any issues

### Custom Skills

User-defined `.skill.toml` files (discovered from `~/.oxo-flow/skills` and
`<project>/.oxo-flow/skills`) can be activated per workflow via
`[ai] skills = [...]` — their `prompt_additions` are appended to the system
prompt. See [Custom Skills](./custom-skills.md).

### Session Logs

Every AI interaction is logged to `.oxo-flow/ai_sessions/` for audit and debugging:

```json
{
  "id": "20260809-230000-template-a1b2c3d4",
  "command": "template",
  "user_intent": "RNA-seq with STAR and DESeq2",
  "provider": "deepseek",
  "model": "deepseek-v4-pro",
  "total_usage": { "prompt_tokens": 1234, "completion_tokens": 432 },
  "outcome": "success",
  "confidence": 0.91
}
```

---

## Provider Comparison

| Provider | Model Examples | Cost (per 1M tokens) | Quality | Setup |
|----------|---------------|----------------------|---------|-------|
| DeepSeek | DeepSeek v4-pro, DeepSeek v4-flash | ~$0.14–$2.50 in | High | API key (`DEEPSEEK_API_KEY`) |
| OpenAI-compatible | Groq LLaMA, Together, Azure | Varies ($0.14–$2.50 in) | High | API key |
| Anthropic-compatible | Claude Sonnet 4 | Varies ($3–$15 in) | Best | API key |
| Local | Ollama (llama3, mistral, etc.) | Free | Moderate | Local install |

**Recommendation**: An OpenAI-compatible provider with high throughput (e.g., DeepSeek, Groq) offers the best cost-quality balance for bioinformatics workflows.

## Complete Command Reference

| Command | AI Flag | Auto-Detect | What It Does |
|---------|---------|:-----------:|--------------|
| `oxo-flow ai` | — | — | Quick status: provider, model, endpoint, connectivity, session count |
| `oxo-flow ai test` | — | — | Comprehensive self-test: connectivity + generation + analysis (tests the AI integration itself — not a workflow; for workflow testing use [`oxo-flow test`](../commands/test.md)) |
| `oxo-flow ai setup` | — | — | Interactive wizard: choose provider, enter key, save config |
| `oxo-flow ai explain WORKFLOW` | — | — | Three-layer workflow explanation (overview → steps → scientific review) |
| `oxo-flow ai explain WORKFLOW --step RULE` | — | — | Focus the explanation on one rule |
| `oxo-flow ai explain WORKFLOW --level beginner\|expert` | — | — | Explanation depth (default `beginner`) |
| `oxo-flow ai explain WORKFLOW --json` | — | — | Machine-readable skeleton + prose (stdout is pure JSON) |
| `oxo-flow template "X" --ai` | required | ❌ | Natural language → .oxoflow file |
| `oxo-flow template "X" --ai --from-url URL` | required | ❌ | Generate with web page as reference |
| `oxo-flow template "X" --ai --from-file PATH` | required | ❌ | Generate with local file as reference |
| `oxo-flow template "X" --ai -o PATH` | required | ❌ | Write output to specific path |
| `oxo-flow template "X" --ai --ai-max-retries N` | required | ❌ | Override max correction rounds |
| `oxo-flow env create "X" --ai` | required | ❌ | Natural language → pinned conda environment YAML (or pixi TOML with `--backend pixi`) |
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

## Embedded Knowledge Freshness

The four knowledge sources are compiled into the binary at build time, so
they only change when the binary is rebuilt. Every data file is documented
by `crates/oxo-flow-ai/src/knowledge/knowledge_meta.json` — a plain JSON
document listing each source's record count, generation timestamp, and
whether it is auto-updated or manually curated:

```json
{
  "sources": {
    "bioconda_tools": {
      "count": 6487,
      "url": "https://conda.anaconda.org/bioconda/channeldata.json",
      "excluded": 6192,
      "data_file": "bioconda_tools.jsonl",
      "generated_at": "2026-09-01T08:23:27Z",
      "auto": true
    }
  },
  "generated_at": "2026-09-01T08:23:27Z"
}
```

`count` always equals the number of records in `data_file` — a drift-guard
test fails the build if the two ever diverge.

### Seeing freshness

`oxo-flow ai` prints a **Knowledge freshness** section: per-source record
count, generation date, staleness in days, and the `auto`/`manual` origin.
Auto-updated sources older than 60 days are flagged `STALE`. The
`lookup_tool` tool response likewise appends a freshness note (data date +
record count), so AI agents can weigh how current the embedded database is.

### Update cadence and the Knowledge Refresh workflow

A scheduled GitHub Action — the **Knowledge Refresh** workflow
(`.github/workflows/refresh-knowledge.yml`) — runs on the 1st and 16th of
each month at 03:00 UTC, and can be dispatched manually at any time
(Actions → Knowledge Refresh → Run workflow). It re-pulls each upstream
source, rewrites the JSONL data files only when content changed, and opens
a reviewed PR (`chore(knowledge): automatic knowledge refresh <date>`)
with per-source entry-count changes. A run that finds no data changes
completes green without a PR. A failing upstream source is tolerated — the
other sources still refresh and the failure is reported in the PR body.

### Staleness gate

The release pipeline rejects a dispatch release when any `auto: true`
source is older than 60 days (twice the 16-day cadence, tolerating one
missed run). The gate lives in `ci.yml`'s version-sync job, so a stale
snapshot is blocked before any version bump or tag is created. Manually
curated sources (`auto: false`) are exempt — their freshness is the
maintainer's call. If a release is blocked, run the Knowledge Refresh
workflow, merge its PR, and re-dispatch the release.

---

## Troubleshooting

### "AI provider not configured"

```bash
# Check your configuration
oxo-flow ai                    # Shows provider status and connectivity
echo $OXO_FLOW_AI_PROVIDER     # Should not be empty or "disabled"

# Set up any provider
export OXO_FLOW_AI_PROVIDER=<provider>
export <PROVIDER>_API_KEY="sk-<YOUR-KEY>"
```

### "AI response did not contain valid TOML"

The AI returned a response without valid `.oxoflow` TOML content. This is rare — try again with a more specific description.

### Rate Limiting

If you see rate limit errors, wait a few seconds and retry. For production use, consider upgrading your API plan or switching to a provider with higher rate limits.

---

## Roadmap

| Phase | Features | Status |
|-------|----------|--------|
| Phase 1 | AI template generation | ✅ Complete |
| Phase 2 | AI dry-run + validate analysis | ✅ Complete |
| Phase 3 | AI error recovery (`--ai-recover`) | ✅ Complete |
| Phase 4 | Scope-level AI config, AI plugin types | ✅ Complete |
| Phase 5 | MCP/Skill ecosystem | ✅ Complete |
| Phase 6 | Workflow explanation (`ai explain`) | ✅ Complete |
| Phase 7 | Provider robustness (tool-call repair, overflow recovery, bounded results) | ✅ Complete |

See the full design spec at `docs/superpowers/specs/2026-08-09-ai-native-cli-design.md` (repository root, not included in the published docs) for architecture details.
