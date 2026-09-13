#!/bin/bash
#SBATCH --job-name=oxo361-verify
#SBATCH --partition=workq
#SBATCH --nodes=1
#SBATCH --cpus-per-task=8
#SBATCH --time=00:30:00
#SBATCH --output=/data/home/wsx/oxo-bench-360/j361/verify-%j.log

cd /data/home/wsx/oxo-bench-360
echo "=== commit ==="; git log --oneline -1
echo "=== build ==="
export CARGO_BUILD_JOBS=8
cargo build --release -p oxo-flow-cli 2>&1 | tail -1
OXO=/data/home/wsx/oxo-bench-360/target/release/oxo-flow
echo "=== new suggestions (dry-run) ==="
for d in d1_uniform_light d2_heavy_middle d3_heavy_first d4_mixed; do
  cd /data/home/wsx/oxo-bench-360/j361/dags/$d
  rm -rf .oxo-flow
  $OXO dry-run workflow.oxoflow 2>&1 | grep "To execute" | sed "s/^/SUGGEST $d /"
  cd /data/home/wsx/oxo-bench-360
done
echo "=== done ==="
