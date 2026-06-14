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
Process:
  1. AI parses intent → structured intent
  2. Calls /api/data/analyze for data characteristics
  3. Matches template library → selects best template
  4. Generates concrete parameters → full .oxoflow config
  5. Auto-calls /api/pipelines/validate for verification
  6. If invalid → correct and re-validate, max 3 rounds
Output: { pipeline_id, toml_content, explanation, alternatives, confidence }
```

### POST /api/ai/explain

Explain why a run failed and suggest fixes.

```
Input:  { run_id, language?: "zh"|"en" }
Output: { summary, root_cause, fix_suggestion }
```

Calls `/api/runs/{run_id}/diagnostics` for deterministic diagnosis,
then augments with human-readable explanation.

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
Output: { optimized_toml, changes, estimated_impact }
```

## Provider Architecture

The AI layer uses an enum-based dispatch system:

```
Claude (Anthropic) → OpenAI → Ollama (local) → Template keyword match
```

**Fallback chain**: If Claude is unavailable, falls back to OpenAI. If OpenAI is
unavailable, falls back to local Ollama. If all AI providers are unavailable,
falls back to template keyword matching (deterministic).

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
export OXO_AI_PROVIDER=claude    # claude | openai | ollama | noop
export OXO_AI_API_KEY=sk-...

# Or via API
POST /api/ai/config { "provider": "claude", "api_key": "..." }
GET  /api/ai/config  → { provider, model, available }
POST /api/ai/test    → { ok: true, latency_ms: 234 }
```

## Non-AI Intelligence (Deterministic)

These functions look like AI but are 100% rule-based and deterministic:

| Function | Method | Why Not AI |
|----------|--------|-----------|
| File format detection | Magic bytes + extension | 100% accurate |
| Reference genome discovery | Path traversal + checksum | Deterministic |
| Pipeline template matching | Keyword scoring | Reproducible |
| Failure classification | Error patterns + exit codes | Rule-based |
| DAG optimization | Topological sort + critical path | Math problem |
