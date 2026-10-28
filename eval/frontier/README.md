# AI Generation Frontier Benchmark (issue #342 / proposal P6)

Measures the **quality/cost frontier** of the AI pipeline-generation paths
against the engine's three deterministic static gates. Success is judged
**only** by the gates — never by an LLM judge — so numbers are cheap and
the judging is deterministic; the naive-prompt arm is pinned to
temperature 0 where the provider honors it but is not fully reproducible
across providers or runs. The `cli` arm runs the real binary and inherits
the engine's `[ai] temperature` config (unset = provider default), so the
two arms may sample at different temperatures.

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
| `naive-prompt` | single-shot naive prompt (7 generic rules, no tools, no knowledge injection) — a prompt-axis baseline, not a product path |

Gates (exit 0 = pass, machine-readable JSON captured per run):

1. `oxo-flow validate --json` — structure / DAG / wildcard syntax
2. `oxo-flow dry-run --json` — expansion preview, resource audit
3. `oxo-flow lint --json` — best practices

Token accounting: `cli` reads the AI session archive
(`~/.oxo-flow/ai_sessions/*-template-*.json`); `naive-prompt` reads the
provider response usage. Cost is derived from the price table in
`run_eval.py` (defaults mirror the repo constant in `oxo-flow-ai/src/types.rs`).

## Usage

```bash
# credentials: ANTHROPIC_BASE_URL / ANTHROPIC_AUTH_TOKEN / ANTHROPIC_MODEL
# from env, or from ~/.oxo-flow/ai_config.json (never from shell rc files;
# values are never printed)

python3 run_eval.py                          # 29 intents x naive-prompt,cli
python3 run_eval.py --filter rnaseq-star     # subset
python3 run_eval.py --models deepseek-chat   # model axis (quality/cost frontier)
python3 run_eval.py --variants naive-prompt  # cheapest full sweep
python3 run_eval.py --seeds 2                # 2 seeds per cell (variance)
python3 run_eval.py --profile full           # Scientist Team roles (compact is default)
python3 run_eval.py --binary /path/oxo-flow  # pin the binary for a whole campaign
```

The intent set spans 10 domains across three complexity tiers, plus three
deliberately under-specified intents (`"vague": true`) used to measure how
well a profile standardizes ambiguous requests. Vague cells are reported
on their own row and are never compared against the specified tiers.

Thinking-style backends (e.g. DeepSeek behind an Anthropic-compatible
endpoint) truncate before the TOML is written if the output budget is too
small — reasoning blocks consume it. The default is 16384 (raised from the
old 4096, which thinking backends consumed entirely); raise it further only
for extreme reasoning lengths.

Results land in `results/<stamp>/` (gitignored): one directory per run
(generated workflow, gate JSON/stderr, CLI log, session snapshot), plus
`results.json` (machine-readable) and `report.md` (aggregated pass@1,
tokens, cost, latency).

## Interpreting

- `all-gates pass@1` is the headline quality metric (deterministic).
- `naive-prompt` vs `cli` quantifies what the rich prompt + tool loop buy
  over a bare prompt — the prompt/tool-loop axis behind the #342
  unification. (Historical note: this arm was named `minimal` and
  described as the web chat prompt before #948 P2-4; since #342 every
  surface, web chat included, runs the unified `PipelineGenAgent`, so no
  product path uses the naive prompt anymore — the rows in
  PILOT_FINDINGS.md labelled `minimal` are this arm under its old name.)
- The `model` axis gives a quality/cost frontier for the `TeamProfile`
  tiers (Full vs Compact) proposed in #342.
- Gate failures carry structured error codes (e.g. `E017` unknown key),
  which classify failure modes without any LLM involvement.

## Known limitations

- Intent set is 29 (3 of them deliberately vague); still small enough that
  single-seed absolute numbers wobble — quote the multi-seed aggregates.
- Judging is deterministic (the three gates + substring fidelity), but
  only the naive-prompt arm is pinned to temperature 0 — the `cli` arm
  inherits the engine's `[ai] temperature` (unset = provider default), and
  an Anthropic-compatible endpoint may still sample non-deterministically,
  so identical reruns can produce different workflows. Naive-prompt numbers
  in PILOT_FINDINGS.md were drawn at provider default temperature
  (pre-pinning), so they are not strictly comparable to post-pinning runs.
- Since the #342 unification, `--ai-max-retries` sizes the orchestrator's
  combined tool + correction-round budget, and engine validation feeds the
  loop; numbers above the unification marker in PILOT_FINDINGS.md were
  produced by the pre-unification paths.
- `--profile full` measures the Scientist Team roles (contract, curator,
  review); each role's marginal value is only interpretable against the
  compact profile on the SAME model, seeds, and ceilings.
