#!/usr/bin/env python3
"""Capture tool-layer AI answers: one row of gold/tool.csv -> one chat call.

Usage:
  python3 eval/scripts/capture_tool.py --out outputs/tool_answers.csv \
      [--limit N] [--include-unreviewed]

Writes (id, query, answer) rows for the runner to judge. The provider is
resolved exactly like the CLI: OXO_FLOW_AI_PROVIDER + key env vars, then
~/.oxo-flow/ai_config.json. Stdlib only.
"""

import argparse
import csv
import os
import sys

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import common  # noqa: E402

SYSTEM_PROMPT = (
    "You are a bioinformatics assistant answering a single question about a "
    "bioinformatics software tool. Answer concisely: name the tool, and when "
    "asked for a version, give the exact version number. If you are confident "
    "no such tool exists, say 'not found' and do not invent one."
)


def main():
    ap = argparse.ArgumentParser(description=__doc__)
    ap.add_argument("--out", required=True, help="output answers CSV")
    ap.add_argument("--limit", type=int, default=0, help="max items (0 = all)")
    ap.add_argument(
        "--include-unreviewed",
        action="store_true",
        help="capture draft rows too (default: approved only)",
    )
    args = ap.parse_args()

    rows = common.load_gold("tool", args.include_unreviewed)
    if args.limit:
        rows = rows[: args.limit]

    provider = common.resolve_provider()
    if provider is None:
        sys.exit(
            "no AI provider configured: set OXO_FLOW_AI_PROVIDER and the API "
            "key env var, or run 'oxo-flow ai setup'"
        )
    api_url, api_key, model = provider

    os.makedirs(os.path.dirname(args.out) or ".", exist_ok=True)
    with open(args.out, "w", newline="", encoding="utf-8") as fh:
        writer = csv.writer(fh)
        writer.writerow(["id", "query", "answer"])
        for i, row in enumerate(rows, 1):
            answer = common.chat(
                [
                    {"role": "system", "content": SYSTEM_PROMPT},
                    {"role": "user", "content": row["query"]},
                ],
                api_url,
                api_key,
                model,
            )
            writer.writerow([row["id"], row["query"], answer])
            print(f"[{i}/{len(rows)}] {row['id']}: {answer[:80]!r}")
    print(f"Wrote {len(rows)} answers to {args.out}")


if __name__ == "__main__":
    main()
