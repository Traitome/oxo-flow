#!/usr/bin/env bash
# check-truthset-stamp.sh — provenance drift gate for the BioType truth-set
# evidence artifacts (issue #1348; extended to the governed campaign by
# issue #1396).
#
# Each results JSON records, in its `commit` field, the commit whose tree
# produced the measured results. The stamp semantics: the recorded commit
# must CONTAIN every case the results were measured on. Concretely the gate
# asserts, for every results file:
#
#   (a) the stamped commit is an ANCESTOR of HEAD (merge-base
#       --is-ancestor) — ancestor-containment, NOT equality with HEAD, because
#       the stamp is written at harness run time (`git rev-parse HEAD` inside
#       the runner) and is therefore usually older than the commit under
#       review;
#   (b) every case path listed in the results exists in the stamped commit's
#       tree (`git cat-file -e <commit>:<path>`), plus every `[[include]]`
#       fixture those case files reference (e.g. the pilot's b31 and the
#       governed g082/g083/g100 pull an include fixture across the boundary —
#       the fixture has no case row of its own).
#
# The gate would have caught the #1348 defect, where the pilot JSON recorded
# 54 cases stamped with `e855fa6a` — the 42-case commit — so re-running the
# harness at the pinned commit could not reproduce the recorded data. When
# the gate fails, re-run the corresponding campaign at HEAD and commit the
# refreshed artifact (the JSON is written by the harness, never by hand).
#
# Usage:  scripts/check-truthset-stamp.sh   (make truthset-stamp)
# Exit:   0 = every stamp is reproducible as recorded; 1 = drift/failure.

set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"

FILES=(
  "docs/superpowers/audits/data/biotype-truth-set-results.json"
  "docs/superpowers/audits/data/biotype-truthset-governed-results.json"
  "docs/superpowers/audits/data/biotype-truthset-governed-supplement-results.json"
)

fail() { echo "truthset-stamp: FAIL — $*" >&2; exit 1; }

check_file() {
  local json="$1"
  local json_abs="$ROOT/$json"

  [[ -f "$json_abs" ]] || fail "results file missing: $json"

  local stamp
  stamp="$(python3 - "$json_abs" <<'PY'
import json, sys
print(json.load(open(sys.argv[1])).get("commit") or "")
PY
)" || fail "could not parse the commit stamp from $json"

  [[ -n "$stamp" ]] || fail "$json records no commit stamp"
  [[ "$stamp" != "unknown" ]] || fail "$json records commit \"unknown\" (harness ran outside a git worktree) — regenerate it inside the repository"

  git -C "$ROOT" rev-parse --verify --quiet "${stamp}^{commit}" >/dev/null \
    || fail "stamped commit $stamp in $json does not resolve in this repository"

  # (a) ancestor-containment, NOT equality with HEAD.
  git -C "$ROOT" merge-base --is-ancestor "$stamp" HEAD \
    || fail "stamped commit $stamp in $json is NOT an ancestor of HEAD ($(git -C "$ROOT" rev-parse --short HEAD)) — the evidence identity has drifted behind the tree; re-run the campaign at HEAD and commit the refreshed artifact"

  # Internal consistency first: cases_total must equal the number of case
  # rows (guards a hand-edited or truncated artifact; the JSON is
  # harness-written).
  local cases
  cases="$(python3 - "$json_abs" <<'PY'
import json, sys
d = json.load(open(sys.argv[1]))
cases = d.get("cases") or []
total = d.get("cases_total")
if total is None:
    total = (d.get("summary") or {}).get("cases_total")
if total is None or len(cases) != total:
    print(f"cases_total={total} but {len(cases)} case rows are present", file=sys.stderr)
    sys.exit(3)
for c in cases:
    print(c.get("case") or "")
PY
)" || fail "$json is internally inconsistent (cases_total vs case rows — see stderr)"

  local missing=0 checked=0 rel
  while IFS= read -r rel; do
    if [[ -z "$rel" ]]; then
      echo "truthset-stamp: a case row in $json has no path" >&2
      missing=$((missing + 1))
      continue
    fi
    checked=$((checked + 1))
    if ! git -C "$ROOT" cat-file -e "${stamp}:${rel}" 2>/dev/null; then
      echo "truthset-stamp: case missing at stamped commit ${stamp}: $rel (from $json)" >&2
      missing=$((missing + 1))
      continue
    fi
    # Content custody: the working-tree case body must be byte-identical to
    # the blob in the stamped commit. A post-stamp edit of a case body (which
    # would silently change the measured corpus) fails closed here.
    local stamped_blob worktree_blob
    stamped_blob="$(git -C "$ROOT" rev-parse "${stamp}:${rel}")"
    worktree_blob="$(git -C "$ROOT" hash-object "$ROOT/$rel")"
    if [[ "$stamped_blob" != "$worktree_blob" ]]; then
      echo "truthset-stamp: case body differs from the stamped commit ${stamp}: $rel (stamped blob ${stamped_blob:0:12}, worktree blob ${worktree_blob:0:12}) — the corpus was edited after the stamp; re-run the campaign and commit the refreshed artifact" >&2
      missing=$((missing + 1))
    fi
  done <<< "$cases"

  (( missing == 0 )) || fail "$missing case file(s) listed in $json are missing or content-differ at the stamped commit $stamp — the stamp cannot reproduce the recorded data; re-run the campaign at HEAD and commit the refreshed artifact"

  # (b-implied) [[include]] fixtures referenced by the case files must also
  # exist at the stamp. Read from the stamped tree (not the working tree —
  # the stamp's tree is what must contain them).
  local inc_missing=0 inc_checked=0 pair case_rel inc base rel_inc
  while IFS= read -r pair; do
    [[ -z "$pair" ]] && continue
    case_rel="${pair%%|*}"
    inc="${pair#*|}"
    base="$(dirname "$case_rel")"
    rel_inc="$(python3 -c 'import os, sys; print(os.path.normpath(os.path.join(sys.argv[1], sys.argv[2])))' "$base" "$inc")"
    inc_checked=$((inc_checked + 1))
    if ! git -C "$ROOT" cat-file -e "${stamp}:${rel_inc}" 2>/dev/null; then
      echo "truthset-stamp: include fixture missing at stamped commit ${stamp}: $rel_inc (referenced by $case_rel, from $json)" >&2
      inc_missing=$((inc_missing + 1))
    fi
  done < <(python3 - "$ROOT" "$stamp" "$json_abs" <<'PY'
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

  (( inc_missing == 0 )) || fail "$inc_missing include-implied fixture(s) missing at the stamped commit $stamp in $json (see stderr) — re-run the campaign at HEAD and commit the refreshed artifact"

  # Manifest binding: every governed results file names the sealed
  # pre-run/supplement manifest it was produced under; verify its hash.
  local manifest_field expected_manifest actual_manifest
  manifest_field="$(python3 - "$json_abs" <<'PY'
import json, sys
print(json.load(open(sys.argv[1])).get("manifest") or "")
PY
)"
  if [[ -n "$manifest_field" ]]; then
    expected_manifest="$(python3 - "$json_abs" <<'PY'
import json, sys
print(json.load(open(sys.argv[1])).get("manifest_sha256") or "")
PY
)"
    actual_manifest="$(shasum -a 256 "$ROOT/$manifest_field" | awk '{print $1}')"
    [[ "$actual_manifest" == "$expected_manifest" ]] \
      || fail "$json records manifest_sha256 $expected_manifest but $manifest_field hashes to $actual_manifest — the results are not bound to the sealed manifest"
  fi

  # Decision binding: the governed primary decision must hash the results
  # file it claims to analyze.
  local decision="$ROOT/docs/superpowers/audits/data/biotype-truthset-governed-decision.json"
  if [[ "$json" == "docs/superpowers/audits/data/biotype-truthset-governed-results.json" && -f "$decision" ]]; then
    python3 - "$decision" "$json_abs" <<'PY' || fail "decision file does not bind this results file (results_sha256 mismatch)"
import hashlib, json, sys
dec = json.load(open(sys.argv[1]))
h = hashlib.sha256(open(sys.argv[2], "rb").read()).hexdigest()
if dec.get("results_sha256") != h:
    print(f"decision results_sha256={dec.get('results_sha256')} but results hash to {h}", file=sys.stderr)
    sys.exit(1)
PY
  fi

  echo "truthset-stamp: $json — stamp $stamp ($(git -C "$ROOT" rev-parse --short "$stamp")) is an ancestor of HEAD and contains all $checked listed case file(s) (content-identical) + $inc_checked include-implied fixture(s)"
}

for f in "${FILES[@]}"; do
  check_file "$f"
done
