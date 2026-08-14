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
