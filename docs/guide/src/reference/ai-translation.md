# AI Translation Layer

> **Trust Boundary**: AI can read and suggest — it CANNOT write to the database,
> spawn processes, or modify files. Every AI action is a proposal that requires
> human confirmation.

## Overview

The AI Translation Layer converts natural language descriptions of bioinformatics
analyses into validated `.oxoflow` pipelines. It calls the deterministic Core API
endpoints — it does not bypass them.

```
Intent (NL) → AI Translator → /api/pipelines/validate → Validated .oxoflow
                  │                    │
                  └─ Template match ───┘ (fallback if AI unavailable)
```

## Endpoints

### POST /api/ai/translate

Convert natural language intent to a validated `.oxoflow` pipeline.

```
Input:  { intent: "RNA-seq, PE, hg38, STAR + featureCounts, strand-specific" }
Process (one harness across all generation surfaces):
  1. The shared `PipelineGenAgent` (oxo-flow-ai) runs through the agent
     orchestrator: engine-accurate prompt, read-only knowledge tools
     (Bioconda lookup, bioSkills, pipeline graph), and the provider
     fallback chain (the caller's own provider → configured provider →
     env-discovered Claude → OpenAI → Ollama)
  2. Requests fail fast with a structured `AI_NOT_CONFIGURED` error when
     no provider is usable
  2. Template names enter the prompt as hints; keyword matching remains a
     deterministic fallback when all providers fail
  3. Every draft is validated by the engine via the web workflow service;
     validation errors feed the orchestrator's correction loop (bounded)
Output: { pipeline_id, toml_content, explanation, alternatives, confidence }
```

### POST /api/ai/explain

Explain why a run failed and suggest fixes.

```
Input:  { run_id, language?: "zh"|"en" }
Output: { summary, root_cause, fix_suggestion }
```

Runs the same deterministic diagnosis engine behind
`/api/runs/{run_id}/diagnostics`, then augments it with a human-readable
explanation.

### POST /api/ai/interpret

Interpret run results (DEGs, variants, QC metrics).

```
Input:  { run_id, result_type: "deg"|"variants"|"qc" }
Output: { narrative, highlights, caveats, suggested_next }
```

Always includes caveats and limitations. Does NOT replace biologist judgment.

### POST /api/ai/optimize

Suggest parameter optimizations for speed, cost, or sensitivity.

```
Input:  { pipeline_id, goal: "speed"|"cost"|"sensitivity" }
Output: { optimized_toml, changes, estimated: { time_saved, memory_reduction } }
```

## Provider Architecture

The AI layer uses an enum-based dispatch system:

```
DeepSeek (default) → Claude (Anthropic) → OpenAI → Ollama (local) → Template keyword match
```

**Fallback chain**: DeepSeek is the default provider. If the primary provider
is unavailable, falls back to Claude. If Claude is unavailable, falls back to
OpenAI. If OpenAI is unavailable, falls back to local Ollama. If all AI
providers are unavailable, the request fails with an error suggesting the
best-matching template name — no pipeline is produced without AI.

**Request dedup**: Same intent + same data characteristics → cached result,
avoiding redundant API calls.

## Trust Boundary (Hard Constraint)

The AI service layer has:

| Operation | Allowed? |
|-----------|----------|
| Read pipeline from DB | ✅ |
| Call deterministic API endpoints | ✅ |
| Generate .oxoflow TOML text | ✅ |
| Write to database | ❌ |
| Spawn processes | ❌ |
| Delete files | ❌ |
| Modify pipelines directly | ❌ |
| Start execution without confirmation | ❌ |

The AI service is **zero-write, zero-execute**. It can only propose changes that
the deterministic core API implements after human confirmation.

## Configuration

```bash
# Set AI provider
export OXO_FLOW_AI_PROVIDER=claude    # claude | openai | deepseek | ollama
export OXO_FLOW_AI_API_KEY=sk-<YOUR-KEY>

# Or via API
POST /api/ai/config { "provider": "claude", "api_key": "..." }
GET  /api/ai/config  → { provider, model, api_url, is_configured }
POST /api/ai/test    → { success, message, provider, model }
```

## Non-AI Intelligence (Deterministic)

These functions look like AI but are 100% rule-based and deterministic:

| Function | Method | Why Not AI |
|----------|--------|-----------|
| File format detection | Extension matching (+ paired-end naming) | Deterministic |
| Reference genome discovery | File existence in fixed search dirs | Deterministic |
| Pipeline template matching | Keyword scoring | Reproducible |
| Failure classification | Error patterns + exit codes | Rule-based |
| DAG optimization | Topological sort + critical path | Math problem |
