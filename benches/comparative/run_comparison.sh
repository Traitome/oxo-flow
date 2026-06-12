#!/usr/bin/env bash
# oxo-flow 对比基准运行器 (Phase 4)
#
# 需要安装: Nextflow, Snakemake, oxo-flow
# 用法: ./benches/comparative/run_comparison.sh

set -euo pipefail

cd "$(git rev-parse --show-toplevel 2>/dev/null || echo "${0%/*}")"
OUTPUT="benches/comparative/results"
mkdir -p "${OUTPUT}"

echo "==> oxo-flow 对比基准测试"
echo ""
echo "需要安装以下工具:"
echo "  - Nextflow  (https://www.nextflow.io/)"
echo "  - Snakemake (https://snakemake.readthedocs.io/)"
echo "  - hyperfine (https://github.com/sharkdp/hyperfine)"
echo ""

# 检查工具可用性
MISSING=""
command -v nextflow  >/dev/null 2>&1 || MISSING="${MISSING} nextflow"
command -v snakemake >/dev/null 2>&1 || MISSING="${MISSING} snakemake"
command -v oxo-flow  >/dev/null 2>&1 || MISSING="${MISSING} oxo-flow"
command -v hyperfine >/dev/null 2>&1 || MISSING="${MISSING} hyperfine"

if [ -n "${MISSING}" ]; then
    echo "缺少:${MISSING}"
    echo "请安装缺失的工具后重试。"
    echo ""
    echo "配置定义位于:"
    echo "  benches/comparative/nextflow/hello.nf"
    echo "  benches/comparative/snakemake/Snakefile"
    exit 1
fi

# 运行对比基准 (使用 hyperfine)
N_RULES=100
WORKFLOW_FILE="/tmp/oxo_bench_hello_${N_RULES}.oxoflow"

# 生成 oxo-flow 工作流
python3 -c "
from benches.macro.suite import generate_hello
with open('${WORKFLOW_FILE}', 'w') as f:
    f.write(generate_hello(${N_RULES}))
"

echo "运行对比基准 (N=${N_RULES} rules)..."
echo ""

hyperfine --warmup 2 --min-runs 5 \
    -n "oxo-flow validate" \
    "oxo-flow validate ${WORKFLOW_FILE}" \
    -n "oxo-flow dry-run" \
    "oxo-flow dry-run ${WORKFLOW_FILE}" \
    --export-json "${OUTPUT}/macro_comparison.json"

echo ""
echo "结果: ${OUTPUT}/macro_comparison.json"
echo ""
echo "管线定义:"
echo "  oxo-flow:  ${WORKFLOW_FILE}"
echo "  Nextflow:  benches/comparative/nextflow/hello.nf"
echo "  Snakemake: benches/comparative/snakemake/Snakefile"
