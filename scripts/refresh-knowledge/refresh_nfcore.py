#!/usr/bin/env python3
"""Refresh nfcore_modules.jsonl — the embedded nf-core module table.

Fetches the nf-core/modules repository tree (GitHub git-trees API, one
request) and then every modules/nf-core/*/meta.yml (+ sibling
environment.yml where present) via raw.githubusercontent.com with 8-way
concurrency (~921 tool directories, 2041 meta.yml files).

Output (JSONL):

    {"n": "<module name>", "v": ["<bioconda pin>", ...],
     "t": "<description>", "license": "...", "biotools_id": "...", "doi": "..."}

  - n         = meta.yml `name`
  - v         = bioconda:: pins from the sibling environment.yml
                (channel prefix stripped, e.g. "fastp=1.3.6")
  - t         = meta.yml `description`
  - license   = first tools[].licence/license (joined with ", ")
  - biotools_id = first tools[].identifier (e.g. "biotools:fastp")
  - doi       = first tools[].doi

meta.yml / environment.yml are parsed with a minimal YAML-subset parser
(stdlib only — no PyYAML dependency).

Idempotent: re-running replaces the output file atomically.
"""

import argparse
import json
import os
import re
import sys
from concurrent.futures import ThreadPoolExecutor, as_completed

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from common import default_out_dir, http_get, http_get_json, log, update_meta, write_jsonl  # noqa: E402

TREE_URL = "https://api.github.com/repos/nf-core/modules/git/trees/master?recursive=1"
RAW_BASE = "https://raw.githubusercontent.com/nf-core/modules/master/"
CONCURRENCY = 8
MAX_ENTRIES = 2000


# ── Minimal YAML subset parser ─────────────────────────────────────────────

def _parse_flow_list(text: str) -> list:
    """Parse an inline list like ["MIT", "GPL v3"] or [a, b]."""
    text = text.strip()
    if text.startswith("[") and text.endswith("]"):
        text = text[1:-1]
    out = []
    for part in re.split(r",\s*(?=(?:[^\"']*[\"'][^\"']*[\"'])*[^\"']*$)", text):
        part = part.strip()
        if not part:
            continue
        if (part.startswith('"') and part.endswith('"')) or (part.startswith("'") and part.endswith("'")):
            part = part[1:-1]
        out.append(part)
    return out


def _fold_block_scalars(text: str) -> str:
    """Replace YAML block scalars (key: |, key: >) with single-line values.

    The folded lines are re-emitted as a quoted scalar so the indentation
    parser below never sees them.
    """
    lines = text.splitlines()
    out: list[str] = []
    i = 0
    while i < len(lines):
        line = lines[i]
        m = re.match(r"^(\s*)([\w.-]+)\s*:\s*(\|[-+]?|>[-+]?)\s*(#.*)?$", line)
        if not m:
            out.append(line)
            i += 1
            continue
        indent, key, kind = m.group(1), m.group(2), m.group(3)
        block: list[str] = []
        i += 1
        while i < len(lines):
            nxt = lines[i]
            if nxt.strip() == "":
                block.append("")
                i += 1
                continue
            if len(nxt) - len(nxt.lstrip(" ")) > len(indent):
                block.append(nxt[len(indent):])
                i += 1
                continue
            break
        # Strip trailing blank lines; '>' folds with spaces, '|' keeps newlines.
        while block and block[-1] == "":
            block.pop()
        value = " ".join(part.strip() for part in block) if kind.startswith(">") else "\n".join(block)
        # Emit as a JSON-escaped double-quoted string (valid YAML too) so the
        # re-emitted line stays a single physical line for MAP_ITEM_RE.
        out.append(f"{indent}{key}: {json.dumps(value)}")
    return "\n".join(out)


# A YAML map entry is "key:" or "key: value" — the colon must be followed by
# whitespace or end-of-line. "bioconda::fastp=1.3.6" is therefore a plain
# scalar, not a map, matching real YAML semantics.
MAP_ITEM_RE = re.compile(r'^(?:"([^"]+)"|\'([^\']+)\'|([\w.-]+))\s*:(?:\s+(.*))?$')


def _indent_of(line: str) -> int:
    return len(line) - len(line.lstrip(" "))


def _parse_node(lines: list[str], i: int, indent: int) -> tuple[object, int]:
    """Parse a block at `indent`; returns (node, next_index)."""
    j = i
    while j < len(lines):
        stripped = lines[j].strip()
        if not stripped or stripped.startswith("#") or stripped == "---":
            j += 1
            continue
        break
    if j >= len(lines) or _indent_of(lines[j]) < indent:
        return None, j
    if lines[j].lstrip().startswith("- "):
        return _parse_list(lines, j, indent)
    return _parse_map(lines, j, indent)


def _parse_list(lines: list[str], i: int, indent: int) -> tuple[list, int]:
    """Parse a "- " list block; items are scalars or maps."""
    lst: list = []
    j = i
    while j < len(lines):
        raw = lines[j]
        stripped = raw.strip()
        if not stripped or stripped.startswith("#") or stripped == "---":
            j += 1
            continue
        ind = _indent_of(raw)
        if ind < indent or ind > indent:
            break
        if not stripped.startswith("- "):
            break
        body = stripped[2:]
        m = MAP_ITEM_RE.match(body)
        if m:
            key = (m.group(1) or m.group(2) or m.group(3))
            val = (m.group(4) or "").strip()
            item: dict = {}
            if val and not val.startswith("#"):
                item[key], j = _consume_value(lines, j, ind, val)
                lst.append(item)
            else:
                child, j2 = _child_block(lines, j + 1, ind, {})
                item[key] = child
                lst.append(item)
                j = j2
        else:
            lst.append(_scalar(body))
            j += 1
    return lst, j


def _consume_value(lines: list[str], j: int, indent: int, val: str) -> tuple[object, int]:
    """Return (value, next_index) for a key with an inline scalar.

    YAML folds deeper-indented continuation lines into a plain scalar:

        description: first line
          second line

    becomes "first line second line". Continuation applies only to plain
    (unquoted, non-inline-list) scalars.
    """
    if val.startswith("[") or val.startswith('"') or val.startswith("'"):
        return _scalar(val), j + 1
    parts = [val]
    k = j + 1
    while k < len(lines):
        nxt = lines[k].strip()
        if not nxt or nxt.startswith("#"):
            break
        if _indent_of(lines[k]) > indent:
            parts.append(nxt)
            k += 1
            continue
        break
    return _scalar(" ".join(parts)), k


def _child_block(lines: list[str], i: int, parent_indent: int, empty: dict) -> tuple[dict, int]:
    """Parse the block under a "key:" whose value spans deeper lines.

    Returns (child, next_index). The child's real indent is discovered from
    the next non-blank line (YAML does not fix the indent delta) — when the
    next line is not deeper than the parent, the value is empty.
    """
    k = i
    while k < len(lines) and not lines[k].strip():
        k += 1
    if k >= len(lines) or _indent_of(lines[k]) <= parent_indent:
        return empty, k
    child, j2 = _parse_node(lines, k, _indent_of(lines[k]))
    return (child if child is not None else empty), j2


def _parse_map(lines: list[str], i: int, indent: int) -> tuple[dict, int]:
    """Parse a "key: value" block."""
    node: dict = {}
    j = i
    while j < len(lines):
        raw = lines[j]
        stripped = raw.strip()
        if not stripped or stripped.startswith("#") or stripped == "---":
            j += 1
            continue
        ind = _indent_of(raw)
        if ind < indent or ind > indent:
            break
        m = MAP_ITEM_RE.match(stripped)
        if not m:
            break
        key = (m.group(1) or m.group(2) or m.group(3))
        val = (m.group(4) or "").strip()
        if val and not val.startswith("#"):
            node[key], j = _consume_value(lines, j, ind, val)
        else:
            child, j2 = _child_block(lines, j + 1, ind, {})
            node[key] = child
            j = j2
    return node, j


def load_yaml(text: str) -> dict:
    """Parse the YAML subset used by nf-core meta.yml / environment.yml."""
    text = _fold_block_scalars(text)
    lines = text.splitlines()
    root, _ = _parse_map(lines, 0, 0)
    return root


def _scalar(val: str):
    """Parse a scalar: quoted string, inline list, or plain string."""
    if val.startswith("[") and val.endswith("]"):
        return _parse_flow_list(val)
    if val.startswith('"') and val.endswith('"'):
        try:
            return json.loads(val)
        except ValueError:
            return val[1:-1]
    if val.startswith("'") and val.endswith("'"):
        return val[1:-1]
    return val


# ── Module record extraction ────────────────────────────────────────────────

def first_of(node, *keys, default=None):
    """First present key of a dict (key may also be a list of keys)."""
    for k in keys:
        v = node.get(k)
        if v is not None:
            return v
    return default


def tool_field(tool: dict, *keys) -> str | None:
    """Extract a field from a meta.yml tools[0] entry (handles list values)."""
    val = first_of(tool, *keys)
    if val is None:
        return None
    if isinstance(val, list):
        return ", ".join(str(x) for x in val if x)
    return str(val)


def parse_meta(meta_text: str) -> dict:
    """Extract {n, t, license, biotools_id, doi} from a meta.yml body."""
    rec: dict = {}
    meta = load_yaml(meta_text)
    rec["n"] = str(meta.get("name") or "")
    rec["t"] = str(meta.get("description") or "")
    tools = meta.get("tools") or []
    tool = tools[0] if isinstance(tools, list) and tools else {}
    if isinstance(tool, str):
        tool = {}
    elif tool:
        # tools entries are maps keyed by tool name ("- fastp:" with the
        # fields nested under it) — unwrap the single inner dict.
        inner = [v for v in tool.values() if isinstance(v, dict)]
        if len(tool) == 1 and len(inner) == 1:
            tool = inner[0]
    rec["license"] = tool_field(tool, "licence", "license") or ""
    rec["biotools_id"] = tool_field(tool, "identifier") or ""
    rec["doi"] = tool_field(tool, "doi") or ""
    return rec


def parse_env(env_text: str) -> list:
    """Extract bioconda pins (channel prefix stripped) from environment.yml."""
    env = load_yaml(env_text)
    pins = []
    for dep in env.get("dependencies") or []:
        if isinstance(dep, str):
            m = re.match(r"^bioconda::(.+)$", dep.strip())
            if m:
                pins.append(m.group(1))
    return pins


def fetch_module_yml(path: str, is_env: bool) -> tuple[str, str]:
    """Fetch one YAML file; returns (path, content) or (path, "")."""
    try:
        raw = http_get(RAW_BASE + path, timeout=90, retries=2)
        return path, raw.decode("utf-8", errors="replace")
    except Exception:
        return path, ""


def main() -> int:
    ap = argparse.ArgumentParser(description="Refresh nfcore_modules.jsonl from nf-core/modules")
    ap.add_argument("--out", default=default_out_dir(), help="output dir (default: crates/oxo-flow-ai/src/knowledge/)")
    ap.add_argument("--max-entries", type=int, default=MAX_ENTRIES, help="hard cap on emitted rows")
    args = ap.parse_args()
    out_dir = args.out

    log(f"Fetching nf-core/modules tree from {TREE_URL} ...")
    tree = http_get_json(TREE_URL, timeout=180)
    if tree.get("truncated"):
        log("WARNING: repository tree was truncated by the API")
    paths = sorted(t["path"] for t in tree.get("tree", []) if t.get("type") == "blob")
    meta_paths = [p for p in paths if p.startswith("modules/nf-core/") and p.endswith("/meta.yml")]
    env_paths = set(p for p in paths if p.startswith("modules/nf-core/") and p.endswith("/environment.yml"))
    log(f"  Found {len(meta_paths)} meta.yml files across "
        f"{len(set(p.rsplit('/', 1)[0] for p in meta_paths))} tool directories.")

    fetch_paths = list(meta_paths) + sorted(env_paths)
    log(f"Fetching {len(fetch_paths)} YAML files ({CONCURRENCY}-way concurrent) ...")
    contents: dict[str, str] = {}
    failed = 0
    with ThreadPoolExecutor(max_workers=CONCURRENCY) as ex:
        futs = [ex.submit(fetch_module_yml, p, p.endswith("environment.yml")) for p in fetch_paths]
        for i, fut in enumerate(as_completed(futs), 1):
            path, text = fut.result()
            if text:
                contents[path] = text
            else:
                failed += 1
            if i % 500 == 0 or i == len(futs):
                log(f"  {i}/{len(fetch_paths)} fetched ({failed} failed so far)")

    rows = []
    for meta_path in meta_paths:
        meta_text = contents.get(meta_path)
        if not meta_text:
            continue
        try:
            rec = parse_meta(meta_text)
        except Exception as e:
            log(f"  WARNING: failed to parse {meta_path}: {e}")
            failed += 1
            continue
        if not rec["n"]:
            rec["n"] = meta_path.rsplit("/", 2)[-2] if "/" in meta_path else meta_path
        sibling = meta_path.rsplit("/", 1)[0] + "/environment.yml"
        if sibling in contents:
            try:
                rec["v"] = parse_env(contents[sibling])
            except Exception as e:
                log(f"  WARNING: failed to parse {sibling}: {e}")
                failed += 1
                rec["v"] = []
        else:
            rec["v"] = []
        rows.append(rec)

    rows.sort(key=lambda r: r["n"])
    if len(rows) > args.max_entries:
        log(f"  Trimming {len(rows)} rows to --max-entries={args.max_entries}.")
        rows = rows[: args.max_entries]

    if failed:
        log(f"WARNING: {failed} files failed to fetch/parse; emitted {len(rows)} rows.")
    path, count, size = write_jsonl(out_dir, "nfcore_modules.jsonl", rows)
    log(f"Wrote {count} nf-core modules to {path} ({size} bytes).")
    update_meta(out_dir, "nfcore_modules", {"count": count, "url": "https://github.com/nf-core/modules"})
    return 0 if failed == 0 else 1


if __name__ == "__main__":
    sys.exit(main())
