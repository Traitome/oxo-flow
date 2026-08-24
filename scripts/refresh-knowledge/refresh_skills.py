#!/usr/bin/env python3
"""Refresh skills_index.jsonl — the embedded bioSkills agent-skill library.

Downloads the GPTomics/bioSkills repository tree (GitHub git-trees API, one
request) and every SKILL.md file (raw.githubusercontent, 8-way concurrent),
then re-derives the compact index records:

    {"name", "description", "domain", "tool_type", "primary_tool", "preview"}

The transform mirrors the shipped skills_index.jsonl exactly:

  - name        = SKILL.md frontmatter `name` (fallback: directory name)
  - description = frontmatter `description`, truncated to 300 chars
  - domain      = top-level repository directory
  - tool_type   = frontmatter `tool_type` ("" when absent)
  - primary_tool= frontmatter `primary_tool` ("" when absent)
  - preview     = body after the frontmatter, newlines folded to spaces,
                  truncated to 250 chars

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

TREE_URL = "https://api.github.com/repos/GPTomics/bioSkills/git/trees/main?recursive=1"
RAW_BASE = "https://raw.githubusercontent.com/GPTomics/bioSkills/main/"
DESC_CAP = 300
PREVIEW_CAP = 250
CONCURRENCY = 8


def fetch_skill_md(path: str) -> tuple[str, str]:
    """Fetch one SKILL.md; returns (path, content) or (path, "") on failure."""
    try:
        raw = http_get(RAW_BASE + path, timeout=90, retries=2)
        return path, raw.decode("utf-8", errors="replace")
    except Exception:
        return path, ""


def parse_frontmatter(text: str) -> tuple[dict, str]:
    """Split YAML frontmatter (--- delimited) from the markdown body.

    Frontmatter is parsed with a permissive key: value scanner (the subset
    used by SKILL.md headers); unknown keys are ignored. Returns
    (frontmatter dict, body-after-frontmatter).
    """
    if not text.startswith("---"):
        return {}, text
    m = re.match(r"^---\n(.*?)\n---\n", text, re.DOTALL)
    if not m:
        return {}, text
    fm = {}
    for line in m.group(1).splitlines():
        kv = re.match(r"^([A-Za-z_][A-Za-z0-9_-]*)\s*:\s*(.*)$", line)
        if kv:
            fm[kv.group(1)] = kv.group(2).strip().strip('"').strip("'")
    body = text[m.end():]
    return fm, body


def build_record(path: str, text: str) -> dict | None:
    """Derive one compact index record from a SKILL.md file."""
    fm, body = parse_frontmatter(text)
    dirname = os.path.basename(os.path.dirname(path))
    name = fm.get("name") or dirname
    if not name:
        return None
    return {
        "name": name,
        "description": (fm.get("description") or "")[:DESC_CAP],
        "domain": path.split("/", 1)[0],
        "tool_type": fm.get("tool_type") or "",
        "primary_tool": fm.get("primary_tool") or "",
        "preview": body.replace("\n", " ")[:PREVIEW_CAP],
    }


def main() -> int:
    ap = argparse.ArgumentParser(description="Refresh skills_index.jsonl from GPTomics/bioSkills")
    ap.add_argument("--out", default=default_out_dir(), help="output dir (default: crates/oxo-flow-ai/src/knowledge/)")
    args = ap.parse_args()
    out_dir = args.out

    log(f"Fetching bioSkills tree from {TREE_URL} ...")
    tree = http_get_json(TREE_URL, timeout=120)
    if tree.get("truncated"):
        log("WARNING: repository tree was truncated by the API")
    skill_paths = sorted(
        t["path"] for t in tree.get("tree", []) if t.get("path", "").endswith("/SKILL.md")
    )
    log(f"  Found {len(skill_paths)} SKILL.md files.")

    log(f"Fetching {len(skill_paths)} SKILL.md files ({CONCURRENCY}-way concurrent) ...")
    contents: dict[str, str] = {}
    failed = 0
    with ThreadPoolExecutor(max_workers=CONCURRENCY) as ex:
        futs = [ex.submit(fetch_skill_md, p) for p in skill_paths]
        for i, fut in enumerate(as_completed(futs), 1):
            path, text = fut.result()
            if text:
                contents[path] = text
            else:
                failed += 1
            if i % 100 == 0 or i == len(futs):
                log(f"  {i}/{len(futs)} fetched ({failed} failed so far)")

    records = []
    for path in skill_paths:
        text = contents.get(path)
        if text:
            rec = build_record(path, text)
            if rec:
                records.append(rec)
    records.sort(key=lambda r: r["name"])

    if failed:
        log(f"WARNING: {failed}/{len(skill_paths)} SKILL.md files failed to fetch; emitted {len(records)} records.")
    path, count, size = write_jsonl(out_dir, "skills_index.jsonl", records)
    log(f"Wrote {count} skills to {path} ({size} bytes).")
    domains = len({r["domain"] for r in records})
    update_meta(
        out_dir,
        "skills_index",
        {"count": count, "url": "https://github.com/GPTomics/bioSkills", "domains": domains},
    )
    return 0 if failed == 0 else 1


if __name__ == "__main__":
    sys.exit(main())
