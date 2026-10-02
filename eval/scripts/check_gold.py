#!/usr/bin/env python3
"""Consistency checks for the eval gold set (issue #172).

Usage:
  python3 eval/scripts/check_gold.py [--strict] [--repo-root DIR]

Hard findings exit 1 — they invalidate scoring or evidence and must be
resolved before review continue; warnings exit 1 only with --strict.

Hard checks
  negative-name  a negative sample's fabricated tool resolves in the
                 knowledge base: the item is no longer negative (a grounded
                 model would find it), and no correct answer can reject it
  tool-missing   a reviewed tool row's expected_tool is absent from every
                 knowledge-base table, so a grounded answer cannot name it
  version-drift  a reviewed tool row's gold version no longer matches the
                 embedded knowledge base (approvals are snapshot-scoped;
                 the 2026-10-01 refresh drifted 7 rows this way)
  provenance     a reviewed non-negative tool row has no provenance URL,
                 or a row claims "URL resolves" without one
  dag-edge       a workflow row's declared DAG edge does not exist in the
                 engine's dependency graph for its reference file. Local
                 gallery references are read in place; community references
                 (`oxo-flow-community/...`) are fetched from
                 raw.githubusercontent.com (disable with --no-fetch-community;
                 fetch failures are warnings, not hard failures)

Warnings
  version-intent expected_version is set although the query never asks for
                 a version (strip-vs-keep decision pending, see #172)

Stdlib only.
"""

import argparse
import csv
import json
import os
import re
import sys
import tempfile
import tomllib
import urllib.error
import urllib.request

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import common  # noqa: E402
import runner  # noqa: E402

REVIEWED = ("approved", "corrected")
COMMUNITY_BASE = "https://raw.githubusercontent.com/oxo-flow-community/{repo}/main/{path}"


def load_rows(repo_root, layer):
    path = os.path.join(repo_root, "eval", "gold", f"{layer}.csv")
    with open(path, newline="", encoding="utf-8") as fh:
        return list(csv.DictReader(fh))


def kb_name_norms():
    return {common.norm(name) for name in common.known_tool_names() if common.norm(name)}


def kb_version_map():
    """Tool name (normalized) -> version from the tables the gold derives from."""
    versions = {}
    for file_name in ("bioconda_tools.jsonl", "commercial_tools.jsonl"):
        for row in common.load_jsonl(file_name):
            key = common.norm(row.get("n", ""))
            if key and row.get("v") and key not in versions:
                versions[key] = row["v"]
    return versions


def check_negative_names(tool_rows, kb_norms):
    findings, checked = [], 0
    for row in tool_rows:
        if row.get("negative_sample") != "1":
            continue
        name = common.fake_tool_name(row.get("query", ""))
        if not name:
            findings.append(f"{row['id']}: could not extract the fabricated name from the query")
            continue
        checked += 1
        if common.norm(name) in kb_norms:
            findings.append(f"{row['id']}: fabricated name {name!r} now resolves in the knowledge base")
    return findings, checked


def check_expected_tools(tool_rows, kb_norms):
    """Reviewed tool rows must expect a tool that exists in the knowledge base."""
    findings, checked = [], 0
    for row in tool_rows:
        if row.get("negative_sample") == "1" or row.get("review_status") not in REVIEWED:
            continue
        tool = row.get("expected_tool", "")
        if not tool:
            continue
        checked += 1
        if common.norm(tool) not in kb_norms:
            findings.append(f"{row['id']}: expected_tool {tool!r} is absent from every knowledge-base table")
    return findings, checked


def check_version_drift(tool_rows, kb_versions):
    findings, checked = [], 0
    for row in tool_rows:
        if row.get("negative_sample") == "1" or row.get("review_status") not in REVIEWED:
            continue
        tool = row.get("expected_tool", "")
        gold = row.get("expected_version", "")
        if not tool or not gold:
            continue
        kb = kb_versions.get(common.norm(tool))
        if kb is None:
            continue
        checked += 1
        if kb != gold:
            findings.append(f"{row['id']}: {tool} gold={gold} kb={kb}")
    return findings, checked


def check_provenance(tool_rows):
    findings, checked = [], 0
    for row in tool_rows:
        url = (row.get("provenance_url") or "").strip()
        comment = (row.get("review_comment") or "").lower()
        negative = row.get("negative_sample") == "1"
        if not negative and row.get("review_status") in REVIEWED:
            checked += 1
            if not url:
                findings.append(f"{row['id']}: reviewed row without a provenance URL")
        if not url and "url resolves" in comment:
            findings.append(f"{row['id']}: review_comment claims a URL resolves but provenance_url is empty")
    return findings, checked


MISSING_REFERENCE_RE = re.compile(r"parse error in \./([^\s:]+)")


def reference_graph(path, oxo_flow_bin):
    """Engine DAG of a reference workflow; returns (nodes, edges, error)."""
    code, out, err = common.oxo_flow_cmd(
        oxo_flow_bin, ["graph", os.path.basename(path), "-f", "dot"], cwd=os.path.dirname(path)
    )
    if code != 0:
        return set(), set(), err.strip() or f"exit {code}"
    nodes, edges = runner.parse_dot_graph(out)
    return nodes, edges, None


def _fetch(url, dest):
    os.makedirs(os.path.dirname(dest), exist_ok=True)
    with urllib.request.urlopen(url, timeout=30) as resp:
        data = resp.read()
    with open(dest, "wb") as fh:
        fh.write(data)


def materialize_community_reference(ref_repo, ref_file, workdir):
    """Fetch a community reference and its [[include]] tree; (path, error).

    The engine resolves [[include]] paths relative to the referencing file,
    so the mirrored directory layout must preserve the repo's structure.
    """
    root = os.path.join(workdir, ref_repo)
    pending, seen = [ref_file], set()
    while pending:
        rel = pending.pop()
        if rel in seen:
            continue
        seen.add(rel)
        url = COMMUNITY_BASE.format(repo=ref_repo, path=rel)
        dest = os.path.join(root, rel)
        try:
            _fetch(url, dest)
        except (urllib.error.URLError, OSError, ValueError) as exc:
            return None, f"{url}: {exc}"
        try:
            data = tomllib.loads(open(dest, encoding="utf-8").read())
        except (tomllib.TOMLDecodeError, UnicodeDecodeError):
            continue  # engine (and the graph step) will surface parse errors
        for entry in data.get("include", []):
            included = entry.get("path", "")
            if included:
                pending.append(included)
    return os.path.join(root, ref_file), None


MAX_FIXTURE_ROUNDS = 8


def check_workflow_edges(workflow_rows, repo_root, graph_source, fetch_community=False, workdir=None):
    """graph_source(path) -> (nodes, edges, error) — the engine graph of a reference.

    Returns (findings, checked, skipped, fetch_warnings). Community references
    are materialized from raw.githubusercontent.com when `fetch_community` is
    set, including fixture files the engine reads at parse time (samplesheets,
    pairs TSVs); fetch failures are warnings (network), edge mismatches are
    findings. Step mapping uses the engine's node labels, which include rules
    from [[include]]d modules — parsing only the main file would report every
    namespaced step as missing.
    """
    findings, checked, skipped, fetch_warnings = [], 0, 0, []
    for row in workflow_rows:
        ref_repo = row.get("reference_repo", "")
        ref_file = row.get("reference_file", "")
        if not ref_file:
            skipped += 1
            continue
        local = ref_repo.startswith("examples/")
        if local:
            path = os.path.join(repo_root, ref_repo, ref_file)
            if not os.path.isfile(path):
                skipped += 1
                continue
        elif fetch_community and workdir:
            path, error = materialize_community_reference(ref_repo, ref_file, workdir)
            if error:
                fetch_warnings.append(f"{row['id']}: could not fetch community reference ({error})")
                continue
        else:
            skipped += 1
            continue
        nodes, edges, error = graph_source(path)
        rounds = 0
        while error and not local and rounds < MAX_FIXTURE_ROUNDS:
            match = MISSING_REFERENCE_RE.search(error)
            if not match:
                break
            rel = match.group(1)
            if ".." in rel or rel.startswith("/"):
                break
            try:
                _fetch(
                    COMMUNITY_BASE.format(repo=ref_repo, path=rel),
                    os.path.join(workdir, ref_repo, rel),
                )
            except (urllib.error.URLError, OSError, ValueError) as exc:
                error = f"{error}; fetching {rel} failed: {exc}"
                break
            nodes, edges, error = graph_source(path)
            rounds += 1
        if error:
            findings.append(f"{row['id']}: engine graph failed for reference {ref_repo}/{ref_file} ({error})")
            continue
        try:
            expected_edges = json.loads(row.get("expected_dag_edges") or "[]")
            expected_steps = json.loads(row.get("expected_steps") or "[]")
        except json.JSONDecodeError:
            findings.append(f"{row['id']}: invalid JSON in expected_dag_edges/expected_steps")
            continue
        checked += 1
        name_map = common.map_expected_steps(expected_steps, sorted(nodes))
        for src, dst in expected_edges:
            mapped_src, mapped_dst = name_map.get(src), name_map.get(dst)
            if mapped_src is None or mapped_dst is None:
                findings.append(f"{row['id']}: declared step missing from reference ({src} -> {dst})")
            elif (mapped_src, mapped_dst) not in edges:
                findings.append(
                    f"{row['id']}: declared DAG edge {src} -> {dst} not in reference {ref_repo}/{ref_file}"
                )
    return findings, checked, skipped, fetch_warnings


def check_version_intent(tool_rows):
    warnings = []
    for row in tool_rows:
        if row.get("negative_sample") == "1":
            continue
        if row.get("expected_version") and "version" not in (row.get("query") or "").lower():
            warnings.append(row["id"])
    return warnings


def run_checks(repo_root, oxo_flow_bin, fetch_community=True):
    tool_rows = load_rows(repo_root, "tool")
    workflow_rows = load_rows(repo_root, "workflow")
    report = {"hard": [], "warnings": [], "summary": []}

    findings, checked = check_negative_names(tool_rows, kb_name_norms())
    report["hard"] += [f"[negative-name] {f}" for f in findings]
    report["summary"].append(f"negative names checked: {checked}")

    findings, checked = check_expected_tools(tool_rows, kb_name_norms())
    report["hard"] += [f"[tool-missing] {f}" for f in findings]
    report["summary"].append(f"expected tools checked: {checked}")

    findings, checked = check_version_drift(tool_rows, kb_version_map())
    report["hard"] += [f"[version-drift] {f}" for f in findings]
    report["summary"].append(f"version pins checked: {checked}")

    findings, checked = check_provenance(tool_rows)
    report["hard"] += [f"[provenance] {f}" for f in findings]
    report["summary"].append(f"reviewed tool rows with a provenance check: {checked}")

    graph_source = lambda path: reference_graph(path, oxo_flow_bin)  # noqa: E731
    with tempfile.TemporaryDirectory(prefix="oxo-gold-") as workdir:
        findings, checked, skipped, fetch_warnings = check_workflow_edges(
            workflow_rows, repo_root, graph_source, fetch_community=fetch_community, workdir=workdir
        )
    report["hard"] += [f"[dag-edge] {f}" for f in findings]
    report["summary"].append(f"workflow references checked: {checked} ({skipped} skipped)")
    report["warnings"] += [f"[community-fetch] {w}" for w in fetch_warnings]

    warnings = check_version_intent(tool_rows)
    report["warnings"] += [f"[version-intent] {row_id}" for row_id in warnings]

    report["rows"] = {"tool": len(tool_rows), "workflow": len(workflow_rows)}
    return report


def print_report(report):
    print("eval gold consistency (#172)")
    print(f"  rows: tool={report['rows']['tool']} workflow={report['rows']['workflow']}")
    for line in report["summary"]:
        print(f"  checked: {line}")
    print(f"hard findings: {len(report['hard'])}")
    for finding in report["hard"]:
        print(f"  {finding}")
    if any(f.startswith("[version-drift]") for f in report["hard"]):
        print(
            "  hint: a knowledge refresh moved versions — resync eval/gold/*.csv in the same "
            "change (see #172); do not merge with silent drift"
        )
    print(f"warnings: {len(report['warnings'])}")
    for warning in report["warnings"][:10]:
        print(f"  {warning}")
    if len(report["warnings"]) > 10:
        print(f"  ... and {len(report['warnings']) - 10} more")


def main(argv=None):
    parser = argparse.ArgumentParser(description="Gold-set consistency checks (issue #172)")
    parser.add_argument("--strict", action="store_true", help="treat warnings as failures")
    parser.add_argument("--repo-root", default=common.REPO_ROOT)
    parser.add_argument(
        "--oxo-flow",
        default=None,
        help="engine binary for the DAG-edge check (default: <repo>/target/debug/oxo-flow)",
    )
    parser.add_argument(
        "--no-fetch-community",
        action="store_true",
        help="skip the 22 community references instead of fetching them from GitHub",
    )
    args = parser.parse_args(argv)
    oxo_flow_bin = os.path.abspath(args.oxo_flow) if args.oxo_flow else os.path.join(
        args.repo_root, "target", "debug", "oxo-flow"
    )
    if not os.path.isfile(oxo_flow_bin):
        print(
            f"error: engine binary not found at {oxo_flow_bin}; "
            "run `cargo build --workspace` first or pass --oxo-flow",
            file=sys.stderr,
        )
        return 2
    report = run_checks(args.repo_root, oxo_flow_bin, fetch_community=not args.no_fetch_community)
    print_report(report)
    if report["hard"]:
        return 1
    if args.strict and report["warnings"]:
        return 1
    return 0


if __name__ == "__main__":
    sys.exit(main())
