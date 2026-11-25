# AI synthesis three-arm ablation — non-governed operational run (2026-11-26)

Status: **operational evidence only**, not the pre-registered governed campaign.
It executes the existing #1315 harness (`eval/frontier/run_eval.py`) over the full
36-intent set with the three arm definitions from `eval/frontier/README.md`:
Arm A `unconstrained`, Arm B `syntax-only`, Arm C `full-contract`
(`--ai-contract-feedback`). Judging is the engine's three deterministic gates
(validate / dry-run / lint) plus named-tool fidelity; no LLM judge was used.

## Campaign manifest

| Field | Value |
| --- | --- |
| Date | 2026-11-26 (run started 2026-11-25 23:37 local) |
| Binary | `target/debug/oxo-flow` at main `131d6f0e` (the harness binary is pinned in the run manifest) |
| Model | `qwen3.8-flash-next-nothink` via an internal Anthropic-compatible proxy (placeholder `<internal-proxy>`; host/key not committed) |
| Intents | 36 (5 easy / 14 medium / 17 hard, including 3 deliberately vague; vague intents are also counted in their base tier by the harness) |
| Arms | `unconstrained` (A), `syntax-only` (B), `full-contract` (C) |
| Seeds | 1 |
| Attempts | 1 (`--ai-attempts` default) |
| Temperature | provider default (not pinned by the harness for B/C) |
| Command | `python3 eval/frontier/run_eval.py --variants unconstrained,syntax-only,full-contract --models qwen3.8-flash-next-nothink --binary <binary> --timeout 600 --sleep 2` |

Environment: `OXO_FLOW_AI_TIMEOUT_SECS=300`, `OXO_FLOW_AI_MAX_TOKENS=16384`,
`OXO_FLOW_AI_CONTEXT_TOKENS=131072`; proxy keys were passed only through the
environment. Machine-readable sanitized results:
`ai-synthesis-ablation-2026-11-26-results.json` (108 runs, `workflow_file`
paths stripped).

## Results (success = all three gates pass AND fidelity passes)

| Arm | Generated | All-gates | Success | pass@1 | Tokens in / out | Est. cost | Wall mean |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| A `unconstrained` | 34/36 | 0/36 | **0/36** | 0% | 7,934 / 65,267 | $0.0740 | 38.2 s |
| B `syntax-only` | 29/36 | 28/36 | **28/36** | 77.8% | 2,636,002 / 131,551 | $0.8828 | 96.7 s |
| C `full-contract` | 23/36 | 19/36 | **18/36** | 50.0% | 3,398,183 / 153,093 | $1.1199 | 103.5 s |

| Tier | A | B | C |
| --- | ---: | ---: | ---: |
| easy | 0/5 | 5/5 | 5/5 |
| medium | 0/14 | 12/14 | 5/14 |
| hard | 0/17 | 11/17 | 8/17 |
| vague (also in base tier) | 0/3 | 3/3 | 2/3 |

## Paired tests (exact McNemar, two-sided, success outcome)

| Comparison | Discordant (first-only / second-only) | n | p |
| --- | ---: | ---: | ---: |
| A vs B | 0 / 28 | 28 | < 0.0001 |
| B vs C | 12 / 2 | 14 | **0.0129** |
| A vs C | 0 / 18 | 18 | < 0.0001 |

B beats A decisively: the engine-accurate prompt + tool loop + syntax/structural
feedback is what turns naive generation into gate-passing workflows.

**C did not beat B; on this model it was significantly worse.** C passed 18/36
vs B 28/36, with 12 intents that B passed and C failed against 2 the other way
(`metag-assembly`, `spatial-visium`). C cost ~27% more than B and ran ~7%
slower; neither difference is the headline — the 10-intent success gap and the
paired test are.

## Failure pattern (descriptive, not causal)

C's extra failures are dominated by very short outputs (`completion_tokens`
around 500–800) followed by `template exited 1`, with the harness reporting
missing expected tools. The contract-feedback loop asks the model to react to
BioTypes/SCI/deep-check findings; on this no-think backend the observed effect
was more frequent early termination rather than repair. Examples where B passed
but C failed include `rnaseq-hisat2`, `rnaseq-de-deseq2`, `chipseq-broad`,
`amplicon-dada2`, `single-cell-multiome`, `metagenome-humann-pathways`,
`rnaseq-star-salmon-quant`, and `variant-vague`. Both arms still failed
`cnv-cnvkit`, `variant-annotate`, `atacseq`, `wgbs-bismark`, `gatk-cohort` and
`cutandtag-epigenomics`; those intents remain hard for this model regardless of
the contract loop. The two C-only wins show the loop is not uniformly harmful;
the net effect is negative here.

## Interpretation and limits

- This is **one model, one provider/proxy, one seed, 36 intents, default
  temperature, deterministic gate judging**. It does not estimate variance, and
  it is not the governed pre-registered campaign. No POSITIVE/NEGATIVE
  publication decision is claimed.
- The pre-registered campaign still needs the PR-01/PR-02/PR-03 amendments
  (repeat count R, run-order randomization, temperature policy), multiple
  models/seeds, and a pre-registered analysis. The harness is ready; this run
  is operational evidence that the instrument works end-to-end on all 108 cells.
- The negative C-vs-B result should be investigated before changing product
  defaults: options include model-specific feedback budgets/tool policy,
  detecting early-termination loops, or requiring a stronger thinking model for
  Arm C. Do not generalize the result to other models or to the Scientist Team
  `--profile full` (which is a different axis, not Arm C).
- Estimated cost uses the repo's static price table; it is not a billing
  receipt. Raw generated workflows are not committed (size/latent content);
  the sanitized run manifest is committed beside this report.
- The harness reported 108/108 cells completed; no paid calls are made in CI.
  Reproduce with the operator's proxy credentials only, outside CI.

## Evidence / links

- Harness: `eval/frontier/README.md`, `run_eval.py`, `statistical_report.py`;
  issue #1315 (instrument), issue #1395 (this campaign).
- Sanitized results: `docs/superpowers/audits/data/ai-synthesis-ablation-2026-11-26-results.json`.
- The governed exit criteria remain in ROADMAP Stage 12.4 and the Vision page.
