#!/usr/bin/env python3
"""Capture workflow/rule-layer AI outputs into .oxoflow files.

Rule mode: one chat call per rule.csv row, asking the model to write a
single-rule oxo-flow workflow (there is no single-rule CLI surface, so
the raw chat is the honest capture of that capability).

Workflow mode: one `oxo-flow template --ai "<requirement>"` run per
workflow.csv row — the real generation surface, knowledge-grounded by
the embedded tool registry.

Usage:
  python3 eval/scripts/capture_workflow.py rule --out outputs/rules \
      [--limit N] [--include-unreviewed]
  python3 eval/scripts/capture_workflow.py workflow --out outputs/workflows \
      --oxo-flow <path/to/oxo-flow> [--limit N] [--include-unreviewed]

Stdlib only.
"""

import argparse
import csv
import os
import subprocess
import sys

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import common  # noqa: E402

RULE_SYSTEM_PROMPT = (
    "You are an expert oxo-flow workflow author. Oxo-flow is a TOML-based "
    "bioinformatics workflow engine. Answer ONLY with a complete .oxoflow "
    "file (TOML, [workflow] header plus one [[rules]] section) implementing "
    "the requested step, with [rules.resources] and a pinned "
    "[rules.environment] (bioconda::tool=VERSION where possible). No prose, "
    "no markdown fences."
)

WORKFLOW_SYSTEM_PROMPT = (
    "You are an expert oxo-flow workflow author. Oxo-flow is a TOML-based "
    "bioinformatics workflow engine. Design a complete multi-rule workflow "
    "implementing the requested pipeline, with [rules.resources] and pinned "
    "[rules.environment] per rule. No prose, no markdown fences."
)


def capture_rules(rows, out_dir):
    provider = common.resolve_provider()
    if provider is None:
        sys.exit("no AI provider configured (see capture_tool.py help)")
    os.makedirs(out_dir, exist_ok=True)
    for i, row in enumerate(rows, 1):
        task = row["task_description"]
        if row.get("context_note"):
            task += f"\nConstraints: {row['context_note']}"
        content = common.chat(
            [
                {"role": "system", "content": RULE_SYSTEM_PROMPT},
                {"role": "user", "content": task},
            ],
            provider,
        )
        path = os.path.join(out_dir, f"{row['id']}.oxoflow")
        with open(path, "w", encoding="utf-8") as fh:
            fh.write(content + "\n")
        print(f"[{i}/{len(rows)}] wrote {path}")


def capture_workflows(rows, out_dir, oxo_flow_bin):
    os.makedirs(out_dir, exist_ok=True)
    for i, row in enumerate(rows, 1):
        dest = os.path.join(out_dir, row["id"])
        proc = subprocess.run(
            [
                oxo_flow_bin,
                "template",
                "--ai",
                row["requirement_text"],
                "-o",
                dest,
            ],
            capture_output=True,
            text=True,
            timeout=600,
        )
        print(f"[{i}/{len(rows)}] {row['id']}: exit {proc.returncode}")
        if proc.returncode != 0:
            print(proc.stderr.strip()[:400])


def main():
    ap = argparse.ArgumentParser(description=__doc__)
    ap.add_argument("mode", choices=["rule", "workflow"])
    ap.add_argument("--out", required=True, help="output directory")
    ap.add_argument("--oxo-flow", help="path to the oxo-flow binary (workflow mode)")
    ap.add_argument("--limit", type=int, default=0, help="max items (0 = all)")
    ap.add_argument("--include-unreviewed", action="store_true")
    args = ap.parse_args()

    rows = common.load_gold(args.mode, args.include_unreviewed)
    if args.limit:
        rows = rows[: args.limit]
    if args.mode == "rule":
        capture_rules(rows, args.out)
    else:
        if not args.oxo_flow:
            sys.exit("--oxo-flow <binary> is required for workflow mode")
        capture_workflows(rows, args.out, args.oxo_flow)


if __name__ == "__main__":
    main()
