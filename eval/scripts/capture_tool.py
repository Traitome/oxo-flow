#!/usr/bin/env python3
"""Capture tool-layer AI answers: one row of gold/tool.csv -> one or more chat calls.

Usage:
  python3 eval/scripts/capture_tool.py --out outputs/tool_answers.csv \
      [--gold eval/gold/tool.csv] [--limit N] [--trials K] [--seed N] [--include-unreviewed]

Writes one CSV row per (item, trial) for the runner to judge, plus a JSON
manifest sidecar documenting the benchmark provenance and decoding settings.
Stdlib only.
"""

import argparse
import csv
import os
import sys
import time

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
    ap.add_argument("--gold", help="override gold CSV path")
    ap.add_argument("--limit", type=int, default=0, help="max items (0 = all)")
    ap.add_argument("--trials", type=int, default=1, help="number of repeated trials per item")
    ap.add_argument("--seed", type=int, help="optional seed for providers that support it")
    ap.add_argument("--temperature", type=float, default=0.2, help="sampling temperature")
    ap.add_argument("--max-tokens", type=int, default=2048, help="max response tokens")
    ap.add_argument(
        "--include-unreviewed",
        action="store_true",
        help="capture draft rows too (default: approved only)",
    )
    args = ap.parse_args()

    if args.trials < 1:
        sys.exit("--trials must be >= 1")

    rows = common.load_gold("tool", args.include_unreviewed, args.gold)
    if args.limit:
        rows = rows[: args.limit]

    provider = common.resolve_provider()
    if provider is None:
        sys.exit(
            "no AI provider configured: set OXO_FLOW_AI_PROVIDER and the API "
            "key env var, or run 'oxo-flow ai setup'"
        )

    out_path = os.path.abspath(args.out)
    os.makedirs(os.path.dirname(out_path) or ".", exist_ok=True)
    manifest_path = common.manifest_path(out_path)
    run_started = common.utc_now()
    rows_out = []
    failures = 0

    for trial in range(1, args.trials + 1):
        for i, row in enumerate(rows, 1):
            messages = [
                {"role": "system", "content": SYSTEM_PROMPT},
                {"role": "user", "content": row["query"]},
            ]
            started = common.utc_now()
            t0 = time.time()
            answer = ""
            meta = {}
            error = ""
            try:
                response = common.chat(
                    messages,
                    provider,
                    max_tokens=args.max_tokens,
                    temperature=args.temperature,
                    seed=args.seed,
                )
                answer = response["content"]
                meta = response.get("meta", {})
                preview = answer[:80]
            except Exception as exc:  # deterministic recording of failures
                failures += 1
                error = str(exc)
                preview = f"ERROR: {error[:72]}"
            finished = common.utc_now()
            duration_ms = int((time.time() - t0) * 1000)
            rows_out.append(
                {
                    "id": row["id"],
                    "trial": trial,
                    "query": row["query"],
                    "answer": answer,
                    "started_at": started,
                    "finished_at": finished,
                    "duration_ms": duration_ms,
                    "provider_kind": provider["kind"],
                    "model_requested": provider["model"],
                    "model_reported": meta.get("response_model", ""),
                    "response_id": meta.get("response_id", ""),
                    "system_fingerprint": meta.get("system_fingerprint", ""),
                    "stop_reason": meta.get("stop_reason", ""),
                    "seed": meta.get("seed", ""),
                    "seed_supported": meta.get("seed_supported", ""),
                    "usage_json": json_dump(meta.get("usage", {})),
                    "error": error,
                }
            )
            print(f"[trial {trial}/{args.trials}] [{i}/{len(rows)}] {row['id']}: {preview!r}")

    with open(out_path, "w", newline="", encoding="utf-8") as fh:
        fieldnames = [
            "id",
            "trial",
            "query",
            "answer",
            "started_at",
            "finished_at",
            "duration_ms",
            "provider_kind",
            "model_requested",
            "model_reported",
            "response_id",
            "system_fingerprint",
            "stop_reason",
            "seed",
            "seed_supported",
            "usage_json",
            "error",
        ]
        writer = csv.DictWriter(fh, fieldnames=fieldnames)
        writer.writeheader()
        writer.writerows(rows_out)

    common.write_json(
        manifest_path,
        {
            "benchmark": "oxo-flow-ai-eval",
            "schema_version": 1,
            "layer": "tool",
            "capture_mode": "chat",
            "created_at": run_started,
            "finished_at": common.utc_now(),
            "repo_root": common.REPO_ROOT,
            "repo_commit": common.repo_commit(),
            "repo_dirty": common.repo_dirty(),
            "gold_path": common.gold_csv_path("tool", args.gold),
            "gold_sha256": common.sha256_file(common.gold_csv_path("tool", args.gold)),
            "knowledge_sha256": common.knowledge_digest(),
            "provider": common.provider_public_config(provider),
            "generation": {
                "max_tokens": args.max_tokens,
                "temperature": args.temperature,
                "seed": args.seed,
            },
            "trials": args.trials,
            "include_unreviewed": args.include_unreviewed,
            "item_count": len(rows),
            "item_ids": [row["id"] for row in rows],
            "row_count": len(rows_out),
            "failures": failures,
            "system_prompt": SYSTEM_PROMPT,
            "output_csv": out_path,
        },
    )
    print(f"Wrote {len(rows_out)} answers to {out_path}")
    print(f"Wrote manifest to {manifest_path}")
    if failures:
        sys.exit(f"{failures} capture(s) failed; see CSV/manifest for details")


def json_dump(value):
    return common.json.dumps(value, sort_keys=True, ensure_ascii=False) if value else "{}"


if __name__ == "__main__":
    main()
