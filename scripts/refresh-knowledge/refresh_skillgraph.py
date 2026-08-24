#!/usr/bin/env python3
"""Refresh pipeline_graph.jsonl + skillgraph_docs.jsonl from the live
Pipette SkillGraph MCP server.

Drives the JSON-RPC 2.0 MCP endpoint (https://skillgraph.pipette.bio/mcp):

    initialize → tools/list → tools/call list_skills
    → tools/call get_skill ×N → tools/call get_transitions ×N
    → tools/call get_graph_stats

Outputs:
  - pipeline_graph.jsonl — EXISTING schema, unchanged keys:
        nodes {"i", "n", "t", "o"}  (id, name, tools-count-as-string,
                                     overview truncated to 100 chars)
        edges {"f", "t", "d", "p"}  (from, to, data-types, papers)
    The richer MCP data is reduced to this schema on purpose — the Rust
    loader (knowledge/pipeline_graph.rs) is left untouched.
  - skillgraph_docs.jsonl — {"i", "doc"} with each skill's full SKILL.md
    documentation (the "# Full Documentation" section of get_skill) for a
    later doc layer. Not embedded yet.
  - knowledge_meta.json — "skillgraph" section with graph stats.

Idempotent: re-running replaces the outputs atomically.
"""

import argparse
import json
import os
import re
import sys
import time
from concurrent.futures import ThreadPoolExecutor, as_completed

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from common import default_out_dir, http_post_json, log, update_meta, write_jsonl  # noqa: E402

MCP_URL = "https://skillgraph.pipette.bio/mcp"
OVERVIEW_CAP = 100
CONCURRENCY = 4

LINE_RE = re.compile(r"^# Skill: (.+?)\s*$", re.MULTILINE)
TOOL_LINE_RE = re.compile(r"^-\s+\S", re.MULTILINE)
UP_EDGE_RE = re.compile(
    r"^- \*\*(?P<name>.*?)\*\* `(?P<fid>[^`]+)` → `(?P<data>[^`]+)` — (?P<papers>\d+) papers",
    re.MULTILINE,
)
DOWN_EDGE_RE = re.compile(
    r"^- \*\*(?P<name>.*?)\*\* `(?P<tid>[^`]+)` ← `(?P<data>[^`]+)` — (?P<papers>\d+) papers",
    re.MULTILINE,
)


class McpClient:
    """Minimal MCP client for the SkillGraph server."""

    def __init__(self, url: str):
        self.url = url
        self._id = 0

    def call(self, method: str, params: dict | None = None) -> dict:
        self._id += 1
        payload = {"jsonrpc": "2.0", "id": self._id, "method": method}
        if params is not None:
            payload["params"] = params
        resp = http_post_json(self.url, payload)
        if "error" in resp:
            raise RuntimeError(f"MCP {method} error: {resp['error']}")
        return resp.get("result", {})

    def tools_call(self, name: str, arguments: dict | None = None) -> str:
        result = self.call("tools/call", {"name": name, "arguments": arguments or {}})
        for block in result.get("content", []):
            if block.get("type") == "text":
                return block.get("text", "")
        return ""


def parse_list_skills(text: str) -> list[str]:
    """Extract skill ids from the list_skills markdown table."""
    ids = re.findall(r"^\| `([^`]+)` \|", text, re.MULTILINE)
    return ids


def parse_get_skill(text: str) -> tuple[dict, str]:
    """Parse one get_skill response → (node dict, full documentation)."""
    node = {"i": "", "n": "", "t": "0", "o": ""}
    doc = ""
    m = LINE_RE.search(text)
    if m:
        node["n"] = m.group(1).strip()
    lines = text.splitlines()
    in_tools = False
    tools = 0
    for line in lines:
        if line.startswith("## Tools"):
            in_tools = True
            continue
        if line.startswith("## ") and line != "## Tools":
            in_tools = False
        if in_tools and line.startswith("- "):
            tools += 1
    node["t"] = str(tools)
    # Overview = the line IMMEDIATELY after the title when it is a plain
    # paragraph (not blank, not a section header, not a Tools bullet).
    # Skills whose get_skill output starts with a section (e.g. "## I/O")
    # have no overview (""), mirroring the shipped pipeline_graph.jsonl.
    for idx, line in enumerate(lines):
        if LINE_RE.match(line) and idx + 1 < len(lines):
            nxt = lines[idx + 1]
            if nxt.strip() and not nxt.startswith("## ") and not nxt.startswith("- "):
                node["o"] = nxt.strip()[:OVERVIEW_CAP].rstrip()
            break
    # Full documentation section (may be absent for minimal skills).
    idx = re.search(r"^# Full Documentation\s*$", text, re.MULTILINE)
    if idx:
        doc = text[idx.end():].strip()
    return node, doc


def parse_transitions(text: str, skill_id: str) -> list[dict]:
    """Parse one get_transitions response → edge records {f, t, d, p}.

    Upstream entries name the feeding skill only, downstream entries the
    consuming skill only — the missing endpoint is the queried skill itself.
    """
    edges = []
    for m in UP_EDGE_RE.finditer(text):
        edges.append({"f": m.group("fid"), "t": skill_id,
                      "d": m.group("data").strip(), "p": int(m.group("papers"))})
    for m in DOWN_EDGE_RE.finditer(text):
        edges.append({"f": skill_id, "t": m.group("tid"),
                      "d": m.group("data").strip(), "p": int(m.group("papers"))})
    return edges


def parse_graph_stats(text: str) -> dict:
    """Parse get_graph_stats → {skills, transitions, papers}."""
    def grab(pattern: str, commas: bool = False) -> int:
        m = re.search(pattern, text)
        if not m:
            return 0
        return int(m.group(1).replace(",", "") if commas else m.group(1))

    # The stats markdown bolds the numbers ("**78**") — skip the asterisks.
    return {
        "skills": grab(r"Total skills:\**\s*(\d+)"),
        "transitions": grab(r"Total transitions:\**\s*(\d+)"),
        "papers": grab(r"Papers referenced in edges:\**\s*([\d,]+)", commas=True),
    }


def fetch_one(client: McpClient, skill_id: str) -> tuple[dict | None, str, list[dict]]:
    """Fetch get_skill + get_transitions for one id (with retries inside http_post_json)."""
    try:
        skill_text = client.tools_call("get_skill", {"skill_id": skill_id})
        node, doc = parse_get_skill(skill_text)
        if not node["n"]:
            node["n"] = skill_id
        node["i"] = skill_id
        trans_text = client.tools_call("get_transitions", {"skill_id": skill_id})
        edges = parse_transitions(trans_text, skill_id)
        return node, doc, edges
    except Exception as e:
        log(f"  ERROR fetching '{skill_id}': {e}")
        return None, "", []


def main() -> int:
    ap = argparse.ArgumentParser(description="Refresh pipeline_graph.jsonl + skillgraph_docs.jsonl from the SkillGraph MCP")
    ap.add_argument("--out", default=default_out_dir(), help="output dir (default: crates/oxo-flow-ai/src/knowledge/)")
    ap.add_argument("--url", default=MCP_URL, help="SkillGraph MCP endpoint")
    args = ap.parse_args()
    out_dir = args.out

    client = McpClient(args.url)
    log(f"Initializing MCP session at {args.url} ...")
    client.call("initialize", {
        "protocolVersion": "2024-11-05",
        "capabilities": {},
        "clientInfo": {"name": "oxo-flow-knowledge-refresh", "version": "1.0"},
    })
    tools = client.call("tools/list")
    tool_names = [t.get("name") for t in tools.get("tools", [])]
    needed = {"list_skills", "get_skill", "get_transitions", "get_graph_stats"}
    missing = needed - set(tool_names)
    if missing:
        log(f"ERROR: MCP server missing tools {sorted(missing)}")
        return 2

    list_text = client.tools_call("list_skills")
    ids = parse_list_skills(list_text)
    log(f"list_skills returned {len(ids)} skill ids.")

    nodes: list[dict] = []
    docs: list[dict] = []
    edges: list[dict] = []
    failed = 0
    with ThreadPoolExecutor(max_workers=CONCURRENCY) as ex:
        futs = {ex.submit(fetch_one, client, i): i for i in ids}
        for i, fut in enumerate(as_completed(futs), 1):
            skill_id = futs[fut]
            node, doc, trans = fut.result()
            if node is None:
                failed += 1
                continue
            nodes.append(node)
            if doc:
                docs.append({"i": skill_id, "doc": doc})
            edges.extend(trans)
            if i % 20 == 0 or i == len(ids):
                log(f"  {i}/{len(ids)} skills fetched.")

    # Deduplicate edges by (from, to) — each transition appears once as the
    # source's downstream and once as the target's upstream.
    seen: set[tuple[str, str]] = set()
    unique: list[dict] = []
    for e in edges:
        key = (e["f"], e["t"])
        if key in seen:
            continue
        seen.add(key)
        unique.append(e)
    nodes.sort(key=lambda n: n["i"])
    unique.sort(key=lambda e: (e["f"], e["t"]))
    docs.sort(key=lambda d: d["i"])

    node_ids = {n["i"] for n in nodes}
    dangling = [e for e in unique if e["f"] not in node_ids or e["t"] not in node_ids]
    if dangling:
        log(f"WARNING: {len(dangling)} edges reference unknown nodes: "
            f"{[(e['f'], e['t']) for e in dangling[:5]]}")
        unique = [e for e in unique if e["f"] in node_ids and e["t"] in node_ids]

    path, ncount, nsize = write_jsonl(out_dir, "pipeline_graph.jsonl", nodes + unique)
    log(f"Wrote {ncount} records ({len(nodes)} nodes, {len(unique)} edges) to {path} ({nsize} bytes).")
    dpath, dcount, dsize = write_jsonl(out_dir, "skillgraph_docs.jsonl", docs)
    log(f"Wrote {dcount} skill docs to {dpath} ({dsize} bytes).")
    update_meta(
        out_dir,
        "skillgraph_docs",
        {
            "count": dcount,
            "url": args.url,
            "description": "full SKILL.md docs per skill (reserved for a later doc layer)",
        },
    )

    stats = parse_graph_stats(client.tools_call("get_graph_stats"))
    log(f"Graph stats from server: {stats}")
    update_meta(
        out_dir,
        "pipeline_graph",
        {
            # The meta contract counts non-empty file lines (drift guard):
            # nodes + edges.
            "count": ncount,
            "url": args.url,
            "stats": {
                "skills": stats.get("skills"),
                "transitions": stats.get("transitions"),
                "papers": stats.get("papers"),
                "nodes": len(nodes),
                "edges": len(unique),
            },
        },
    )
    return 0 if failed == 0 else 1


if __name__ == "__main__":
    sys.exit(main())
