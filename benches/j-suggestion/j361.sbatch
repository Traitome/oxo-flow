#!/bin/bash
#SBATCH --job-name=oxo361-j
#SBATCH --partition=workq
#SBATCH --nodes=1
#SBATCH --cpus-per-task=64
#SBATCH --time=02:00:00
#SBATCH --output=/data/home/wsx/oxo-bench-360/j361/j361-%j.log

cd /data/home/wsx/oxo-bench-360
cargo build --release -p oxo-flow-cli 2>&1 | tail -1
OXO=/data/home/wsx/oxo-bench-360/target/release/oxo-flow
echo "=== node ==="; hostname; nproc

declare -A CUR=( [d1_uniform_light]=16 [d2_heavy_middle]=4 [d3_heavy_first]=4 [d4_mixed]=4 )
declare -A GRP=( [d1_uniform_light]=20 [d2_heavy_middle]=20 [d3_heavy_first]=20 [d4_mixed]=20 )

for d in d1_uniform_light d2_heavy_middle d3_heavy_first d4_mixed; do
  for mode in cur grp unbounded; do
    case $mode in cur) j=${CUR[$d]};; grp) j=${GRP[$d]};; unbounded) j=64;; esac
    cd /data/home/wsx/oxo-bench-360/j361/$d
    rm -rf .oxo-flow s*.txt out
    start=$(date +%s.%N)
    $OXO run -j $j workflow.oxoflow > run-$mode.log 2>&1
    rc=$?
    end=$(date +%s.%N)
    secs=$(awk "BEGIN{printf \"%.1f\", $end - $start}")
    ok=$(ls s*.txt 2>/dev/null | wc -l)
    echo "RESULT $d mode=$mode j=$j rc=$rc wall=${secs}s outputs=$ok"
    cd /data/home/wsx/oxo-bench-360
  done
done

echo "=== engine suggestions (dry-run) ==="
for d in d1_uniform_light d2_heavy_middle d3_heavy_first d4_mixed; do
  cd /data/home/wsx/oxo-bench-360/j361/$d
  rm -rf .oxo-flow
  $OXO dry-run workflow.oxoflow 2>&1 | grep "To execute" | sed "s/^/SUGGEST $d /"
  cd /data/home/wsx/oxo-bench-360
done
echo "=== done ==="
