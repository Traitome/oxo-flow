#!/usr/bin/env bash
# hyperfine --prepare hook: reset every engine to a clean, work-free state.
#
# Runs in the benchmark sandbox (see run_comparison.sh), before every timed
# iteration. Deleting the whole per-engine run directory removes outputs,
# engine metadata (`.snakemake/`, `work/`, `.nextflow/`) and checkpoints, so a
# measured iteration cannot reuse work from a previous one. The input seed
# files (input.txt) are recreated empty, matching the workload definition
# (every engine derives its outputs from the chain index, not from the seed).
set -euo pipefail

rm -rf run/oxo run/nf run/sm .oxo-flow
rm -f run/oxo.log run/oxo-dry.log
mkdir -p run/oxo run/nf run/sm
: > run/oxo/input.txt
: > run/nf/input.txt
: > run/sm/input.txt
cp defs/hello.nf defs/echo_step.nf run/nf/
cp defs/Snakefile run/sm/
