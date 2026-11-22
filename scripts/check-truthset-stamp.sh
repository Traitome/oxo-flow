#!/usr/bin/env bash
# check-truthset-stamp.sh — provenance drift gate for the BioType truth-set
# evidence artifact (issue #1348).
#
# docs/superpowers/audits/data/biotype-truth-set-results.json records, in its
# `commit` field, the commit whose tree produced the measured results. The
# stamp semantics: the recorded commit must CONTAIN every case the results
# were measured on. Concretely the gate asserts:
#
#   (a) the stamped commit is an ANCESTOR of HEAD (merge-base
#       --is-ancestor) — ancestor-containment, NOT equality with HEAD, because
#       the stamp is written at harness run time (`git rev-parse HEAD` inside
#       scripts/biotype-truth-set.sh) and is therefore usually older than the
#       commit under review;
#   (b) every case path listed in the results exists in the stamped commit's
#       tree (`git cat-file -e <commit>:<path>`), plus every `[[include]]`
#       fixture those case files reference (e.g. b31 pulls
#       incmod_grch38.oxoflow across the include boundary — the fixture has no
#       case row of its own).
#
# The gate would have caught the #1348 defect, where the JSON recorded 54
# cases stamped with `e855fa6a` — the 42-case commit — so re-running the
# harness at the pinned commit could not reproduce the recorded data. When the
# gate fails, re-run `scripts/biotype-truth-set.sh` at HEAD and commit the
# refreshed artifact (the JSON is written by the harness, never by hand).
#
# Usage:  scripts/check-truthset-stamp.sh   (make truthset-stamp)
# Exit:   0 = stamp is reproducible as recorded; 1 = drift/failure.

set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
JSON="$ROOT/docs/superpowers/audits/data/biotype-truth-set-results.json"

fail() { echo "truthset-stamp: FAIL — $*" >&2; exit 1; }

[[ -f "$JSON" ]] || fail "results file missing: docs/superpowers/audits/data/biotype-truth-set-results.json"

STAMP="$(python3 - "$JSON" <<'PY'
import json, sys
print(json.load(open(sys.argv[1])).get("commit") or "")
PY
)" || fail "could not parse the commit stamp from $JSON"

[[ -n "$STAMP" ]] || fail "the results file records no commit stamp"
[[ "$STAMP" != "unknown" ]] || fail 'the results file records commit "unknown" (harness ran outside a git worktree) — regenerate with scripts/biotype-truth-set.sh inside the repository'

git -C "$ROOT" rev-parse --verify --quiet "${STAMP}^{commit}" >/dev/null \
  || fail "stamped commit $STAMP does not resolve in this repository"

# (a) ancestor-containment, NOT equality with HEAD.
git -C "$ROOT" merge-base --is-ancestor "$STAMP" HEAD \
  || fail "stamped commit $STAMP is NOT an ancestor of HEAD ($(git -C "$ROOT" rev-parse --short HEAD)) — the evidence identity has drifted behind the tree; re-run scripts/biotype-truth-set.sh at HEAD and commit the refreshed artifact"

# Internal consistency first: cases_total must equal the number of case rows
# (guards a hand-edited or truncated artifact; the JSON is harness-written).
CASES="$(python3 - "$JSON" <<'PY'
import json, sys
d = json.load(open(sys.argv[1]))
cases = d.get("cases") or []
total = d.get("cases_total")
if total is None or len(cases) != total:
    print(f"cases_total={total} but {len(cases)} case rows are present", file=sys.stderr)
    sys.exit(3)
for c in cases:
    print(c.get("case") or "")
PY
)" || fail "results file is internally inconsistent (cases_total vs case rows — see stderr)"

missing=0
checked=0
while IFS= read -r rel; do
  if [[ -z "$rel" ]]; then
    echo "truthset-stamp: a case row in the results has no path" >&2
    missing=$((missing + 1))
    continue
  fi
  checked=$((checked + 1))
  if ! git -C "$ROOT" cat-file -e "${STAMP}:${rel}" 2>/dev/null; then
    echo "truthset-stamp: case missing at stamped commit ${STAMP}: $rel" >&2
    missing=$((missing + 1))
  fi
done <<< "$CASES"

(( missing == 0 )) || fail "$missing case file(s) listed in the results do not exist at the stamped commit $STAMP — the stamp cannot reproduce the recorded data; re-run scripts/biotype-truth-set.sh at HEAD and commit the refreshed artifact"

# (b-implied) [[include]] fixtures referenced by the case files must also
# exist at the stamp. Read from the stamped tree (not the working tree — the
# stamp's tree is what must contain them).
inc_missing=0
inc_checked=0
while IFS= read -r pair; do
  [[ -z "$pair" ]] && continue
  case_rel="${pair%%|*}"
  inc="${pair#*|}"
  base="$(dirname "$case_rel")"
  rel="$(python3 -c 'import os, sys; print(os.path.normpath(os.path.join(sys.argv[1], sys.argv[2])))' "$base" "$inc")"
  inc_checked=$((inc_checked + 1))
  if ! git -C "$ROOT" cat-file -e "${STAMP}:${rel}" 2>/dev/null; then
    echo "truthset-stamp: include fixture missing at stamped commit ${STAMP}: $rel (referenced by $case_rel)" >&2
    inc_missing=$((inc_missing + 1))
  fi
done < <(python3 - "$ROOT" "$STAMP" "$JSON" <<'PY'
import json, re, subprocess, sys
root, stamp, json_path = sys.argv[1], sys.argv[2], sys.argv[3]
d = json.load(open(json_path))
for c in d.get("cases", []):
    rel = c.get("case") or ""
    if not rel:
        continue
    try:
        content = subprocess.run(
            ["git", "-C", root, "show", f"{stamp}:{rel}"],
            capture_output=True, text=True, check=True,
        ).stdout
    except subprocess.CalledProcessError:
        continue  # already reported missing above
    section = None
    for line in content.splitlines():
        s = line.strip()
        if s.startswith("[[") and "include" in s:
            section = "include"
            continue
        if s.startswith("["):
            section = None
            continue
        if section == "include":
            m = re.match(r'path\s*=\s*"([^"]+)"', s)
            if m:
                print(f"{rel}|{m.group(1)}")
PY
)

(( inc_missing == 0 )) || fail "$inc_missing include-implied fixture(s) missing at the stamped commit $STAMP (see stderr) — re-run scripts/biotype-truth-set.sh at HEAD and commit the refreshed artifact"

echo "truthset-stamp: stamp $STAMP ($(git -C "$ROOT" rev-parse --short "$STAMP")) is an ancestor of HEAD and contains all $checked listed case file(s) + $inc_checked include-implied fixture(s)"
