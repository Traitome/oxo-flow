#!/usr/bin/env bash
# oxo-flow 集成基准测试运行器
set -euo pipefail

cd "$(git rev-parse --show-toplevel 2>/dev/null || echo "${0%/*}")"

OXO_FLOW="${1:-target/debug/oxo-flow}"
OUTPUT="${2:-benches/macro/results}"
ITERATIONS="${3:-3}"

echo "==> oxo-flow 集成基准测试"
echo "    二进制: ${OXO_FLOW}"
echo "    输出:   ${OUTPUT}"
echo "    迭代:   ${ITERATIONS}"
echo ""

# 确保二进制存在
if [ ! -f "${OXO_FLOW}" ]; then
    echo "错误: 二进制文件未找到: ${OXO_FLOW}"
    echo "请先运行 'cargo build'"
    exit 1
fi

# 运行 Python 基准套件
python3 benches/macro/suite.py \
    --oxo-flow "${OXO_FLOW}" \
    --output "${OUTPUT}" \
    --iterations "${ITERATIONS}"

echo ""
echo "==> 完成"
echo "    结果: ${OUTPUT}/macro_results.json"
