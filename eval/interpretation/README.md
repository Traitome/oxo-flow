# Interpretation Benchmark: `report --ai` on Seeded Failures (issue #359)

Measures how well `report --ai` **interprets** a run — a different capability
from generating a correct pipeline, which the gold-set (`eval/`) and frontier
(`eval/frontier/`) benchmarks already cover. Nothing in the existing eval
system exercises the analysis surface; this benchmark closes that gap.

Same philosophy as the rest of `eval/`: **deterministic judging, never an
LLM-as-judge**. Ground truth comes from faults we plant ourselves; scoring is
fixed regexes over the model's interpretation text.

## Method

For every seed the runner:

1. materializes a fresh workdir from `seeds/<id>/` (workflow + auxiliary files
   declared in `manifest.json`),
2. runs `oxo-flow run workflow.oxoflow` for real and checks the exit code
   against the manifest (`run_expect_exit`) — a mismatch marks the seed
   *broken*, never *failed*,
3. runs `oxo-flow report workflow.oxoflow --ai [--failed] --format md` and
   captures the interpretation prose (stderr),
4. scores the prose: **hit** = any `expect_any` regex (case-insensitive)
   matches and no `forbid` regex matches.

The seed corpus uses the `system` backend and plain shell/python so the runs
are dependency-free, fast, and deterministic on any machine.

## Seed design rules

Lessons already paid for during prototyping — do not regress them:

- **Neutral naming.** Workflow and rule names must not hint at the fault.
  A seed named `missing-input` lets the model guess the cause from the name
  alone (observed in the first prototype), which defeats the measurement.
- **One fault per seed.** Input files that exist, references that exist,
  tools absent only where the fault requires it.
- **Ground truth must name specifics.** Scoring regexes reward *naming the
  actual cause* (the missing filename, the absent binary, the exception);
  generic "check your inputs" boilerplate must not score. The v1 calibration
  run demonstrated why: loose patterns awarded 2 false-positive hits to
  boilerplate guesses before being tightened (0.6 → honest 0.2).

## Scoring caveats

Regex scoring measures cause-identification, not depth of reasoning. It is a
floor, not a ceiling: an interpretation can pass by quoting the decisive line
and still be shallow. Human review of `ai.txt` artifacts supplements the
number (they are retained per run under `results/<stamp>/<seed>/`).

## Usage

```bash
# provider env first, e.g.:
export OXO_FLOW_AI_PROVIDER=claude ANTHROPIC_BASE_URL=… ANTHROPIC_AUTH_TOKEN=… ANTHROPIC_MODEL=…
python3 run_bench.py                     # all seeds
python3 run_bench.py --seeds s01,s04     # subset
python3 run_bench.py --stamp my-label    # named results dir
```

## Surfaces

v1 covers `report --ai` (the post-execution interpretation surface), both
failed and successful checkpoints. `dry-run --ai` (pre-execution workflow
analysis) is a planned second surface; several seeds (e.g. charset-gate
refusals) only make sense there and will join with it.

## Results

| stamp | binary | hit-rate | note |
|---|---|---|---|
| `baseline-tight` | 0.18.1 (pre-fix) | 0.2 (1/5) | only the success anchor passed; the interpreter's prompt carried no stderr — the model itself reported "no error message included" and guessed |
| `post-fix` | prompt-fix build | 1.0 (5/5) | `report --ai` now receives per-rule exit statuses + stderr tails and quotes them |

The pre-fix prompt gap is itself the benchmark's first finding: a failed run
was interpreted from rule counts and timings alone. The fix
(`build_interpretation_prompt` in `crates/oxo-flow-cli/src/commands/output.rs`)
feeds each rule's status, exit code, and a bounded stderr tail, and instructs
the model to quote the decisive line and surface stderr warnings hidden
behind success statuses (the `s04` silent-failure seed).

The `s04` class deserves emphasis: `zcat X | wc -l > out` exits 0 with a
poisoned output when `X` is missing. Post-fix, the interpretation explicitly
flags the success status as misleading — exactly the behavior the seeded
method was designed to measure.

## Limitations

- 5 seeds, single provider, macOS arm64 — quote with the same caution as the
  frontier benchmark's small intent set.
- Regexes are calibrated to the current seed corpus; new seeds must keep the
  "name the specific cause" bar.
- `forbid` patterns (negative controls) are defined but empty in v1 seeds.
