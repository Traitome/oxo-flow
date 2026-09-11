# AI Generation Frontier Benchmark (issue #342 / proposal P6)

Measures the **quality/cost frontier** of the AI pipeline-generation paths
against the engine's three deterministic static gates. Success is judged
**only** by the gates — never by an LLM judge — so numbers are reproducible
and cheap.

This complements the gold-set evaluation in [`eval/`](../README.md) and
[docs/guide/src/reference/ai-eval.md](https://traitome.github.io/oxo-flow/latest/reference/ai-eval/):
that system scores coverage against reviewed gold rows on a single
generation path; this one compares **paths and models** (quality per token
spent) on a tiered intent set, which is the evidence base for the #342
unification proposal and its TeamProfile tiers.

## Method

| | variants |
|---|---|
| `cli` | full CLI path: `oxo-flow template --ai` — rich prompt + knowledge tools + tool-calling loop (the "strong" path) |
| `minimal` | faithful replication of the weakest embedded path (web chat `process_chat` prompt: 7 generic rules, no tools, single shot) (the "floor") |

Gates (exit 0 = pass, machine-readable JSON captured per run):

1. `oxo-flow validate --json` — structure / DAG / wildcard syntax
2. `oxo-flow dry-run --json` — expansion preview, resource audit
3. `oxo-flow lint --json` — best practices

Token accounting: `cli` reads the AI session archive
(`~/.oxo-flow/ai_sessions/*-template-*.json`); `minimal` reads the provider
response usage. Cost is derived from the price table in `run_eval.py`
(defaults mirror the repo constant in `oxo-flow-ai/src/types.rs`).

## Usage

```bash
# credentials: ANTHROPIC_BASE_URL / ANTHROPIC_AUTH_TOKEN / ANTHROPIC_MODEL
# from env, or parsed silently from ~/.zshrc (never printed)

python3 run_eval.py                          # 29 intents x minimal,cli
python3 run_eval.py --filter rnaseq-star     # subset
python3 run_eval.py --models deepseek-chat   # model axis (quality/cost frontier)
python3 run_eval.py --variants minimal       # cheapest full sweep
python3 run_eval.py --seeds 2                # 2 seeds per cell (variance)
python3 run_eval.py --profile full           # Scientist Team roles (compact is default)
python3 run_eval.py --binary /path/oxo-flow  # pin the binary for a whole campaign
```

The intent set spans 10 domains across three complexity tiers, plus three
deliberately under-specified intents (`"vague": true`) used to measure how
well a profile standardizes ambiguous requests. Vague cells are reported
on their own row and are never compared against the specified tiers.

Thinking-style backends (e.g. DeepSeek behind an Anthropic-compatible
endpoint) need a raised output budget: export
`OXO_FLOW_AI_MAX_TOKENS=16384` or the loop truncates before the TOML is
written (the default 4096 is consumed by reasoning blocks).

Results land in `results/<stamp>/` (gitignored): one directory per run
(generated workflow, gate JSON/stderr, CLI log, session snapshot), plus
`results.json` (machine-readable) and `report.md` (aggregated pass@1,
tokens, cost, latency).

## Interpreting

- `all-gates pass@1` is the headline quality metric (deterministic).
- `minimal` vs `cli` quantifies what the rich prompt + tool loop buy —
  the core empirical claim behind unifying the four generation paths (#342).
- The `model` axis gives a quality/cost frontier for the `TeamProfile`
  tiers (Full vs Compact) proposed in #342.
- Gate failures carry structured error codes (e.g. `E017` unknown key),
  which classify failure modes without any LLM involvement.

## Known limitations

- Intent set is 29 (3 of them deliberately vague); still small enough that
  single-seed absolute numbers wobble — quote the multi-seed aggregates.
- Since the #342 unification, `--ai-max-retries` sizes the orchestrator's
  combined tool + correction-round budget, and engine validation feeds the
  loop; numbers above the unification marker in PILOT_FINDINGS.md were
  produced by the pre-unification paths.
- `--profile full` measures the Scientist Team roles (contract, curator,
  review); each role's marginal value is only interpretable against the
  compact profile on the SAME model, seeds, and ceilings.
