#!/usr/bin/env bash
#
# Single source of truth for the project version.
#
#   scripts/bump-version.sh --check            verify every location agrees
#   scripts/bump-version.sh --set 0.18.0       propagate a new version
#   scripts/bump-version.sh --self-test        run the built-in regression tests
#
# Canonical version: `[workspace.package] version` in the root Cargo.toml.
#
# Covered locations
#   Cargo.toml              [package], [workspace.package], workspace dep pins
#   Cargo.lock              internal crate entries (`cargo update --workspace`)
#   crates/oxo-flow-desktop/{Cargo.toml,Cargo.lock}   excluded from the workspace
#   CITATION.cff            version + date-released
#   Dockerfile              ARG VERSION default (local-dev only; CI passes it)
#   frontend/package.json + package-lock.json         ships in the desktop app
#   README.md, Dockerfile.release, docs/guide/src/**   current-version refs
#
# Only references to the CURRENT version are rewritten — never "any
# version-looking string", which is how the previous inline workflow could
# rewrite a third-party `nf-core v3.14.0` or a historical "fixed in v0.17.1".
# Historical references (CHANGELOG.md, benchmark snapshots, test fixtures)
# are deliberately not in the list at all.
#
# `--root DIR` operates on another tree (used by --self-test, and by CI when
# a release tag predates this script).
set -euo pipefail

SELF_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT="$SELF_DIR/.."
MODE=""
NEW_VERSION=""

while [ $# -gt 0 ]; do
  case "$1" in
    --check) MODE="check"; shift ;;
    --self-test) MODE="self-test"; shift ;;
    --set) MODE="set"; shift; NEW_VERSION="${1:-}"; if [ $# -gt 0 ]; then shift; fi ;;
    --root) shift; ROOT="${1:-}"; if [ $# -gt 0 ]; then shift; fi ;;
    -h|--help) sed -n '2,26p' "$0" | sed 's/^# \{0,1\}//'; exit 0 ;;
    *) echo "unknown argument: $1" >&2; exit 2 ;;
  esac
done

[ -n "$MODE" ] || { echo "usage: $0 --check | --set X.Y.Z | --self-test [--root DIR]" >&2; exit 2; }
ROOT="$(cd "$ROOT" && pwd)"

# ── helpers ────────────────────────────────────────────────────────────────

canonical_version() {
  [ -f "$ROOT/Cargo.toml" ] || return 0
  sed -n '/^\[workspace\.package\]/,/^\[/{s/^version = "\(.*\)"/\1/p;}' \
    "$ROOT/Cargo.toml" 2>/dev/null | head -1 || true
}

major_minor() { printf '%s' "${1%.*}"; }

# Regex-escape the dots of a version so it can be interpolated into -E.
re_dots() { printf '%s' "$1" | sed 's/[.]/\\./g'; }

# Portable in-place sed. The two dialects take the backup suffix differently
# (and BSD sed silently ignores GNU's `0,/re/` address), so detect once.
if sed --version >/dev/null 2>&1; then
  sed_i() { sed -i "$@"; }
else
  sed_i() { sed -i '' "$@"; }
fi

# `want <description> <actual> <expected>` — records a mismatch in check mode.
MISMATCHES=""
want() {
  local what="$1" actual="$2" expected="$3"
  [ "$actual" = "$expected" ] || MISMATCHES="${MISMATCHES}${what}: found '${actual}', expected '${expected}'"$'\n'
}

# ── locations ──────────────────────────────────────────────────────────────
#
# Each reader is reused by the check and by the patcher so the two can never
# drift apart; every reader prints one `scope:version` record per line.

# Cargo.toml: [package] version, [workspace.package] version, and the
# internal crate pins in [workspace.dependencies] (required by `cargo publish`).
cargo_toml_versions() {
  awk '
    /^\[package\]/{s="pkg"; next} /^\[workspace\.package\]/{s="ws"; next}
    /^\[workspace\.dependencies\]/{s="dep"; next} /^\[/{s=""}
    s=="pkg" && /^version = "/{print "root:" $3; exit}
  ' "$ROOT/Cargo.toml" | tr -d '"'
  sed -n '/^\[workspace\.package\]/,/^\[/{s/^version = "\(.*\)"/ws:\1/p;}' "$ROOT/Cargo.toml" | head -1
  awk '
    /^\[workspace\.dependencies\]/{s=1; next} /^\[/{s=0}
    s && /oxo-flow-/ && /version = "/{
      match($0, /version = "[^"]*"/)
      print "dep:" substr($0, RSTART+11, RLENGTH-12)
    }
  ' "$ROOT/Cargo.toml" | sort -u
}

patch_cargo_toml() {
  local v="$1"
  sed_i -e "/^\[package\]/,/^\[/{s/^version = \"[0-9][^\"]*\"/version = \"$v\"/;}" \
         -e "/^\[workspace\.package\]/,/^\[/{s/^version = \"[0-9][^\"]*\"/version = \"$v\"/;}" \
         -e "/^\[workspace\.dependencies\]/,/^\[/{/oxo-flow-/s/version = \"[^\"]*\"/version = \"$v\"/;}" \
         "$ROOT/Cargo.toml"
}

# crates/oxo-flow-desktop/Cargo.toml: its own [package] version and the
# oxo-flow-web path pin (the crate is excluded from the workspace).
desktop_toml_versions() {
  local f="$ROOT/crates/oxo-flow-desktop/Cargo.toml"
  [ -f "$f" ] || return 0
  awk '/^\[package\]/{s=1;next} /^\[/{s=0} s && /^version = "/{print "desktop:" $3; exit}' "$f" | tr -d '"'
  sed -n '/oxo-flow-web = /{s/.*version = "\([^"]*\)".*/pin:\1/p;}' "$f" | head -1
}

patch_desktop_toml() {
  local v="$1" f="$ROOT/crates/oxo-flow-desktop/Cargo.toml"
  [ -f "$f" ] || return 0
  sed_i -e "s/^version = \"[0-9][^\"]*\"/version = \"$v\"/" \
         -e "/oxo-flow-web = /s/version = \"[^\"]*\"/version = \"$v\"/" "$f"
}

# CITATION.cff: version + release date.
citation_version() {
  [ -f "$ROOT/CITATION.cff" ] || return 0
  sed -n 's/^version: *//p' "$ROOT/CITATION.cff" | head -1
}

# `date-released` only moves when the version does — a re-run against an
# already-synced tree must not restate the release date of a shipped version.
patch_citation() {
  local cur="$1" v="$2" f="$ROOT/CITATION.cff"
  [ -f "$f" ] || return 0
  sed_i "s/^version: .*/version: $v/" "$f"
  [ "$v" = "$cur" ] || sed_i "s/^date-released: .*/date-released: \"$(date -u +%Y-%m-%d)\"/" "$f"
}

# Dockerfile: the local-dev default of ARG VERSION (CI passes --build-arg).
dockerfile_version() {
  [ -f "$ROOT/Dockerfile" ] || return 0
  sed -n 's/^ARG VERSION=\(.*\)/\1/p' "$ROOT/Dockerfile" | head -1
}

patch_dockerfile() {
  local v="$1" f="$ROOT/Dockerfile"
  [ -f "$f" ] || return 0
  sed_i "s/^ARG VERSION=.*/ARG VERSION=$v/" "$f"
}

# frontend/package.json: the SPA version ships inside the desktop bundle.
package_json_version() {
  [ -f "$ROOT/frontend/package.json" ] || return 0
  sed -n 's/^  "version": "\(.*\)",/\1/p' "$ROOT/frontend/package.json" | head -1
}

patch_package_json() {
  local v="$1" f="$ROOT/frontend/package.json"
  [ -f "$f" ] || return 0
  # Replace the FIRST `"version":` line only, and leave the file's own
  # formatting untouched (BSD sed does not implement GNU's `0,/re/` address,
  # so awk does the one-shot rewrite portably).
  awk -v v="$v" '
    !done && /^  "version": "/ {sub(/"version": "[^"]*"/, "\"version\": \"" v "\""); done = 1}
    {print}
  ' "$f" > "$f.bump-tmp" && mv "$f.bump-tmp" "$f"
}

# frontend/package-lock.json: two version fields (root + packages[""]).
# npm rewrites this on install, but the release job never runs npm — so it
# used to be `git add`ed while still carrying the OLD version.
package_lock_version() {
  [ -f "$ROOT/frontend/package-lock.json" ] || return 0
  python3 - "$ROOT/frontend/package-lock.json" <<'PY'
import json, sys
d = json.load(open(sys.argv[1]))
root = d.get("version") or "?"
nested = d.get("packages", {}).get("", {}).get("version") or "?"
print(f"{root}/{nested}")
PY
}

patch_package_lock() {
  local v="$1" f="$ROOT/frontend/package-lock.json"
  [ -f "$f" ] || return 0
  python3 - "$f" "$v" <<'PY'
import json, sys
path, v = sys.argv[1], sys.argv[2]
with open(path) as fh:
    d = json.load(fh)
d["version"] = v
d.setdefault("packages", {}).setdefault("", {})["version"] = v
with open(path, "w") as fh:
    json.dump(d, fh, indent=2)
    fh.write("\n")
PY
}

# Prose that references the project version: README, the release Dockerfile's
# usage example, and every docs page (each command page carries the version
# banner in its sample output).
text_files() {
  if [ -f "$ROOT/README.md" ]; then printf '%s\n' "$ROOT/README.md"; fi
  if [ -f "$ROOT/Dockerfile.release" ]; then printf '%s\n' "$ROOT/Dockerfile.release"; fi
  find "$ROOT/docs/guide/src" -name '*.md' 2>/dev/null | LC_ALL=C sort
}

patch_text_refs() {
  local cur="$1" v="$2"
  # An empty `cur` would turn the rewrite into `s/v//g` — never run blind.
  [ -n "$cur" ] || return 0
  local files=()
  while IFS= read -r f; do files+=("$f"); done < <(text_files)
  [ ${#files[@]} -gt 0 ] || return 0
  local cur_re cur_mm cur_mm_re v_mm
  cur_re="$(re_dots "$cur")"
  cur_mm="$(major_minor "$cur")"
  cur_mm_re="$(re_dots "$cur_mm")"
  v_mm="$(major_minor "$v")"
  sed_i -E \
    -e "s/v${cur_re}/v${v}/g" \
    -e "s/${cur_re}/${v}/g" \
    -e "s#(oxo-flow:)${cur_mm_re}([^0-9.]|\$)#\1${v_mm}\2#g" \
    "${files[@]}"
}

# Unambiguous project-version references in prose. Deliberately narrow so a
# historical `0.17.1` or a third-party `nf-core v3.14.0` never matches.
check_text_refs() {
  local cur="$1" cur_mm="$2"
  local f rel ln tok ver
  local full='oxo-flow v?[0-9]+\.[0-9]+\.[0-9]+|X-OxoFlow-Version: [0-9]+\.[0-9]+\.[0-9]+|"oxo_flow_version": "[0-9]+\.[0-9]+\.[0-9]+"|oxo-flow:[0-9]+\.[0-9]+\.[0-9]+'
  local mm='oxo-flow:[0-9]+\.[0-9]+([^0-9.]|$)'
  while IFS= read -r f; do
    [ -f "$f" ] || continue
    rel="${f#"$ROOT"/}"
    while IFS=: read -r ln tok; do
      [ -n "${tok:-}" ] || continue
      ver="$(printf '%s' "$tok" | grep -oE '[0-9]+\.[0-9]+\.[0-9]+' | head -1)"
      want "$rel:$ln version reference" "$ver" "$cur"
    done < <(grep -noE "$full" "$f" || true)
    while IFS=: read -r ln tok; do
      [ -n "${tok:-}" ] || continue
      ver="$(printf '%s' "$tok" | grep -oE '[0-9]+\.[0-9]+' | head -1)"
      want "$rel:$ln container tag" "$ver" "$cur_mm"
    done < <(grep -noE "$mm" "$f" || true)
  done < <(text_files)
}

# Cargo.lock (workspace + desktop): the internal crate entries must carry the
# new version. `cargo update --workspace` is authoritative; without cargo
# (check mode) read the lock directly.
lock_versions() {
  for lock in "$ROOT/Cargo.lock" "$ROOT/crates/oxo-flow-desktop/Cargo.lock"; do
    [ -f "$lock" ] || continue
    awk '
      /^name = "oxo-flow/ {name=$3} /^version = "/ {if (name != "") {print name "=" $3; name=""}}
    ' "$lock" | tr -d '"' | sort -u
  done
}

# Content fingerprint of every file the bump can touch — the `changed=` answer
# must not depend on git being present (self-test fixtures are not repos).
snapshot() {
  local rel
  while IFS= read -r rel; do
    if [ -f "$ROOT/$rel" ]; then printf '%s ' "$rel"; cksum < "$ROOT/$rel"; fi
  done <<'LIST'
Cargo.toml
Cargo.lock
CITATION.cff
Dockerfile
Dockerfile.release
README.md
frontend/package.json
frontend/package-lock.json
crates/oxo-flow-desktop/Cargo.toml
crates/oxo-flow-desktop/Cargo.lock
LIST
  while IFS= read -r f; do
    [ -f "$f" ] || continue
    printf '%s ' "${f#"$ROOT"/}"; cksum < "$f"
  done < <(find "$ROOT/docs/guide/src" -name '*.md' 2>/dev/null | LC_ALL=C sort)
}

# ── check ──────────────────────────────────────────────────────────────────

run_check() {
  MISMATCHES=""
  local cur; cur="$(canonical_version)"
  [ -n "$cur" ] || { echo "::error::cannot read [workspace.package] version from Cargo.toml" >&2; return 1; }
  echo "canonical version: $cur"

  local entry what
  while IFS= read -r entry; do
    [ -n "$entry" ] || continue
    what="${entry%%:*}"
    case "$what" in
      root)      want "Cargo.toml [package] version" "${entry#*:}" "$cur" ;;
      ws)        want "Cargo.toml [workspace.package] version" "${entry#*:}" "$cur" ;;
      dep)       want "Cargo.toml workspace-dependency pin" "${entry#*:}" "$cur" ;;
      desktop)   want "oxo-flow-desktop [package] version" "${entry#*:}" "$cur" ;;
      pin)       want "oxo-flow-desktop oxo-flow-web pin" "${entry#*:}" "$cur" ;;
    esac
  done < <(cargo_toml_versions; desktop_toml_versions)

  want "CITATION.cff version" "$(citation_version)" "$cur"
  want "Dockerfile ARG VERSION default" "$(dockerfile_version)" "$cur"
  want "frontend/package.json version" "$(package_json_version)" "$cur"
  local plv; plv="$(package_lock_version)"
  [ -z "$plv" ] || want "frontend/package-lock.json version" "$plv" "$cur/$cur"

  while IFS= read -r entry; do
    [ -n "$entry" ] || continue
    want "Cargo.lock entry ${entry%%=*}" "${entry#*=}" "$cur"
  done < <(lock_versions)

  check_text_refs "$cur" "$(major_minor "$cur")"

  if [ -n "$MISMATCHES" ]; then
    echo "::error::version locations disagree with Cargo.toml ($cur):"
    printf '%s' "$MISMATCHES" | sed 's/^/  - /'
    echo "run: scripts/bump-version.sh --set $cur   (or fix the listed files)"
    return 1
  fi
  echo "all version locations agree ✓"
}

# ── set ────────────────────────────────────────────────────────────────────

run_set() {
  local v="$1"
  echo "$v" | grep -Eq '^[0-9]+\.[0-9]+\.[0-9]+$' \
    || { echo "::error::invalid version '$v' (expected X.Y.Z)" >&2; exit 2; }
  local cur; cur="$(canonical_version)"
  [ -n "$cur" ] || { echo "::error::cannot read [workspace.package] version from $ROOT/Cargo.toml" >&2; exit 1; }
  [ "$v" = "$cur" ] || echo "bumping $cur → $v"

  local before; before="$(snapshot)"

  patch_cargo_toml "$v"
  patch_desktop_toml "$v"
  patch_citation "$cur" "$v"
  patch_dockerfile "$v"
  patch_package_json "$v"
  patch_package_lock "$v"
  patch_text_refs "$cur" "$v"

  # Lockfiles: cargo is authoritative when available (it also resolves the
  # desktop crate's excluded lock); otherwise patch the internal entries.
  if command -v cargo >/dev/null 2>&1 && [ -f "$ROOT/Cargo.toml" ] && [ -z "${BUMP_NO_CARGO:-}" ]; then
    (cd "$ROOT" && cargo update --workspace --offline >/dev/null 2>&1 \
      || cargo update --workspace >/dev/null 2>&1 || true)
    if [ -f "$ROOT/crates/oxo-flow-desktop/Cargo.toml" ]; then
      (cd "$ROOT/crates/oxo-flow-desktop" && cargo update --workspace --offline >/dev/null 2>&1 \
        || cargo update --workspace >/dev/null 2>&1 || true)
    fi
  fi

  # Convergence proof: every location the check knows about must now agree.
  # A patcher that silently no-ops (e.g. an unsupported sed address) fails
  # here instead of shipping a half-bumped release.
  if ! run_check; then
    echo "::error::bump to $v did not converge — see the mismatches above" >&2
    exit 1
  fi

  if [ "$(snapshot)" = "$before" ]; then
    echo "changed=false"
  else
    echo "changed=true"
  fi
}

# ── self-test ──────────────────────────────────────────────────────────────

run_self_test() {
  # Not `local`: the EXIT trap runs after the function scope is gone, where
  # `set -u` would make `$tmp` an unbound-variable error.
  tmp="$(mktemp -d)"
  trap 'rm -rf "${tmp:-}"' EXIT
  local fixture="$tmp/repo"
  mkdir -p "$fixture/frontend" "$fixture/docs/guide/src" "$fixture/crates/oxo-flow-desktop"
  cat > "$fixture/Cargo.toml" <<'EOF'
[package]
name = "oxo-flow"
version = "1.2.3"

[workspace.package]
version = "1.2.3"

[workspace.dependencies]
oxo-flow-core = { path = "crates/oxo-flow-core", version = "1.2.3" }
EOF
  cat > "$fixture/CITATION.cff" <<'EOF'
version: 1.2.3
date-released: "2020-01-01"
EOF
  printf 'ARG VERSION=1.2.3\n' > "$fixture/Dockerfile"
  printf '{\n  "name": "frontend",\n  "version": "1.2.3",\n  "private": true\n}\n' > "$fixture/frontend/package.json"
  printf '{\n  "name": "frontend",\n  "version": "1.2.3",\n  "packages": {\n    "": {\n      "version": "1.2.3"\n    }\n  }\n}\n' > "$fixture/frontend/package-lock.json"
  printf '[package]\nname = "oxo-flow-desktop"\nversion = "1.2.3"\n\n[dependencies]\noxo-flow-web = { path = "../oxo-flow-web", version = "1.2.3" }\n' \
    > "$fixture/crates/oxo-flow-desktop/Cargo.toml"
  printf 'oxo-flow 1.2.3 and v1.2.3\nghcr.io/traitome/oxo-flow:1.2\n' > "$fixture/README.md"
  printf '# doc\n\noxo-flow v1.2.3 — v1.2.3\n' > "$fixture/docs/guide/src/a.md"

  # BUMP_NO_CARGO keeps the fixture hermetic: the fixture manifests are not
  # real cargo packages, and the lockfile path is covered by the repo itself.
  local fails=0
  bump() { BUMP_NO_CARGO=1 bash "$0" "$@"; }
  # `return 0` keeps `set -e` from aborting the harness on an expected failure.
  expect_ok()   { "$@" >/dev/null 2>&1 || { echo "FAIL: expected success: $*"; fails=$((fails+1)); }; return 0; }
  expect_fail() { "$@" >/dev/null 2>&1 && { echo "FAIL: expected failure: $*"; fails=$((fails+1)); }; return 0; }
  check() { bump --check --root "$fixture"; }

  expect_ok check
  # A no-op bump must not restate the release date of a shipped version.
  expect_ok bump --set 1.2.3 --root "$fixture"
  grep -q 'date-released: "2020-01-01"' "$fixture/CITATION.cff" \
    || { echo "FAIL: no-op --set moved date-released"; fails=$((fails+1)); }
  # A single stale location must fail the check.
  sed_i 's/^ARG VERSION=.*/ARG VERSION=9.9.9/' "$fixture/Dockerfile"
  expect_fail check
  sed_i 's/^ARG VERSION=.*/ARG VERSION=1.2.3/' "$fixture/Dockerfile"
  # Drift in prose and in the major.minor container tag must fail too.
  sed_i 's/v1\.2\.3/v9.9.9/' "$fixture/docs/guide/src/a.md"
  expect_fail check
  sed_i 's/v9\.9\.9/v1.2.3/' "$fixture/docs/guide/src/a.md"
  sed_i 's#oxo-flow:1\.2$#oxo-flow:1.1#' "$fixture/README.md"
  expect_fail check
  sed_i 's#oxo-flow:1\.1$#oxo-flow:1.2#' "$fixture/README.md"

  # --set propagates everywhere and the check then passes.
  expect_ok bump --set 4.5.6 --root "$fixture"
  expect_ok check
  grep -q 'version = "4.5.6"' "$fixture/Cargo.toml" || { echo "FAIL: Cargo.toml not bumped"; fails=$((fails+1)); }
  grep -q 'ARG VERSION=4.5.6' "$fixture/Dockerfile" || { echo "FAIL: Dockerfile not bumped"; fails=$((fails+1)); }
  grep -q '"version": "4.5.6"' "$fixture/frontend/package.json" || { echo "FAIL: package.json not bumped"; fails=$((fails+1)); }
  grep -q '"version": "4.5.6"' "$fixture/frontend/package-lock.json" || { echo "FAIL: package-lock.json not bumped"; fails=$((fails+1)); }
  grep -q 'version = "4.5.6"' "$fixture/crates/oxo-flow-desktop/Cargo.toml" || { echo "FAIL: desktop manifest not bumped"; fails=$((fails+1)); }
  grep -q 'v4.5.6' "$fixture/README.md" || { echo "FAIL: README not bumped"; fails=$((fails+1)); }
  grep -q 'oxo-flow:4.5' "$fixture/README.md" || { echo "FAIL: README container tag not bumped"; fails=$((fails+1)); }
  grep -q 'v4.5.6' "$fixture/docs/guide/src/a.md" || { echo "FAIL: docs not bumped"; fails=$((fails+1)); }
  # An unchanged tree reports changed=false; a real bump reported true.
  grep -q 'changed=false' <(bump --set 4.5.6 --root "$fixture") \
    || { echo "FAIL: idempotent --set did not report changed=false"; fails=$((fails+1)); }
  grep -q 'changed=true' <(bump --set 5.0.0 --root "$fixture") \
    || { echo "FAIL: real bump did not report changed=true"; fails=$((fails+1)); }

  # Historical and third-party versions are never rewritten.
  printf 'nf-core v3.14.0 stays; fixed in v4.5.6; R 4.4.1\n' >> "$fixture/docs/guide/src/a.md"
  expect_ok bump --set 7.8.9 --root "$fixture"
  grep -q 'nf-core v3.14.0 stays' "$fixture/docs/guide/src/a.md" \
    || { echo "FAIL: third-party v-version was rewritten"; fails=$((fails+1)); }
  grep -q 'fixed in v4.5.6' "$fixture/docs/guide/src/a.md" \
    || { echo "FAIL: historical v-version was rewritten"; fails=$((fails+1)); }

  if [ "$fails" -ne 0 ]; then
    echo "self-test FAILED ($fails)"
    exit 1
  fi
  echo "self-test passed"
}

case "$MODE" in
  check)     run_check ;;
  set)       [ -n "$NEW_VERSION" ] || { echo "--set needs a version" >&2; exit 2; }; run_set "$NEW_VERSION" ;;
  self-test) run_self_test ;;
esac
