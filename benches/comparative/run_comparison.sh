#!/usr/bin/env bash
# oxo-flow 对比基准运行器 (Phase 4)
#
# 在等价的 N 步串行链上对比三个引擎的调度开销:
#   - oxo-flow:  dry-run（解析 + DAG 构建 + 拓扑执行模拟）
#   - Nextflow:  完整运行（echo 链，include 别名静态展开）
#   - Snakemake: 完整运行（通配符链）
#
# 语义注记: oxo-flow 计时的是 dry-run（不实际 spawn 进程），
# Nextflow/Snakemake 计时的是真实执行（含 echo 子进程）。
# 由于每步都是微秒级 echo， wall time 由引擎调度开销主导，可作
# 调度吞吐的量级对比；严格同语义对比请参考结果中的说明。
#
# 需要安装: Nextflow, Snakemake, oxo-flow, hyperfine
# 用法: ./benches/comparative/run_comparison.sh [N_RULES] [OXO_FLOW_BIN]
set -euo pipefail

# 定位仓库根（脚本可能被 bash 直接执行；无 git 时退回脚本上两级目录）
REPO_ROOT="$(git -C "$(dirname "$0")" rev-parse --show-toplevel 2>/dev/null || cd "$(dirname "$0")/../.." && pwd)"
cd "${REPO_ROOT}"
OUTPUT="benches/comparative/results"
mkdir -p "${OUTPUT}"

N_RULES="${1:-100}"
OXO_BIN="${2:-target/release/oxo-flow}"
WORKFLOW_FILE="/tmp/oxo_bench_hello_${N_RULES}.oxoflow"
NF_DIR="benches/comparative/nextflow"
SM_DIR="benches/comparative/snakemake"

# 检查工具可用性（缺哪个就跳过哪个引擎，全部缺失则报错）
MISSING=""
HAVE_NF=0; HAVE_SM=0; HAVE_OXO=0; HAVE_HF=0
command -v nextflow  >/dev/null 2>&1 && HAVE_NF=1 || MISSING="${MISSING} nextflow"
command -v snakemake >/dev/null 2>&1 && HAVE_SM=1 || MISSING="${MISSING} snakemake"
[ -f "${OXO_BIN}" ]                   && HAVE_OXO=1 || MISSING="${MISSING} oxo-flow(${OXO_BIN})"
command -v hyperfine >/dev/null 2>&1 && HAVE_HF=1 || MISSING="${MISSING} hyperfine"

if [ -n "${MISSING}" ]; then
    echo "警告: 缺少${MISSING} —— 对应引擎将被跳过。"
    [ "${HAVE_OXO}" = "0" ] && { echo "错误: oxo-flow 二进制必须存在，请先 cargo build 或指定路径。"; exit 1; }
    [ "${HAVE_HF}" = "0" ] && { echo "错误: hyperfine 是计时工具，必须安装。"; exit 1; }
fi

# 生成 oxo-flow 工作流（与 hello.nf --count N / Snakefile count=N 等价的链）
python3 -c "
from benches.macro.suite import generate_hello
with open('${WORKFLOW_FILE}', 'w') as f:
    f.write(generate_hello(${N_RULES}))
"

echo "==> 对比基准 (N=${N_RULES} 步串行链, oxo-flow=${OXO_BIN})"
echo ""

# hy 标签数组 -> hyperfine 参数
HYPERFINE_ARGS=(--warmup 2 --min-runs 5 --style basic)

# 1) oxo-flow: dry-run（纯调度开销，无进程 spawn）
if [ "${HAVE_OXO}" = "1" ]; then
    HYPERFINE_ARGS+=(-n "oxo-flow dry-run" "${OXO_BIN} dry-run ${WORKFLOW_FILE}")
    HYPERFINE_ARGS+=(-n "oxo-flow validate" "${OXO_BIN} validate ${WORKFLOW_FILE}")
fi

# 2) Nextflow: 真实执行 echo 链
#    -ansi-log false 关闭进度条; -work-dir 每轮独立避免缓存命中导致空转
if [ "${HAVE_NF}" = "1" ]; then
    NF_RUN_DIR=$(mktemp -d /tmp/nf_bench_run.XXXXXX)
    HYPERFINE_ARGS+=(
        -n "nextflow run"
        "cd ${NF_RUN_DIR} && touch input.txt && nextflow run ${OLDPWD}/${NF_DIR}/hello.nf --count ${N_RULES} -work-dir ${NF_RUN_DIR}/work -ansi-log false > /dev/null && cd ${OLDPWD}"
    )
fi

# 3) Snakemake: 真实执行通配符链
#    注: Snakemake 9.x DAG 构建链深 >484 会 RecursionError（已知边界），
#    超限时跳过并在结果中注记。
if [ "${HAVE_SM}" = "1" ] && [ "${N_RULES}" -le 484 ]; then
    SM_RUN_DIR=$(mktemp -d /tmp/sm_bench_run.XXXXXX)
    cp "${SM_DIR}/Snakefile" "${SM_RUN_DIR}/Snakefile"
    HYPERFINE_ARGS+=(
        -n "snakemake run"
        "cd ${SM_RUN_DIR} && snakemake --cores 1 --config count=${N_RULES} --quiet all > /dev/null && cd ${OLDPWD}"
    )
elif [ "${HAVE_SM}" = "1" ]; then
    echo "注: N=${N_RULES} > 484，跳过 snakemake（DAG 递归深度边界，见 Snakefile 说明）。"
fi

hyperfine "${HYPERFINE_ARGS[@]}" --export-json "${OUTPUT}/macro_comparison.json"

# 清理运行目录
[ -n "${NF_RUN_DIR:-}" ] && rm -rf "${NF_RUN_DIR}"
[ -n "${SM_RUN_DIR:-}" ] && rm -rf "${SM_RUN_DIR}"

echo ""
echo "结果: ${OUTPUT}/macro_comparison.json"
echo ""
echo "管线定义:"
echo "  oxo-flow:  ${WORKFLOW_FILE} (generate_hello(${N_RULES}))"
echo "  Nextflow:  ${NF_DIR}/hello.nf --count ${N_RULES}"
echo "  Snakemake: ${SM_DIR}/Snakefile --config count=${N_RULES}"
echo ""
echo "语义注记: oxo-flow 为 dry-run（无进程 spawn），nextflow/snakemake 为真实执行；"
echo "每步均为微秒级 echo，wall time 由引擎调度开销主导。"
