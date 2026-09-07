# Comparative Benchmarks (Phase 4)

This directory contains pipeline definitions equivalent to the oxo-flow
macro-benchmarks, written for Nextflow and Snakemake, plus a runner that
times all three engines on the same workload.

## Purpose

These definitions allow direct comparison between oxo-flow and other
workflow engines on equivalent workloads.  The metrics of interest are:

| Metric | Measurement | Tool |
|---|---|---|
| Startup overhead | Time to process 10 rules | `hyperfine --min-runs 10` |
| DAG construction | `validate` / `dry-run` time | `hyperfine` |
| Rule scaling | Time from 10 to 1000 rules | `suite.py` |
| Definition brevity | Lines of pipeline definition | `wc -l` |

## Usage

Requires: [Nextflow](https://www.nextflow.io/),
[Snakemake](https://snakemake.readthedocs.io/),
[hyperfine](https://github.com/sharkdp/hyperfine).
Missing engines are skipped with a warning (oxo-flow binary and hyperfine
are mandatory).

```bash
# Run all comparisons (default N=100 steps)
./benches/comparative/run_comparison.sh

# Custom chain length and oxo-flow binary
./benches/comparative/run_comparison.sh 1000 target/release/oxo-flow

# Individual runs
python3 -c "from benches.macro.suite import generate_hello; open('/tmp/wf.oxoflow', 'w').write(generate_hello(100))"
oxo-flow dry-run /tmp/wf.oxoflow
cd benches/comparative/nextflow   && nextflow run hello.nf --count 100
cd benches/comparative/snakemake  && snakemake --cores 1 --config count=100
```

## Pipeline Equivalence

All three pipelines produce an identical DAG: a sequential dependency
chain `step_0 -> step_1 -> ... -> step_{N-1}` where each step writes a
one-line file derived from its index (`echo {i} > step_{i}_output.txt`).

| Engine | File | Depth control |
|---|---|---|
| oxo-flow | `benches/macro/suite.py:generate_hello(N)` | generated N `[[rules]]` |
| Nextflow | `nextflow/hello.nf --count N` | 1000 static include aliases (see below) |
| Snakemake | `snakemake/Snakefile --config count=N` | wildcard rule + input function (dynamic) |

## Engine semantic differences (measured, 2026-09)

The three engines do **not** express this chain with equal ease — this is a
real, benchmarkable difference:

- **oxo-flow / Snakemake**: dynamic chains of any depth via wildcards
  (Snakemake: `rule step_i` with an input function; oxo-flow: generated
  rules). No per-step definition cost.
- **Nextflow (DSL2)**: *"A given process or workflow can only be called
  once in a given workflow"* (docs.seqera.io/nextflow/dsl2). A serial
  chain must reuse one process N times, so `hello.nf` pre-expands **1000
  static include aliases** (`include { echo_step as step_i }`) plus 1000
  one-line guards. `--count` beyond 1000 fails; the alias block dominates
  the file (2041 lines vs. 34 for the Snakefile).
- **Snakemake depth ceiling**: the DAG builder is recursive; chains
  deeper than ~484 steps hit `RecursionError` on Python's default
  recursionlimit (measured on Snakemake 9.19.0: 484 OK, 485 fails).
  The runner skips snakemake above 484 and says so.

## Measurement semantics

`run_comparison.sh` times:

- `oxo-flow dry-run` / `validate`: parse + DAG construction + simulated
  execution (**no process spawn**)
- `nextflow run` / `snakemake run`: **full real execution** of the echo
  chain (spawns one `echo` process per step)

Since each step is a microsecond-scale `echo`, wall time is dominated by
each engine's scheduling overhead, which is the quantity of interest.
Cross-engine comparisons are therefore *order-of-magnitude scheduling*
comparisons, not identical-semantics timings; the JSON export
(`results/macro_comparison.json`) preserves per-engine labels so the
difference is always visible.

## Reference numbers (bioinfo-wsx, 25.10.4 nextflow / 9.19.0 snakemake / oxo-flow 0.17.2, 2026-09-08)

| N | oxo-flow dry-run | nextflow run | snakemake run |
|---|---|---|---|
| 10 | ~27 ms | ~6.6 s | ~1.04 s |
| 100 | ~161 ms | ~8.1 s | ~1.09 s |
| 1000 | ~1.27 s | ~25.7 s | skipped (> 484) |
