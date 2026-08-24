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

`oxo-flow ai` reports how fresh the four embedded knowledge sources are
(tool reference, Bioconda database, bioSkills library, pipeline graph) in a
**Knowledge freshness** section: per-source record count, generation date,
staleness in days, and whether the source is auto-updated (`auto`) or
manually curated (`manual`):

```text
Knowledge freshness:
  bioconda_tools (auto) 6132 records, generated 2026-08-22 (1 day ago)
  skills_index (auto) 562 records, generated 2026-08-22 (1 day ago)
  pipeline_graph (auto) 548 records, generated 2026-08-22 (1 day ago)
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
