#!/usr/bin/env python3
"""Judge captured AI outputs against the gold CSVs.

Usage:
  python3 eval/scripts/runner.py tool --captures outputs/tool_answers.csv
  python3 eval/scripts/runner.py rule --captures outputs/rules --oxo-flow <bin>
  python3 eval/scripts/runner.py workflow --captures outputs/workflows --oxo-flow <bin>

Options (all layers): --out results.csv, --include-unreviewed.
Scores are 0..1 per sub-metric; `overall` is the mean of the applicable
sub-metrics for the row. Stdlib only.
"""

import argparse
import csv
import json
import os
import re
import sys
import tomllib

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import common  # noqa: E402

NOT_FOUND_RE = re.compile(
    r"not found|no such|does not exist|doesn't exist|no tool|unable to find|"
    r"cannot find|unknown tool|i don'?t know of|no known",
    re.IGNORECASE,
)


# ── Tool layer ──────────────────────────────────────────────────────────────

def judge_tool(gold_rows, captures_path):
    answers = {}
    with open(captures_path, newline="", encoding="utf-8") as fh:
        for row in csv.DictReader(fh):
            answers[row["id"]] = row["answer"]

    results = []
    for row in gold_rows:
        answer = answers.get(row["id"])
        if answer is None:
            print(f"WARN: no capture for {row['id']} — skipped")
            continue
        scores = {}
        if row["expected_tool"]:
            scores["name_match"] = 1.0 if common.name_present(row["expected_tool"], answer) else 0.0
        if row["expected_version"]:
            scores["version_match"] = (
                1.0 if re.sub(r"\s", "", row["expected_version"]) in re.sub(r"\s", "", answer)
                else 0.0
            )
        if row["negative_sample"] == "1":
            scores["no_hallucination"] = 1.0 if NOT_FOUND_RE.search(answer) else 0.0
        scores["overall"] = round(sum(scores.values()) / len(scores), 3) if scores else 0.0
        results.append({"id": row["id"], "layer": "tool", **scores, "answer_len": len(answer)})
    return results


# ── Shared TOML helpers ─────────────────────────────────────────────────────

def load_generated(path):
    """Parse a generated .oxoflow file; returns (text, rules, err)."""
    with open(path, encoding="utf-8") as fh:
        text = fh.read()
    try:
        data = tomllib.loads(text)
        return text, data.get("rules", []), None
    except tomllib.TOMLDecodeError as exc:
        return text, [], str(exc)


def find_pinned_version(text, tool):
    """Extract a `tool=X.Y.Z` pin (also `bioconda::tool=X.Y.Z`)."""
    pattern = re.compile(
        r"(?:bioconda::)?" + re.escape(tool) + r"\s*=\s*(\d+(?:\.\d+)*[a-zA-Z0-9.-]*)",
        re.IGNORECASE,
    )
    m = pattern.search(text)
    return m.group(1) if m else None


def find_generated_file(captures_dir, item_id):
    """Find the .oxoflow file for an item (direct file or template dir)."""
    direct = os.path.join(captures_dir, f"{item_id}.oxoflow")
    if os.path.isfile(direct):
        return direct
    folder = os.path.join(captures_dir, item_id)
    if os.path.isdir(folder):
        files = [
            os.path.join(folder, name)
            for name in os.listdir(folder)
            if name.endswith(".oxoflow")
        ]
        if files:
            return files[0]
    return None


def memory_to_mb(value):
    m = re.fullmatch(r"(\d+(?:\.\d+)?)\s*([GMK])i?B?", value.strip(), re.IGNORECASE)
    if not m:
        return None
    number, unit = float(m.group(1)), m.group(2).upper()
    return int(number * {"G": 1024, "M": 1, "K": 1 / 1024}[unit])


def parse_resources(rule):
    res = rule.get("resources", {})
    return res.get("threads"), res.get("memory")


# ── Rule layer ──────────────────────────────────────────────────────────────

def judge_rule(gold_rows, captures_dir, oxo_flow_bin):
    versions = common.bioconda_versions()
    known_pins = common.known_version_pins(versions)
    results = []
    for row in gold_rows:
        path = find_generated_file(captures_dir, row["id"])
        if path is None:
            print(f"WARN: no capture for {row['id']} — skipped")
            continue
        text, rules, parse_err = load_generated(path)
        rule = rules[0] if rules else {}
        shell = rule.get("shell", "") if rule else text
        scores = {}

        # Tool present in the shell / workflow text.
        scores["tool_present"] = (
            1.0 if common.name_present(row["expected_tool"], shell + " " + text) else 0.0
        )

        # Version pinned to a REAL version: either the gold reference pin
        # (from the gallery/community env yaml) or a version the embedded
        # knowledge base attests. The KB stores only the latest bioconda
        # version, so a correct-but-older pin (e.g. fastp 0.23.4) must not
        # be penalized against the KB latest (fastp 1.3.6).
        if row["expected_version"]:
            pinned = find_pinned_version(text, row["expected_tool"])
            if pinned is None:
                scores["version_pinned"] = 0.0
            else:
                known = known_pins.get(row["expected_tool"], set())
                scores["version_pinned"] = (
                    1.0 if pinned in known or pinned == row["expected_version"] else 0.0
                )

        # Key parameters present in the shell block.
        try:
            params = json.loads(row["expected_key_params"])
        except json.JSONDecodeError:
            params = []
        if params:
            scores["key_params"] = round(
                sum(1 for p in params if re.search(re.escape(p), shell, re.IGNORECASE)) / len(params),
                3,
            )

        # Inputs/outputs declared (wildcard-normalized).
        declared_in = {common.wildcard_norm(p) for p in rule.get("input", [])} if rule else set()
        declared_out = {common.wildcard_norm(p) for p in rule.get("output", [])} if rule else set()
        try:
            expected_in = [common.wildcard_norm(p) for p in json.loads(row["expected_inputs"])]
            expected_out = [common.wildcard_norm(p) for p in json.loads(row["expected_outputs"])]
        except json.JSONDecodeError:
            expected_in, expected_out = [], []
        io_pairs = len(expected_in) + len(expected_out)
        if io_pairs:
            io_hits = sum(1 for p in expected_in if p in declared_in) + sum(
                1 for p in expected_out if p in declared_out
            )
            scores["io_declared"] = round(io_hits / io_pairs, 3)

        # Structural validity via the engine itself.
        code, _, _ = common.oxo_flow_cmd(
            oxo_flow_bin, ["validate", os.path.basename(path)], cwd=os.path.dirname(path)
        )
        scores["validate_pass"] = 1.0 if code == 0 else 0.0

        # Resource sanity against the gold range.
        try:
            resource_range = json.loads(row["resource_range"])
        except json.JSONDecodeError:
            resource_range = {}
        threads, memory = parse_resources(rule) if rule else (None, None)
        if threads is None and memory is None:
            scores["resources_ok"] = 0.5  # unstated: neutral, documented heuristic
        else:
            ok = True
            if threads is not None and isinstance(threads, int):
                ok = ok and resource_range.get("threads_min", 1) <= threads <= resource_range.get("threads_max", 128)
            if memory is not None:
                mem_mb = memory_to_mb(memory)
                ok = ok and (mem_mb is None or mem_mb <= resource_range.get("memory_max_mb", 262144))
            scores["resources_ok"] = 1.0 if ok else 0.0

        scores["overall"] = round(sum(scores.values()) / len(scores), 3) if scores else 0.0
        results.append(
            {"id": row["id"], "layer": "rule", **scores, "parse_err": parse_err or ""}
        )
    return results


# ── Workflow layer ──────────────────────────────────────────────────────────

def generated_edges(rules):
    """Infer DAG edges: B consumes a path A declared as output (wildcard-norm)."""
    edges = []
    for src in rules:
        src_name = src.get("name", "")
        src_out = {common.wildcard_norm(p) for p in src.get("output", [])}
        for dst in rules:
            if dst is src:
                continue
            dst_in = {common.wildcard_norm(p) for p in dst.get("input", [])}
            if src_out & dst_in:
                edges.append((src_name, dst.get("name", "")))
    return edges


def judge_workflow(gold_rows, captures_dir, oxo_flow_bin):
    results = []
    for row in gold_rows:
        path = find_generated_file(captures_dir, row["id"])
        if path is None:
            print(f"WARN: no capture for {row['id']} — skipped")
            continue
        text, rules, parse_err = load_generated(path)
        names = [r.get("name", "") for r in rules]
        scores = {}

        code, _, _ = common.oxo_flow_cmd(
            oxo_flow_bin, ["validate", os.path.basename(path)], cwd=os.path.dirname(path)
        )
        scores["validate_pass"] = 1.0 if code == 0 else 0.0
        code, _, _ = common.oxo_flow_cmd(
            oxo_flow_bin, ["lint", os.path.basename(path)], cwd=os.path.dirname(path)
        )
        scores["lint_pass"] = 1.0 if code == 0 else 0.0

        try:
            expected_steps = json.loads(row["expected_steps"])
            expected_tools = json.loads(row["expected_tools"])
            expected_edges = json.loads(row["expected_dag_edges"])
            expected_outputs = json.loads(row["expected_outputs"])
        except json.JSONDecodeError:
            expected_steps = expected_tools = expected_edges = expected_outputs = []

        if expected_steps:
            hits = sum(
                1 for step in expected_steps
                if any(common.loose_step_match(step, name) for name in names)
            )
            scores["step_coverage"] = round(hits / len(expected_steps), 3)
        if expected_tools:
            hits = sum(1 for tool in expected_tools if common.name_present(tool, text))
            scores["tool_coverage"] = round(hits / len(expected_tools), 3)

        # Edge coverage: map expected step names to generated rule names.
        name_map = {}
        for step in expected_steps:
            for name in names:
                if common.loose_step_match(step, name):
                    name_map[step] = name
                    break
        edges = generated_edges(rules)
        if expected_edges:
            hits = sum(
                1 for src, dst in expected_edges
                if (name_map.get(src), name_map.get(dst)) in edges
            )
            scores["edge_coverage"] = round(hits / len(expected_edges), 3)

        if expected_outputs:
            declared = {
                common.wildcard_norm(p) for r in rules for p in r.get("output", [])
            }
            hits = sum(1 for p in expected_outputs if common.wildcard_norm(p) in declared)
            scores["output_coverage"] = round(hits / len(expected_outputs), 3)

        scores["overall"] = round(sum(scores.values()) / len(scores), 3) if scores else 0.0
        results.append(
            {
                "id": row["id"],
                "layer": "workflow",
                **scores,
                "rule_count": len(rules),
                "parse_err": parse_err or "",
            }
        )
    return results


# ── Reporting ───────────────────────────────────────────────────────────────

def report(results, gold_rows, out_path):
    """Print the summary and write results.csv."""
    if not results:
        print("no rows judged — nothing to report")
        return
    fieldnames = ["id", "layer"]
    for key in results[0]:
        if key not in fieldnames:
            fieldnames.append(key)
    with open(out_path, "w", newline="", encoding="utf-8") as fh:
        writer = csv.DictWriter(fh, fieldnames=fieldnames, extrasaction="ignore")
        writer.writeheader()
        writer.writerows(results)

    overalls = [r["overall"] for r in results if "overall" in r]
    by_type = {}
    gold_by_id = {r["id"]: r for r in gold_rows}
    for r in results:
        gold = gold_by_id.get(r["id"], {})
        qtype = gold.get("query_type") or gold.get("difficulty") or "all"
        by_type.setdefault(qtype, []).append(r["overall"])

    print(f"\n=== {results[0]['layer']} layer: {len(results)} items, "
          f"mean overall {sum(overalls) / len(overalls):.3f} ===")
    for qtype, values in sorted(by_type.items()):
        print(f"  {qtype:14s} n={len(values):3d}  mean={sum(values) / len(values):.3f}")
    print(f"\nWrote {out_path}")


def main():
    ap = argparse.ArgumentParser(description=__doc__)
    ap.add_argument("layer", choices=["tool", "rule", "workflow"])
    ap.add_argument("--captures", required=True, help="answers CSV or captures dir")
    ap.add_argument("--oxo-flow", help="oxo-flow binary (rule/workflow layers)")
    ap.add_argument("--out", default="eval/results.csv", help="results CSV path")
    ap.add_argument(
        "--gold",
        help="override gold CSV path (default: eval/gold/<layer>.csv)",
    )
    ap.add_argument("--include-unreviewed", action="store_true")
    args = ap.parse_args()

    gold_path = args.gold or os.path.join(common.GOLD_DIR, f"{args.layer}.csv")
    with open(gold_path, newline="", encoding="utf-8") as fh:
        gold_rows = list(csv.DictReader(fh))
    if not args.include_unreviewed:
        approved = [r for r in gold_rows if r.get("review_status") == "approved"]
        skipped = len(gold_rows) - len(approved)
        if skipped:
            print(
                f"note: skipping {skipped} unreviewed {args.layer} row(s); "
                f"pass --include-unreviewed to judge them anyway"
            )
        gold_rows = approved
    if args.layer == "tool":
        results = judge_tool(gold_rows, args.captures)
    else:
        if not args.oxo_flow:
            sys.exit("--oxo-flow <binary> is required for the rule/workflow layers")
        if args.layer == "rule":
            results = judge_rule(gold_rows, args.captures, args.oxo_flow)
        else:
            results = judge_workflow(gold_rows, args.captures, args.oxo_flow)
    report(results, gold_rows, args.out)


if __name__ == "__main__":
    main()
