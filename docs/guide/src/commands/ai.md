# `oxo-flow ai`

AI status, self-test, setup, and three-layer workflow explanation.

```bash
oxo-flow ai                 # quick status: provider, model, quota
oxo-flow ai test            # comprehensive self-test (provider round-trip)
oxo-flow ai setup           # interactive provider configuration wizard
oxo-flow ai explain wf.oxoflow
```

## Options

| Option | Short | Description |
|---|---|---|
| `--step <RULE>` | — | Explain a single rule by name (with `explain`) |
| `--level <LEVEL>` | — | Explanation depth: `beginner` (default — jargon is defined) or `expert` (parameter-level, efficiency-focused) |
| `--json` | — | Machine-readable JSON output (with `explain`): the deterministic skeleton plus model-written prose fields |

## The three-layer explanation (`ai explain`)

1. **Overview** — what the workflow does, in plain language.
2. **Per-step detail** — every rule, its role, and how steps connect.
3. **Scientific review** — deterministic evidence-backed constraints
   (the same preflight rules `dry-run` prints) grounded to rule text, plus
   model-written prose. The grounding is deterministic — the model only
   writes prose around machine-derived facts.

## Degraded mode

`ai explain` never hard-fails on the model: when the provider is
unreachable, errors, or is explicitly disabled, it emits the
**deterministic grounding skeleton** (workflow identity, per-step
order/description/tools/inputs/outputs/resources, and the knowledge-base
grounding) with a note on stderr, and **exits 0**. The skeleton is the
same data the model would have received — verification data, not
hallucination — so scripts can rely on the JSON contract in every state.

- `OXO_FLOW_AI_PROVIDER=disabled` explicitly disables the model and
  overrides any saved provider configuration: the skeleton is emitted
  with a "disabled" note and exit 0.
- A failed provider call (dead endpoint, quota, timeout) degrades the
  same way, with the error explained in the note.

## Embedded knowledge freshness

`oxo-flow ai` reports how fresh the eight embedded knowledge sources are
(Bioconda, bio.tools, commercial tools, EDAM ontology, nf-core modules,
pipeline graph, skillgraph docs, bioSkills library) in a **Knowledge
freshness** section: per-source record count, generation date, staleness in
days, and whether the source is auto-updated (`auto`) or manually curated
(`manual`):

```text
Knowledge freshness:
  bioconda_tools (auto) 6487 records, generated 2026-09-01 (7 days)
  biotools_overlay (auto) 1925 records, generated 2026-08-23 (15 days)
  commercial_tools (auto) 25 records, generated 2026-09-01 (7 days)
  edam_terms (auto) 839 records, generated 2026-09-01 (7 days)
  nfcore_modules (auto) 2000 records, generated 2026-09-01 (7 days)
  pipeline_graph (auto) 543 records, generated 2026-09-01 (7 days)
  skillgraph_docs (auto) 77 records, generated 2026-09-01 (7 days)
  skills_index (auto) 562 records, generated 2026-09-01 (7 days)
```

- Auto-updated sources older than 60 days are flagged `STALE` — the same
  threshold the release pipeline's staleness gate enforces, so a shipped
  binary never embeds auto-updated knowledge older than 60 days.
- `lookup_tool` responses carry the same data date and record count as a
  freshness note, so agents can weigh how current the embedded database is.
- See "Embedded Knowledge Freshness" in the
  [AI CLI reference](https://traitome.github.io/oxo-flow/latest/reference/ai-cli/)
  for the update cadence, the update GitHub Action, and the gate details.

## The `--ai` flag on other commands

One flag, a different action per command — the context is the command
itself:

| Command | What `--ai` does |
|---|---|
| `run --ai-recover` | AI error recovery: on rule failure, the model analyzes stderr and proposes a fix (note: this one is `--ai-recover`, not `--ai`) |
| `dry-run --ai` | Plain-language analysis of the workflow (scientific preflight findings passed to the model) |
| `validate --ai` | Semantic validation beyond structure — scientific plausibility checks |
| `lint --ai` | Semantic linting on top of the deterministic best-practice rules |
| `debug --ai` | Plain-language explanation of a rule's expanded shell command |
| `report --ai` | AI result interpretation — a summary of execution outcomes, caveats, and next steps |
| `template --ai` | Generate a workflow from a natural-language description (optionally grounded in `--from-url`/`--from-file` reference material) |
| `env create --ai` | Generate a conda/pixi environment spec from a natural-language description |

All AI features go through the configured provider (see `oxo-flow ai
status`); deterministic behavior never depends on the model — AI output is
always additive prose or proposals, never silent engine decisions.

## How `template --ai` generates a workflow

`template --ai` runs the same generation agent as the web translate and
chat surfaces — one shared persona in `oxo-flow-ai`, one orchestrator
loop (issue #342). The steps:

1. **Resolve the destination** — `-o` is parsed before anything is sent
   to the provider, so a bad output path fails cheaply.
2. **Gather context** — `--from-url` references are fetched through the
   SSRF-screened fetcher; `--from-file` files are read with bounded
   previews. Both enter the prompt as external reference material.
3. **Plan** — the shared persona's system prompt encodes the `.oxoflow`
   schema (single-brace templates, `[[rules]]` arrays, `depends_on`,
   version-pinned conda packages) and explicitly bans the wrong dialects
   models tend to drift into (`foreach`, `inputs = {...}` maps,
   `[resources]` sections). Domain-matched bioSkills and any activated
   `[ai] skills` are injected as prompt sections.
4. **Ground with tools** — the model queries the embedded knowledge
   bases on demand: `lookup_tool` for exact Bioconda names/versions,
   `lookup_skill`/`lookup_pipeline` for domain procedures and topologies.
   Non-read-only tools (e.g. MCP tools from activated tool skills) ask
   for interactive approval on stderr before running; non-interactive
   sessions refuse them.
5. **Validate against the engine** — every draft is parsed by the core
   engine (`WorkflowConfig`). On failure the errors feed back to the
   model for correction; this loop is bounded by `--ai-max-retries`.
6. **Deliver** — the validated TOML is written, the three static gates
   (`validate` / `dry-run` / `lint`) can then confirm it, and the AI
   session (token usage, tool calls) is archived to
   `~/.oxo-flow/ai_sessions/`.

### Correction budget and failure behavior

- `--ai-max-retries N` sizes the orchestrator's combined tool +
  correction-round budget (default from `[ai]` config, 3 when unset).
- If the budget runs out after a pipeline was drafted, the command
  **degrades instead of discarding**: the generated TOML is extracted
  from the transcript and written with a prominent review warning —
  a paid-for artifact is never silently lost.
- Provider failures (auth, quota, network) fail fast with the error.

### Generation tuning knobs

| Variable | Default | When to change |
|---|---|---|
| `OXO_FLOW_AI_MAX_TOKENS` | `4096` | Thinking-style backends (e.g. DeepSeek behind an Anthropic-compatible endpoint) spend reasoning tokens against this ceiling and truncate before producing TOML — raise to 16384+ there. For non-thinking models the default holds with ≥3× headroom (measured: 1.9k–4.4k output tokens per generation) |
| `OXO_FLOW_AI_TIMEOUT_SECS` | `120` | Non-thinking generations complete in 12–20 s; thinking backends can exceed two minutes per call — raise together with `OXO_FLOW_AI_MAX_TOKENS`, or long completions die mid-body |

Both are consumed by the Anthropic Messages backend; the DeepSeek-native,
OpenAI-compatible, and Ollama backends have their own budgets.

Quality evidence for these calibrations (gate-scored benchmark,
before/after the generation unification) lives in `eval/frontier/` in
the repository. The architecture view of the shared harness — how the
CLI, translate, and chat surfaces differ only in provider resolution,
tool policy, and presentation — is in
[Architecture → AI Subsystem](https://traitome.github.io/oxo-flow/latest/reference/architecture/#ai-subsystem).
