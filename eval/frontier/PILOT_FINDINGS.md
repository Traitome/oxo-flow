# Pilot findings — DeepSeek v4-flash (Anthropic-compatible endpoint)

First run of the frontier benchmark, 2026-09-09. 9 intents (3 easy /
3 medium / 3 hard), one seed per cell, `deepseek-v4-flash-vision-exp`
via `https://api.deepseek.com/anthropic` (the same wire protocol Claude
Code uses). Machine-local artifacts under `results/` (gitignored).

## Headline

| cell | runs | artifacts | all-gates pass | tokens in/out | est. cost* |
|---|---|---|---|---|---|
| `minimal` (weak embedded prompt) | 9 | 8 | **0/9** | 2,382 / 51,447 | ~$0.057 |
| `cli` @ default (max_tokens 4096, timeout 120s) | 9 | **0** | 0/9 | n/a (sessions lost — pre-fix) | wasted |
| `cli` @ raised (16384 / 300s) | 9 | 4 | **3/9** | ~79k / ~65k (two batches) | ~$0.094 |

\* price table: $0.28/M in, $1.10/M out (repo constant, `oxo-flow-ai/src/types.rs`).

Both provider-side ceilings were found **by this benchmark** and are fixed
in the same PR: `OXO_FLOW_AI_MAX_TOKENS` (thinking blocks count against
`max_tokens`; 4096 truncates before any text) and
`OXO_FLOW_AI_TIMEOUT_SECS` (120s kills long completions mid-body).

## Failure taxonomy

- `minimal`, 8/9: generates **a wrong dialect** — Snakemake-style
  `[rules.x]` tables, `inputs = {...}` maps, `depends` keys. The gate
  rejects with structured `E017` (unknown key). The weakest embedded path
  does not merely underperform; it teaches the model a schema oxo-flow
  does not have. This is the quantified case for #342's P1 (one shared,
  engine-accurate generation persona).
- `minimal`, 1/9 (hard tier): hit the 8192 completion cap mid-fence → no
  extractable TOML at all.
- `cli` @ raised, 4/9: tool loop ends without text TOML ("did not contain
  valid .oxoflow TOML") — rounds burned on knowledge tools, and the
  tool-free final round has no retry. 1/9: contentless (thinking-only)
  response, no retry. 1/9 (`qc-fastqc`): valid-looking artifact using a
  Nextflow-ism (`foreach`) → `E017`; **the strong path also lacks a
  validation-feedback loop** (it warns on schema errors and writes
  anyway).
- 3/9 PASS with all three gates, including the hard-tier
  `metagenomics-kraken2`.

## Implications for #342

1. The cross-path quality gap is real and measurable (0/9 vs 3/9 on the
   same model) — unification P1–P4 is justified by data, not aesthetics.
2. The dominant CLI failure mode (tool-loop TOML miss + no final-round
   retry) is an orchestrator-robustness gap that P3 resolves by
   construction; a cheap interim fix is a bounded retry of the
   tool-free final round.
3. The `E017` class (wrong-dialect keys) is exactly what P2's gate tools
   + correction loop eliminate: the engine already returns structured,
   actionable codes.
4. Cost: even on a cheap backend, every failed path costs real tokens.
   Token accounting (session archives) must never be silently skipped —
   fixed here for the CLI failure paths.

Method and caveats: see README ("Known limitations" — small intent set,
single seed, tool-loop budget is not yet a correction loop).

## Post-unification re-measurement (PR: unified generation harness)

After wiring every surface onto `PipelineGenAgent` + orchestrator (the
#342 unification), the same 9 intents, same model, same ceilings
(16384 / 300s), `cli` variant:

| model | all-gates before → after | artifacts | tokens in/out (after) |
|---|---|---|---|
| `deepseek-chat` (non-thinking) | 6/9 → **8/9** | 9/9 | 32k / 23k (was 41k / 24k) |
| `deepseek-v4-flash-vision-exp` (thinking) | 3/9 → **6/9** | 8/9 (was 4/9) | 42k / 67k |

What the correction loop visibly repaired: the `qc-fastqc` Nextflow-ism
(`foreach` → E017) that the old path wrote out with a warning is now
caught and fixed in-loop; round-cap failures degrade to the transcript
artifact instead of vanishing with the session. Model guidance is
unchanged: for generation, the non-thinking tier is faster and cheaper
at higher gate-pass quality — the evidence base for #342's TeamProfile
cost tiers.

Residual misses (3/18 cells) are the remaining harness frontier:
correction-budget exhaustion on hard intents and one final-round TOML
miss — bounded-retry and evaluator roles are the next lever, not the
prompt.

## Post-default re-measurement (round-budget starvation fix)

The shipped default `[ai] max_retries` moved 3 → 6 after the spot-check
below showed the thinking tier starving at the old default: with 2–3
rounds the model spends its whole budget on knowledge-tool lookups
before writing any TOML, and the run dies at the round cap with the
session archived (paid, invisible in artifacts). `run_eval.py` now
defaults `--max-retries` to 6 so bare runs measure the shipped config.

Spot-check, same model and ceilings (16384 / 300s), `cli` variant:

| intent | budget | all-gates | wall | note |
|---|---|---|---|---|
| `qc-fastqc` (easy) | 6 | ✅ | 10.2s | |
| `rnaseq-star` (medium) | 2 | ❌ | 2.8s | round cap before any TOML |
| `rnaseq-star` (medium) | 6 | ✅ | 24.6s | 2 knowledge rounds + corrections fit |
| CJK two-intent fastp+multiqc | 3 | ❌ | — | round cap, session archived |
| CJK two-intent fastp+multiqc | 6 | ✅ | ~12s | gates re-run locally, exit 0 |

Non-thinking tiers are unaffected (deepseek-chat finished within 3
rounds on all 9 pilot intents); the change only widens the viable
config space for thinking backends, at zero cost when a draft
validates early.
