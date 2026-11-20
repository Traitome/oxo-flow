# Pilot findings — DeepSeek v4-flash (Anthropic-compatible endpoint)

> Note (2026-10-28, #948 P2-4; updated for #1315): rows labelled `minimal`
> below are the arm now named **`unconstrained`** (Arm A), and rows
> labelled `cli` are the arm now named **`syntax-only`** (Arm B). At pilot
> time `minimal` still claimed to replicate the web chat prompt; since the
> #342 unification no product path uses that prompt, so the arm is a bare
> unconstrained baseline. These pilot rows predate the pre-registered
> Arm C (`full-contract`) variant, so no Arm C row appears here. The
> historical results stand as measured.

First run of the frontier benchmark, 2026-09-09. 9 intents (3 easy /
3 medium / 3 hard), one seed per cell, `deepseek-v4-flash-vision-exp`
via `https://api.deepseek.com/anthropic` (the same wire protocol Claude
Code uses). Machine-local artifacts under `results/` (gitignored).

## Headline

| cell | runs | artifacts | all-gates pass | tokens in/out | est. cost* |
|---|---|---|---|---|---|
| `minimal` (weak embedded prompt) — the `unconstrained` arm (Arm A) under its pre-#948 name | 9 | 8 | **0/9** | 2,382 / 51,447 | ~$0.057 |
| `cli` (Arm B, now `syntax-only`) @ default (max_tokens 4096, timeout 120s) | 9 | **0** | 0/9 | n/a (sessions lost — pre-fix) | wasted |
| `cli` @ raised (16384 / 300s); Arm B | 9 | 4 | **3/9** | ~79k / ~65k (two batches) | ~$0.094 |

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
   retry) is a retry-handling gap in the orchestrator that P3 resolves by
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

The shipped default `[ai] max_retries` moved 3 → 6 in this round (raised
again to 10 by the model-axis round, #353) after the spot-check
below showed the thinking tier starving at the old default: with 2–3
rounds the model spends its whole budget on knowledge-tool lookups
before writing any TOML, and the run dies at the round cap with the
session archived (paid, invisible in artifacts). `run_eval.py` no longer
passes `--ai-max-retries` unless overridden, so bare runs measure
whatever the binary actually ships.

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

## Scientist Team ablation (compact vs full profile)

The full profile adds three roles around the generation agent: a bounded
task contract, a deterministic Curator brief (embedded-knowledge retrieval),
and an independent review whose request_changes triggers exactly one
regeneration. Ablated on the expanded intent set — 29 intents (26 specified
+ 3 deliberately vague), 2 seeds, `deepseek-chat`, ceilings 8192 / 180–300s,
58 runs per arm:

| arm | all-gates (specified) | easy | medium | hard | vague | tokens | est. cost |
|---|---|---|---|---|---|---|---|
| compact (baseline) | **43/52 (83%)** | 8/8 | 18/20 | 17/24 | 5/6 | 346k | $0.229 |
| full, v1 (review replaced unconditionally) | 29/52 (56%) | 5/8 | 10/20 | 14/24 | 5/6 | 369k | $0.271 |
| full, v2 (review invariant + calibration + exact-name curator) | 39/52 (75%) | 7/8 | 15/20 | 17/24 | 5/6 | 389k | $0.242 |

Wall time: compact mean 14 s/run; full v1 mean 112 s/run (review demanded
changes on 56/58 runs — a 97% trigger rate — so nearly every run paid a
third generation); full v2 mean 17 s/run (trigger rate 7%, all four
regenerations validated and were adopted).

### What the iterations established

1. **Review must be unable to regress the outcome.** v1 replaced the
   artifact whenever the reviewer demanded changes; because regeneration is
   a fresh draw, gate-passing drafts were swapped for unvalidated ones
   (83% → 56%). v2 adopts a regenerated artifact only when it ALSO passes
   the engine validator — review can now only help or waste its own tokens,
   never degrade the result. This invariant is load-bearing.
2. **The reviewer must approve by default.** An "adversarial" framing made
   `deepseek-chat` demand changes on 97% of artifacts. Recalibrated to a
   closed list of blocking classes with an explicit approve-by-default rule,
   the trigger rate fell to 7%.
3. **The Curator brief must not guess.** Injecting fuzzy Bioconda matches
   anchored the generator on wrong tools (a never-reviewed run still lost
   pass@1). Exact-name hits only.
4. **On this model tier the roles do not pay for themselves in aggregate.**
   v2 rescues hard-tier cells (metag-assembly 0/2→2/2, cnv-cnvkit,
   chipseq-broad, somatic-mutect2 each +1) but destabilizes an equal number
   of previously-solid easy/medium cells; the net −4 cells is within 2-seed
   noise, at +1% cost / +12% tokens.

Conclusion: **compact stays the shipped default**; full remains available
(`--ai-team-profile full`) as the substrate for the next levers — review
scoped to FAILED generations (rescue-only, where it has upside without
destabilizing passes), a larger intent set, and more seeds before any
tier-conditional recommendation.

## Gate-reliability round: deterministic repair + first-class package lists

Dissecting the compact baseline's 9 failures (58-run arm above) showed they
are NOT random: two mechanical classes dominate — `E016` (multi-package
inline conda specs joined by spaces/commas, which the schema rejected) and
`E005` (`{config.x}` references without a `[config]` declaration). Worse,
the inline package form was accepted by `validate` but **failed at run
time** ("refusing to run the tool from PATH") — gate-valid was not
runnable. Three fixes landed and the identical arm re-measured:

| arm | all-gates | success (gates ∧ fidelity) | easy | medium | hard | vague | cost |
|---|---|---|---|---|---|---|---|
| compact, pre-fix | 48/58 (83%) | n/a | 10/10 | 21/24 | 17/24 | 5/6 | $0.25 |
| compact, post-fix | **57/58 (98%)** | **56/58 (97%)** | 10/10 | 24/24 | 22/24 | 6/6 | $0.26 |

The fixes, in the order that matters:

1. **Engine: inline conda package lists are first-class** —
   `conda = "bioconda::fastp=0.23.4 bioconda::samtools=1.24"` parses as an
   allowlist-validated package list, derives a content-addressed env name
   (`fastp-<hash8>`), and installs via direct `conda create`. This is the
   exact form the generation prompt mandates; making it executable is an
   engine improvement for every user, not a benchmark patch. Injection
   attempts still fail the charset allowlist and the hard-error path.
2. **Deterministic E005 fixer** (injected as the generation agent's text
   fixer): missing config keys are declared under `[config]` in code, so
   the mechanical class never reaches a paid model round. The
   orchestrator validates and adopts the fixed text.
3. **Fidelity criterion** (benchmark): each intent names its required
   tools; a run now succeeds only with gates ∧ fidelity — a gate-valid
   pipeline that never mentions the requested tools no longer counts.

Both residuals were then closed in the same round:
- the fidelity checker gained `|` alternates (`"picard|gatk"` — GATK4
  absorbed the Picard tools, so a `gatk` MarkDuplicates satisfies a
  picard requirement), fixing the mutect2 false positive;
- the last gate failure was a THIRD natural spec variant, semicolon-joined
  packages (`bioconda::a=1;bioconda::b=2`), now also first-class — with an
  argv-safety rule that tokens may not start with `-`, closing conda-CLI
  flag injection (`--override-channels -c <attacker>`) that a naive
  separator widening would have allowed.

Final measurement (full 58-run arm, deepseek-chat, ceilings 8192/180s,
success = gates ∧ fidelity, `--ai-attempts 2` available):

| scope | success |
|---|---|
| **specified intents** | **52/52 (100%)** |
| vague intents (deliberately under-specified) | 5/6 |
| **all runs** | **57/58 (98.3%)** |

Attempt 2 was never needed (0/58 runs required a second draw — the
deterministic repair + first-class package lists removed the failure
classes before retries could pay), so the pass@k safety net cost nothing.
The single remaining failure is one seed of a DELIBERATELY vague intent —
the cell class the report isolates from the specified tiers precisely
because its bar is not comparable.

What this round establishes about reaching ~100% scientifically: the gates
are deterministic, so the failure set decomposes; every failure class
either gets a deterministic repair (E005 fixer, package-list grammar) or
an engine capability (executable inline specs), and the multi-seed
benchmark with the fidelity criterion verifies the fixes generalize
rather than overfit. The residual is confined to the vague tier, where
the honest lever is the task contract / clarification design, not more
retries.

## Full user-lifecycle validation (post-fix)

A simulated user ran the complete operational lifecycle against an
AI-generated multi-package workflow (fastqc + fastp + samtools + multiqc,
fastp rule declaring `conda = "bioconda::fastp=1.3.6 bioconda::samtools=1.24"`):

1. generate (`template --ai`) → validate/lint pass
2. real `run` → conda bootstrap executed — and FAILED in the solver:
   `libdeflate ... does not exist (perhaps a missing channel)`. The inline
   form carries only `bioconda::` per-package qualifiers, but bioconda's
   transitive dependencies live on **conda-forge**; a YAML spec would have
   declared that in its channels block, and the inline form had no
   equivalent. Fix: the direct-create command now renders
   `-c conda-forge` (runtime + container export), standing in for the
   channels block.
3. re-run → env solved and created, tools executed; the rule then failed
   on missing input data (the AI used placeholder sample names) — surfaced
   cleanly with the tool's own error, checkpoint recording the failure.
4. user adds valid data, re-runs → **4/4 rules succeed** (fastqc, fastp,
   fastqc_trimmed, multiqc, all inside their conda envs).
5. re-run → 4 skipped (checkpoint); append a read to one input → the
   invalidation set re-runs exactly the affected chain (4/4 again).
6. `status` (Completed: 4 / Failed: 0), `clean --dry-run` (13 outputs),
   `clean --force` (13 deleted).
7. Web regression on the same build: translate + chat produce gate-valid
   artifacts; AI_NOT_CONFIGURED and boundary guards unchanged.

The lifecycle found and fixed one more engine gap (missing dependency
channel) that no gate could see — gates check the recipe, only a real run
exercises the kitchen.

## Verified-repair campaign (pending live run)

The fault-injection harness (`fault_injection.py`, issue #1164) now
MEASURES the repair-application, verification, and verified-rerun stages
instead of declaring them false: per case it drives the real CLI
(`oxo-flow run workflow_ai.oxoflow --ai-recover --ai-max-retries 1` with
`[ai] auto_fix = "always"`), parses the engine's `REPAIR_APPLIED` /
`REPAIR_VERIFIED` narration prefixes and the checkpoint
`repair_record.applied` / `.verified` (with a legacy fallback to the
pre-#1164 "Fix applied to" narration), re-runs the (possibly patched)
workflow, and records the exact WHY for every false stage. Repair quality
is judged by rerun success only — NOT biological correctness (stubs, not
real tools, produce the outputs).

Dry-mode plumbing campaign (no provider; measured 2026-11-18, 10/10 cases
executable-repair verified, dry runs byte-identical across repeats —
deterministic; repair stages honestly false with "provider not configured"):

| case | static | attributed | proposed | executability | fault manifested | applied | rerun ok |
|---|---|---|---|---|---|---|---|
| bio_sort_order_mismatch | ✓ | ✓ | ✓ | ✓ | ✓ | ✗ | ✗ |
| bio_missing_companion_index | ✓ | ✓ | ✓ | ✓ | ✓ | ✗ | ✗ |
| bio_reference_build_conflict | ✓ | ✓ | ✓ | ✓ | ✓ | ✗ | ✗ |
| bio_compression_mismatch | ✓ | ✓ | ✓ | ✓ | ✓ | ✗ | ✗ |
| sci_vqsr_small_cohort | ✓ | ✗ | ✗ | ✓ | ✗ | ✗ | ✗ |
| sci_featurecounts_no_strand | ✓ | ✗ | ✗ | ✓ | ✗ | ✗ | ✗ |
| sci_mutect2_tumor_only | ✓ | ✗ | ✗ | ✓ | ✗ | ✗ | ✗ |
| infra_oom_failure | ✗ | ✓ | ✓ | ✓ | ✓ | ✗ | ✗ |
| infra_timeout_failure | ✗ | ✓ | ✓ | ✓ | ✓ | ✗ | ✗ |
| infra_disk_pressure_enospc | ✗ | ✓ | ✓ | ✓ | ✓ | ✗ | ✗ |

Infrastructure faults are runtime-only (no static phase exists for a
stderr OOM that has not happened yet), so their `static` column is false
by design and their runtime attribution is what the gate checks. The dry
`why` column for every repair stage reads "provider not configured — AI
recovery cannot run" (BioTypes/infra) or "seeded run exited 0 — the SCI
preflight finding is advisory, so no runtime failure exists for AI
recovery to engage" (preflight). No live numbers are claimed here.

### Live campaign command (orchestrator runs this)

The provider is read from the environment, never hardcoded; values below
are placeholders the orchestrator substitutes:

```bash
OXO_FLOW_AI_PROVIDER=openai \
OPENAI_BASE_URL=<provider-endpoint> \
OPENAI_API_KEY=<provider-key> \
OPENAI_MODEL=<model> \
OXO_FLOW_AI_TIMEOUT_SECS=300 \
OXO_FLOW_AI_MAX_TOKENS=16384 \
python3 eval/frontier/fault_injection.py \
  --binary target/release/oxo-flow \
  --out-json eval/frontier/results/fault_repair_logs/live_summary.json
```

Expected cost: at most one recovery round per case (10 cases, `--ai-max-retries 1`).
Results land in the JSON plus per-case raw logs under
`eval/frontier/results/fault_repair_logs/<fault_id>/`; the measured table
goes HERE once the live run completes — this section stays a placeholder
until then.

## Verified-repair campaign — MEASURED (GLM via school proxy, 2026-11-18)

Binary: `target/release/oxo-flow` @ main after #1185/#1186 (v0.32.0 + the
verified-repair loop). Two rounds:

**Round 1 — engine loop, no fence corrective:** detection 7/10,
executability 10/10, repair applied **0/10**, verified rerun **0/10**.
Every applied=false carried the same measured WHY: all three repairable
infra faults produced CORRECT diagnoses flagged `Safe to auto-apply: yes`,
but the replies narrated the fix in prose without emitting the corrected
workflow inside a ```toml fence — `modified_toml` stayed empty and the
authorization layer never ran. Raw logs per case pin this
(`fault_repair_logs/infra_*/fault_run.txt`).

**Round 2 — with the corrective fence round (#1187):** repair applied
**3/10**, verified rerun **3/10** — 3/3 of the repairable class closes the
full loop (diagnosis → safe verdict → patch written → engine validation →
bounded verification rerun → `REPAIR_RERUN_SUCCEEDED` → exit 0 →
checkpoint `repair_record`). Evidence per case: narration tokens + the
checkpoint record + raw logs (`infra_oom_failure` excerpt: `⚠ proposal
carried no fence` → `REPAIR_APPLIED session_id=…` → `REPAIR_RERUN_SUCCEEDED
…` → `✓ repair verified — rerun succeeded`).

**The seven non-repairable cases are honestly blocked upstream, not
failures of the loop** (per-case WHY in the JSON):

- 4 BioTypes faults abort at the type-admission gate BEFORE any rule
  executes (`admission_abort`). That gate was OUTSIDE the recovery loop
  when these rounds ran; issue #1265 routes the type-admission bail
  through the same verified-repair machinery (diagnose → authorized patch
  → in-process re-admission), so the `admission_abort` class is now
  repair-eligible. This campaign has not been re-measured against a
  binary carrying #1265 — the numbers above are the pre-#1265 baseline.
- 3 SCI preflight findings are advisory by design (warn, never abort):
  the seeded workflow runs successfully, so no runtime fault exists to
  repair.

Cost: one diagnosis round + at most one corrective round per case
(10 cases, `--ai-max-retries 1`). Summary JSONs: campaign round 1/2
exported alongside the raw logs.
