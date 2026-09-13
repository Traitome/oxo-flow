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

The benchmark covers both AI analysis surfaces:

- **`report --ai`** (post-execution): hard-failure seeds run with `--failed`,
  silent-failure and success seeds without. The interpretation text arrives
  on **stderr**.
- **`dry-run --ai`** (pre-execution): workflow-level faults scored from the
  audit output. The analysis text arrives on **stdout** (via `ai_check`) —
  scoring stdout and requiring the `Analysis Results` marker is how the
  runner detects a silently skipped AI block; without that check a preview
  echo can masquerade as a diagnosis hit (observed: the engine's E011 gate
  echoes the blocked `rm -rf` command into the preview, which the s06 regex
  matched before the stdout capture was fixed).

## Results

| stamp | binary | provider | hit-rate | note |
|---|---|---|---|---|
| `baseline-tight` | 0.18.1 (pre-fix) | GLM | 0.2 (1/5) | only the success anchor passed; the interpreter's prompt carried no stderr — the model itself reported "no error message included" and guessed |
| `post-fix` | prompt-fix build | GLM | 1.0 (5/5) | `report --ai` now receives per-rule exit statuses + stderr tails and quotes them |
| `glm-8seed-v2` | post-#364 | GLM | 1.0 (8/8) | both surfaces; runner stdout capture fixed (the first 8-seed pass scored preview stderr and produced a false 0.75 with an s06 false positive) |
| `deepseek-8seed` | post-#364 | DeepSeek | 1.0 (8/8) | second provider through the same Anthropic-compatible gateway — headline numbers hold across models |

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

- 8 seeds (5 report, 3 dry-run), two providers through one Anthropic-compatible
  gateway, macOS arm64 — quote with the same caution as the frontier
  benchmark's small intent set; single-run cells wobble.
- The dry-run surface receives the engine's deterministic preflight findings
  in its prompt, so a hit may partly reflect the engine's own detection being
  explained rather than an independent AI discovery. The audit prompt also
  drives independent findings (observed repeatedly: tool-knowledge catches
  beyond the planted fault, e.g. FastQC output-naming mismatches).
- Regexes are calibrated to the current seed corpus; new seeds must keep the
  "name the specific cause" bar.
- `forbid` patterns (negative controls) are defined but empty in all seeds.
