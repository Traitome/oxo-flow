#!/usr/bin/env bash
# Comparative benchmark: oxo-flow vs Nextflow vs Snakemake.
#
# What is measured
#   All engines run the SAME workload: an N-step serial chain
#   step_0 -> step_1 -> ... -> step_{N-1}, where each step runs
#   `echo <i> > step_<i>_output.txt` and consumes the previous step's output.
#   Every timed iteration starts from a clean state (drivers/prepare.sh deletes
#   outputs, checkpoints and engine metadata), so a measured iteration always
#   executes the full chain. Each timed command carries a non-vacuity guard
#   (execution markers + produced artifacts) and fails the iteration rather
#   than timing a cached no-op.
#
# Arms
#   oxo-flow run       REAL execution            (comparable to the engines below)
#   oxo-flow dry-run   parse + DAG + plan only   (scheduling arm, NOT execution)
#   nextflow run       REAL execution
#   snakemake run      REAL execution
#
# Usage
#   ./benches/comparative/run_comparison.sh [N_RULES] [OXO_FLOW_BIN]
#
#   Defaults: N_RULES=100, OXO_FLOW_BIN=target/release/oxo-flow.
#   hyperfine and oxo-flow are mandatory; Nextflow/Snakemake are optional and
#   skipped (with a recorded reason) when absent.
#
# Environment
#   MIN_RUNS=5     hyperfine timed runs per command
#   WARMUP=1       hyperfine warm-up runs per command
#   RESULT_DIR=... output directory (default: benches/comparative/results)
#   ENGINES=oxo-flow,oxo-flow-dry-run,nextflow,snakemake
#   KEEP_WORK=1    keep the temporary sandbox for inspection
#   HOST_LABEL=... host identity written to the identity file
#   GIT_COMMIT=... / GIT_DIRTY_FILES=... commit identity when the benchmark
#                  tree is a plain copy without a .git directory
#   NXF_VER=...    Nextflow framework version (default: the installed version)
#   NXF_OPTS=...   Nextflow JVM options (default: -Xmx1g for small hosts)
#
# Exit status: 0 on a complete run; non-zero if a required engine is missing,
# hyperfine fails, or any non-vacuity guard trips (the workload did not run).
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
REPO_ROOT="$(git -C "${SCRIPT_DIR}" rev-parse --show-toplevel 2>/dev/null || true)"
if [ -z "${REPO_ROOT}" ]; then
    REPO_ROOT="$(cd "${SCRIPT_DIR}/../.." && pwd)"
fi
cd "${REPO_ROOT}"

N_RULES="${1:-100}"
OXO_BIN="${2:-target/release/oxo-flow}"
MIN_RUNS="${MIN_RUNS:-5}"
WARMUP="${WARMUP:-1}"
RESULT_DIR="${RESULT_DIR:-${REPO_ROOT}/benches/comparative/results}"
ENGINES="${ENGINES:-oxo-flow,oxo-flow-dry-run,nextflow,snakemake}"
HOST_LABEL="${HOST_LABEL:-$(hostname 2>/dev/null || echo local)}"
KEEP_WORK="${KEEP_WORK:-0}"

case "${N_RULES}" in
    ''|*[!0-9]*) echo "error: N_RULES must be a positive integer (got '${N_RULES}')" >&2; exit 2 ;;
esac
if [ "${N_RULES}" -lt 1 ]; then
    echo "error: N_RULES must be >= 1" >&2
    exit 2
fi

engine_enabled() {
    case ",${ENGINES}," in
        *",$1,"*) return 0 ;;
        *) return 1 ;;
    esac
}

sha256_of() {
    if command -v sha256sum >/dev/null 2>&1; then
        sha256sum "$1" | cut -d' ' -f1
    elif command -v shasum >/dev/null 2>&1; then
        shasum -a 256 "$1" | cut -d' ' -f1
    else
        echo unavailable
    fi
}

cpu_model() {
    if [ -r /proc/cpuinfo ]; then
        awk -F: '/model name/ {gsub(/^[ \t]+|[ \t]+$/, "", $2); print $2; exit}' /proc/cpuinfo
    elif command -v sysctl >/dev/null 2>&1; then
        sysctl -n machdep.cpu.brand_string 2>/dev/null || echo unknown
    else
        echo unknown
    fi
}

memory_total() {
    if [ -r /proc/meminfo ]; then
        awk '/MemTotal/ {printf "%.1f GiB", $2/1048576}' /proc/meminfo
    elif command -v sysctl >/dev/null 2>&1; then
        sysctl -n hw.memsize 2>/dev/null | awk '{printf "%.1f GiB", $1/1073741824}'
    else
        echo unknown
    fi
}

# --- mandatory tools -------------------------------------------------------
if [ ! -x "${OXO_BIN}" ]; then
    echo "error: oxo-flow binary not found or not executable: ${OXO_BIN}" >&2
    echo "       build it with: cargo build --release -p oxo-flow-cli" >&2
    exit 1
fi
if ! command -v hyperfine >/dev/null 2>&1; then
    echo "error: hyperfine is required (it is the timing tool)" >&2
    exit 1
fi
if ! command -v python3 >/dev/null 2>&1; then
    echo "error: python3 is required (workload generation)" >&2
    exit 1
fi

# --- optional engines ------------------------------------------------------
HAVE_NF=0
HAVE_SM=0
NF_STATUS="skipped(not requested)"
SM_STATUS="skipped(not requested)"
if engine_enabled "nextflow"; then
    if command -v nextflow >/dev/null 2>&1; then
        HAVE_NF=1
        NF_STATUS="used"
    else
        NF_STATUS="skipped(nextflow not on PATH)"
    fi
fi
if engine_enabled "snakemake"; then
    if [ "${N_RULES}" -gt 484 ]; then
        SM_STATUS="skipped(N=${N_RULES} > ~484 DAG recursion limit)"
    elif command -v snakemake >/dev/null 2>&1; then
        HAVE_SM=1
        SM_STATUS="used"
    else
        SM_STATUS="skipped(snakemake not on PATH)"
    fi
fi

export NO_COLOR=1
NXF_OPTS="${NXF_OPTS:--Xmx1g}"
export NXF_OPTS
if [ "${HAVE_NF}" = "1" ] && [ -z "${NXF_VER:-}" ]; then
    NXF_VER="$(nextflow -version 2>&1 | awk '/version/ {print $2; exit}')"
fi
export NXF_VER="${NXF_VER:-}"
if [ "${HAVE_NF}" = "1" ]; then
    # No update checks / remote validation inside the timed process.
    export NXF_OFFLINE="${NXF_OFFLINE:-true}"
fi

# --- sandbox ---------------------------------------------------------------
WORK="$(mktemp -d "${TMPDIR:-/tmp}/oxo-comparative.XXXXXX")"
cleanup() {
    if [ "${KEEP_WORK}" = "1" ]; then
        echo "kept sandbox: ${WORK}" >&2
    else
        rm -rf "${WORK}"
    fi
}
trap cleanup EXIT INT TERM

mkdir -p "${WORK}/defs" "${WORK}/bin" "${WORK}/drivers" "${RESULT_DIR}"
cp "${OXO_BIN}" "${WORK}/bin/oxo-flow"
chmod +x "${WORK}/bin/oxo-flow"
cp "${SCRIPT_DIR}"/drivers/*.sh "${WORK}/drivers/"
chmod +x "${WORK}"/drivers/*.sh

PYTHONPATH="${REPO_ROOT}" python3 - "${N_RULES}" "${WORK}/defs/hello_${N_RULES}.oxoflow" <<'PY'
import sys
from benches.macro.suite import generate_hello

rule_count = int(sys.argv[1])
with open(sys.argv[2], "w") as handle:
    handle.write(generate_hello(rule_count))
PY
python3 "${SCRIPT_DIR}/nextflow/generate_nextflow.py" "${N_RULES}" "${WORK}/defs/hello.nf"
cp "${SCRIPT_DIR}/nextflow/echo_step.nf" "${WORK}/defs/"
cp "${SCRIPT_DIR}/snakemake/Snakefile" "${WORK}/defs/"

# --- identity --------------------------------------------------------------
IDENTITY="${RESULT_DIR}/comparative-N${N_RULES}-identity.txt"
PROOF="${RESULT_DIR}/comparative-N${N_RULES}-proof.txt"
RESULT_JSON="${RESULT_DIR}/comparative-N${N_RULES}.json"

{
    echo "# Comparative benchmark identity"
    echo "generated_utc=$(date -u +%Y-%m-%dT%H:%M:%SZ)"
    echo "host_label=${HOST_LABEL}"
    echo "kernel=$(uname -sr)"
    echo "arch=$(uname -m)"
    echo "cpu_model=$(cpu_model)"
    echo "nproc=$(nproc 2>/dev/null || getconf _NPROCESSORS_ONLN 2>/dev/null || echo unknown)"
    echo "memory_total=$(memory_total)"
    echo "git_commit=${GIT_COMMIT:-$(git -C "${REPO_ROOT}" rev-parse HEAD 2>/dev/null || echo unknown)}"
    echo "git_dirty_files=${GIT_DIRTY_FILES:-$(git -C "${REPO_ROOT}" status --porcelain 2>/dev/null | wc -l | tr -d ' ')}"
    echo "oxo_version=$("${WORK}/bin/oxo-flow" --version 2>/dev/null | head -1)"
    echo "oxo_sha256=$(sha256_of "${OXO_BIN}")"
    echo "hyperfine_version=$(hyperfine --version 2>&1)"
    echo "python_version=$(python3 --version 2>&1)"
    echo "nextflow_status=${NF_STATUS}"
    if [ "${HAVE_NF}" = "1" ]; then
        echo "nextflow_version=$(nextflow -version 2>&1 | awk '/version/ {print $2; exit}')"
        echo "nextflow_java=$(java -version 2>&1 | head -1)"
        echo "NXF_VER=${NXF_VER:-unset}"
        echo "NXF_OPTS=${NXF_OPTS}"
        echo "NXF_OFFLINE=${NXF_OFFLINE:-unset}"
    fi
    echo "snakemake_status=${SM_STATUS}"
    if [ "${HAVE_SM}" = "1" ]; then
        echo "snakemake_version=$(snakemake --version 2>&1 | head -1)"
    fi
    echo "N_rules=${N_RULES}"
    echo "warmup=${WARMUP}"
    echo "min_runs=${MIN_RUNS}"
    echo "engines_requested=${ENGINES}"
    echo "workflow_sha256=$(sha256_of "${WORK}/defs/hello_${N_RULES}.oxoflow")"
    echo "nextflow_definition_sha256=$(sha256_of "${WORK}/defs/hello.nf")"
    echo "nextflow_generator_sha256=$(sha256_of "${SCRIPT_DIR}/nextflow/generate_nextflow.py")"
    echo "nextflow_process_sha256=$(sha256_of "${SCRIPT_DIR}/nextflow/echo_step.nf")"
    echo "snakemake_definition_sha256=$(sha256_of "${WORK}/defs/Snakefile")"
    echo "drivers_sha256=$(cat "${WORK}"/drivers/*.sh | sha256_of /dev/stdin)"
    echo "result_json=$(basename "${RESULT_JSON}")"
    echo "proof_file=$(basename "${PROOF}")"
    echo "commands (cwd = benchmark sandbox):"
    echo "  oxo-flow real:  ./drivers/oxo_run.sh ${N_RULES}"
    echo "  oxo-flow dry:   ./drivers/oxo_dry_run.sh ${N_RULES}"
    if [ "${HAVE_NF}" = "1" ]; then
        echo "  nextflow:       ./drivers/nextflow_run.sh ${N_RULES}"
    fi
    if [ "${HAVE_SM}" = "1" ]; then
        echo "  snakemake:      ./drivers/snakemake_run.sh ${N_RULES}"
    fi
} > "${IDENTITY}"

# --- non-vacuity and cached-no-op control ----------------------------------
# Fresh run through the SAME driver the timed arm uses = positive control: the
# workload must execute and the guard must pass. Running the driver a second
# time in the unchanged directory = negative control: for caching engines the
# second run does no work, so the guard must reject it (non-zero). This proves
# both that measured iterations execute and that a cached no-op cannot be
# timed silently.
reset_state() {
    ( cd "${WORK}" && ./drivers/prepare.sh )
}

{
    echo "# Non-vacuity and cached-no-op control"
    echo "# generated_utc=$(date -u +%Y-%m-%dT%H:%M:%SZ)"
    echo "# fresh_markers/fresh_outputs prove the workload executed in a clean dir;"
    echo "# second_same_dir_markers shows whether the engine reuses prior work;"
    echo "# the negative-control column shows the timed guard rejected that rerun."
    echo "# engine | fresh_markers | fresh_outputs | second_same_dir_markers | guard_on_cached_rerun"
} > "${PROOF}"

if engine_enabled "oxo-flow"; then
    reset_state
    if ! ( cd "${WORK}" && ./drivers/oxo_run.sh "${N_RULES}" ) >/dev/null 2>&1; then
        echo "error: oxo-flow fresh-run positive control failed" >&2
        exit 4
    fi
    oxo_m1=$(grep -c '"event":"rule_started"' "${WORK}/run/oxo.log" || true)
    oxo_o1=$(ls "${WORK}"/run/oxo/step_*_output.txt 2>/dev/null | wc -l | tr -d ' ')
    if ( cd "${WORK}" && ./drivers/oxo_run.sh "${N_RULES}" ) >/dev/null 2>&1; then
        echo "error: oxo-flow guard did not reject a cached rerun" >&2
        exit 5
    fi
    oxo_m2=$(grep -c '"event":"rule_started"' "${WORK}/run/oxo.log" || true)
    if [ "${oxo_m1}" -ne "${N_RULES}" ] || [ "${oxo_o1}" -ne "${N_RULES}" ]; then
        echo "error: oxo-flow fresh-run proof failed (executed=${oxo_m1}, outputs=${oxo_o1}, expected ${N_RULES})" >&2
        exit 4
    fi
    echo "oxo-flow | ${oxo_m1} rule_started | ${oxo_o1} outputs | ${oxo_m2} rule_started | rejected (exit != 0)" >> "${PROOF}"
fi

if [ "${HAVE_SM}" = "1" ]; then
    reset_state
    if ! ( cd "${WORK}" && ./drivers/snakemake_run.sh "${N_RULES}" ) >/dev/null 2>&1; then
        echo "error: snakemake fresh-run positive control failed" >&2
        exit 4
    fi
    sm_j1=$(grep -c "Finished job" "${WORK}/run/sm/run.log" || true)
    sm_o1=$(ls "${WORK}"/run/sm/step_*_output.txt 2>/dev/null | wc -l | tr -d ' ')
    if ( cd "${WORK}" && ./drivers/snakemake_run.sh "${N_RULES}" ) >/dev/null 2>&1; then
        echo "error: snakemake guard did not reject a cached rerun" >&2
        exit 5
    fi
    sm_j2=$(grep -c "Finished job" "${WORK}/run/sm/run.log" || true)
    sm_noop=$(grep -ci "Nothing to be done" "${WORK}/run/sm/run.log" || true)
    if [ "${sm_j1}" -lt "${N_RULES}" ] || [ "${sm_o1}" -ne "${N_RULES}" ]; then
        echo "error: snakemake fresh-run proof failed (jobs=${sm_j1}, outputs=${sm_o1}, expected ${N_RULES})" >&2
        exit 4
    fi
    echo "snakemake | ${sm_j1} Finished job | ${sm_o1} outputs | ${sm_j2} Finished job (noop_marker=${sm_noop}) | rejected (exit != 0)" >> "${PROOF}"
fi

if [ "${HAVE_NF}" = "1" ]; then
    reset_state
    if ! ( cd "${WORK}" && ./drivers/nextflow_run.sh "${N_RULES}" ) >/dev/null 2>&1; then
        echo "error: nextflow fresh-run positive control failed" >&2
        exit 4
    fi
    nf_s1=$(grep -c "Submitted process" "${WORK}/run/nf/run.log" || true)
    nf_o1=$(ls "${WORK}"/run/nf/results/step_*_output.txt 2>/dev/null | wc -l | tr -d ' ')
    ( cd "${WORK}" && ./drivers/nextflow_run.sh "${N_RULES}" ) >/dev/null 2>&1 || {
        echo "error: nextflow rerun in the same directory failed" >&2
        exit 4
    }
    nf_s2=$(grep -c "Submitted process" "${WORK}/run/nf/run.log" || true)
    if [ "${nf_s1}" -ne "${N_RULES}" ] || [ "${nf_o1}" -ne "${N_RULES}" ]; then
        echo "error: nextflow fresh-run proof failed (submitted=${nf_s1}, outputs=${nf_o1}, expected ${N_RULES})" >&2
        exit 4
    fi
    echo "nextflow | ${nf_s1} Submitted process | ${nf_o1} outputs | ${nf_s2} Submitted process | n/a (re-executes without -resume)" >> "${PROOF}"
fi

# --- timed comparison ------------------------------------------------------
HYPERFINE=(hyperfine --warmup "${WARMUP}" --min-runs "${MIN_RUNS}" \
    --style basic --prepare ./drivers/prepare.sh --export-json "${RESULT_JSON}")
ARM_COUNT=0

if engine_enabled "oxo-flow"; then
    HYPERFINE+=(-n "oxo-flow run (real execution)" "./drivers/oxo_run.sh ${N_RULES}")
    ARM_COUNT=$((ARM_COUNT + 1))
fi
if engine_enabled "oxo-flow-dry-run"; then
    HYPERFINE+=(-n "oxo-flow dry-run (scheduling only)" "./drivers/oxo_dry_run.sh ${N_RULES}")
    ARM_COUNT=$((ARM_COUNT + 1))
fi
if [ "${HAVE_NF}" = "1" ]; then
    HYPERFINE+=(-n "nextflow run (real execution)" "./drivers/nextflow_run.sh ${N_RULES}")
    ARM_COUNT=$((ARM_COUNT + 1))
fi
if [ "${HAVE_SM}" = "1" ]; then
    HYPERFINE+=(-n "snakemake run (real execution)" "./drivers/snakemake_run.sh ${N_RULES}")
    ARM_COUNT=$((ARM_COUNT + 1))
fi
if [ "${ARM_COUNT}" -eq 0 ]; then
    echo "error: no benchmark arm is available (requested: ${ENGINES}); check PATH/ENGINES" >&2
    exit 1
fi

echo "==> Comparative benchmark: N=${N_RULES}, warmup=${WARMUP}, min-runs=${MIN_RUNS}"
echo "    oxo-flow=${OXO_BIN} sha256=$(sha256_of "${OXO_BIN}")"
echo "    nextflow: ${NF_STATUS}"
echo "    snakemake: ${SM_STATUS}"
echo ""

( cd "${WORK}" && "${HYPERFINE[@]}" )

echo ""
echo "Results:"
python3 - "${RESULT_JSON}" <<'PY'
import json
import sys

with open(sys.argv[1]) as handle:
    data = json.load(handle)
for result in data["results"]:
    print(f"  {result['command']:48s} median {result['median']*1000:9.1f} ms   "
          f"mean {result['mean']*1000:9.1f} ms   (n={len(result['times'])})")
PY
echo ""
echo "Raw JSON:     ${RESULT_JSON}"
echo "Identity:     ${IDENTITY}"
echo "No-op proof:  ${PROOF}"
