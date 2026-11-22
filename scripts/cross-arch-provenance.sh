#!/usr/bin/env bash
# Cross-architecture reproduction measurement harness (#1166).
# Reproduces one pinned workflow with provenance capture, then re-audits
# the recorded output checksums. Runs UNCHANGED on every host; host-specific
# differences are supplied as args / environment.
#
# Usage: scripts/cross-arch-provenance.sh <workflow.oxoflow> <output-dir> [oxo-flow-bin]
# Env:   OXO_FLOW_BIN  oxo-flow binary (default <repo>/target/debug/oxo-flow, else PATH)
#
# Emits into <output-dir>:
#   identity.txt     per-arm identity block (hostname, uname, nproc, tool versions)
#   checkpoint.json  raw run checkpoint (recorded sha256 checksums)
#   verify.json      raw `provenance verify --json` output
#   artifacts.sha256 sha256sum of every produced artifact
#   rows.tsv         per-artifact {file,status,expected,actual}
#   env_lock.txt     per-rule environment-lock identity (issue #1274): resolved
#                    container digest / env lock hash / explicit sentinel. Two
#                    arms must share these before any cross-arch claim holds.
#
# Exit 0 only when the run succeeds AND every recorded checksum re-verifies.
set -euo pipefail

wf="${1:?usage: $0 <workflow.oxoflow> <output-dir> [oxo-flow-bin]}"
outdir="${2:?usage: $0 <workflow.oxoflow> <output-dir> [oxo-flow-bin]}"
bin="${3:-${OXO_FLOW_BIN:-}}"
if [ -z "$bin" ]; then
  repo="$(cd "$(dirname "$0")/.." && pwd)"
  if [ -x "$repo/target/debug/oxo-flow" ]; then bin="$repo/target/debug/oxo-flow"; else bin="oxo-flow"; fi
fi
mkdir -p "$outdir"

# --- per-arm identity block (ledger §8) -----------------------------------
{
  echo "hostname:   $(hostname)"
  echo "uname:      $(uname -a)"
  echo "arch:       $(uname -m)"
  echo "nproc:      $(nproc 2>/dev/null || sysctl -n hw.ncpu)"
  echo "oxo-flow:   $("$bin" --version 2>/dev/null | head -1)"
  echo "commit:     $(git -C "$(dirname "$0")/.." rev-parse HEAD 2>/dev/null || echo unknown)"
  echo "gzip:       $(gzip -V 2>&1 | head -1)"
  echo "python3:    $(python3 --version 2>&1)"
  echo "sort:       $(sort --version 2>/dev/null | head -1 || echo 'BSD sort (macOS)')"
  echo "locale:     LANG=$(locale 2>/dev/null | sed -n 's/^LANG=//p') LC_ALL=$(locale 2>/dev/null | sed -n 's/^LC_ALL=//p') LC_COLLATE=$(locale 2>/dev/null | sed -n 's/^LC_COLLATE=//p')"
  echo "sh:         $(readlink -f /bin/sh 2>/dev/null || echo /bin/sh)"
  echo "date-utc:   $(date -u +%Y-%m-%dT%H:%M:%SZ)"
} > "$outdir/identity.txt"

# --- run with provenance capture ------------------------------------------
rm -rf "$outdir/work"
"$bin" run "$wf" -d "$outdir/work" --provenance --quiet
ckpt="$outdir/work/.oxo-flow/checkpoint.json"
cp "$ckpt" "$outdir/checkpoint.json"

# --- re-audit recorded checksums (raw JSON) -------------------------------
"$bin" provenance verify "$ckpt" --json > "$outdir/verify.json" 2>/dev/null

# --- environment-lock identity (issue #1274) ------------------------------
# Records the per-rule env-lock identity so two arms can be compared: a
# cross-arch claim is only meaningful when BOTH arms report the SAME resolved
# identity for every rule. Reads it from the checkpoint (the run-time source of
# truth) and emits rule<TAB>value<TAB>resolved so a future re-run can diff the
# two arms' env_lock.txt files. A sentinel ("<kind>|unresolved"/"|unavailable")
# is recorded verbatim — never rewritten into a synthetic digest.
python3 - "$ckpt" > "$outdir/env_lock.txt" <<'PY'
import json, sys
try:
    ckpt = json.load(open(sys.argv[1]))
except Exception as e:
    print("ERROR\tcould not read checkpoint: %s" % e)
    sys.exit(0)
identity = ckpt.get("env_lock_identity") or {}
print("rule\tidentity\tresolved")
for rule in sorted(identity):
    value = identity[rule]
    resolved = not (value.endswith("unresolved") or value.endswith("unavailable"))
    print("%s\t%s\t%s" % (rule, value, "yes" if resolved else "no"))
PY
nresolved=$(python3 -c "import json,sys;d=json.load(open(sys.argv[1])).get('env_lock_identity') or {};print(sum(1 for v in d.values() if not (v.endswith('unresolved') or v.endswith('unavailable'))))" "$ckpt")
ntotal=$(python3 -c "import json,sys;print(len(json.load(open(sys.argv[1])).get('env_lock_identity') or {}))" "$ckpt")
echo "env-lock identity: $nresolved/$ntotal rule(s) resolved (see env_lock.txt)"
cat "$outdir/env_lock.txt"

# --- independent raw artifact hashing -------------------------------------
if command -v sha256sum >/dev/null 2>&1; then
  ( cd "$outdir/work" && find . -name '*.txt' -o -name '*.gz' | sed 's|^\./||' \
      | sort | xargs sha256sum ) > "$outdir/artifacts.sha256" 2>/dev/null || true
else
  ( cd "$outdir/work" && find . -name '*.txt' -o -name '*.gz' | sed 's|^\./||' \
      | sort | xargs shasum -a 256 ) > "$outdir/artifacts.sha256" 2>/dev/null || true
fi

# --- per-artifact rows: file, status, expected, actual --------------------
python3 - "$outdir/verify.json" > "$outdir/rows.tsv" <<'PY'
import json, sys
d = json.load(open(sys.argv[1]))["verify"]
print("file\tstatus\texpected\tactual")
for e in d["entries"]:
    print("%s\t%s\t%s\t%s" % (e["file"], e["status"], e["expected"], e["actual"]))
PY

cat "$outdir/rows.tsv"
mism=$(python3 -c "import json,sys;print(json.load(open(sys.argv[1]))['verify']['mismatched'])" "$outdir/verify.json")
if [ "$mism" != "0" ]; then echo "PROVENANCE MISMATCH: $mism" >&2; exit 1; fi
echo "OK: all recorded checksums re-verified"
