# Comparative Benchmarks (Phase 4)

This directory contains pipeline definitions equivalent to the oxo-flow
micro-benchmarks, written for Nextflow and Snakemake.

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

```bash
# Run all comparisons
./benches/comparative/run_comparison.sh

# Individual runs
hyperfine -n "oxo-flow" "oxo-flow validate hello_100.oxoflow"
hyperfine -n "nextflow" "nextflow run hello.nf"
hyperfine -n "snakemake" "snakemake --cores 1"
```

## Pipeline Equivalence

| Engine | File | Description |
|---|---|---|
| oxo-flow | `benches/macro/suite.py:generate_hello(N)` | N-rule chain |
| Nextflow | `nextflow/hello.nf --count N` | N-process chain |
| Snakemake | `snakemake/Snakefile` | N-rule chain (configurable via COUNT) |

All three pipelines produce an identical DAG: sequential dependency chain
where each step copies its input to an output file.
