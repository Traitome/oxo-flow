#!/usr/bin/env python3
"""Capture workflow/rule-layer AI outputs into canonical per-trial artifacts.

Rule mode: one or more chat calls per rule.csv row, asking the model to write a
single-rule oxo-flow workflow.

Workflow mode: one or more `oxo-flow template --ai` runs per workflow.csv row —
the real generation surface, knowledge-grounded by the embedded tool registry.

Usage:
  python3 eval/scripts/capture_workflow.py rule --out outputs/rules \
      [--gold eval/gold/rule.csv] [--trials K] [--seed N] [--include-unreviewed]
  python3 eval/scripts/capture_workflow.py workflow --out outputs/workflows \
      --oxo-flow <path/to/oxo-flow> [--gold eval/gold/workflow.csv] [--trials K]

Outputs are stored as `<out>/<item-id>/trial-001.oxoflow` plus
`trial-001.meta.json`, along with a top-level `manifest.json`.
Stdlib only.
"""

import argparse
import os
import shutil
import subprocess
import sys
import tempfile
import time

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


def json_dump(value):
    return common.json.dumps(value, sort_keys=True, ensure_ascii=False) if value else "{}"


def item_dir(out_dir, item_id):
    path = os.path.join(out_dir, item_id)
    os.makedirs(path, exist_ok=True)
    return path


def trial_file(out_dir, item_id, trial):
    return os.path.join(item_dir(out_dir, item_id), f"trial-{trial:03d}.oxoflow")


def trial_meta_file(out_dir, item_id, trial):
    return os.path.join(item_dir(out_dir, item_id), f"trial-{trial:03d}.meta.json")


def first_generated_oxoflow(root_dir):
    found = []
    for dirpath, _, filenames in os.walk(root_dir):
        for name in filenames:
            if name.endswith(".oxoflow"):
                found.append(os.path.join(dirpath, name))
    return sorted(found)[0] if found else None


def capture_rules(rows, out_dir, provider, args):
    failures = 0
    for trial in range(1, args.trials + 1):
        for i, row in enumerate(rows, 1):
            task = row["task_description"]
            if row.get("context_note"):
                task += f"\nConstraints: {row['context_note']}"
            messages = [
                {"role": "system", "content": RULE_SYSTEM_PROMPT},
                {"role": "user", "content": task},
            ]
            started = common.utc_now()
            t0 = time.time()
            content = ""
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
                content = response["content"]
                meta = response.get("meta", {})
                with open(trial_file(out_dir, row["id"], trial), "w", encoding="utf-8") as fh:
                    fh.write(content.rstrip() + "\n")
                preview = os.path.basename(trial_file(out_dir, row["id"], trial))
            except Exception as exc:
                failures += 1
                error = str(exc)
                preview = f"ERROR: {error[:72]}"
            finished = common.utc_now()
            common.write_json(
                trial_meta_file(out_dir, row["id"], trial),
                {
                    "id": row["id"],
                    "trial": trial,
                    "layer": "rule",
                    "capture_mode": "chat",
                    "provider_kind": provider["kind"],
                    "model_requested": provider["model"],
                    "model_reported": meta.get("response_model", ""),
                    "response_id": meta.get("response_id", ""),
                    "system_fingerprint": meta.get("system_fingerprint", ""),
                    "stop_reason": meta.get("stop_reason", ""),
                    "seed": meta.get("seed", args.seed),
                    "seed_supported": meta.get("seed_supported", False),
                    "usage_json": meta.get("usage", {}),
                    "prompt": task,
                    "system_prompt": RULE_SYSTEM_PROMPT,
                    "output_file": trial_file(out_dir, row["id"], trial),
                    "started_at": started,
                    "finished_at": finished,
                    "duration_ms": int((time.time() - t0) * 1000),
                    "error": error,
                    "output_sha256": common.sha256_file(trial_file(out_dir, row["id"], trial)) if content else "",
                },
            )
            print(f"[trial {trial}/{args.trials}] [{i}/{len(rows)}] {row['id']}: {preview}")
    return failures


def capture_workflows(rows, out_dir, oxo_flow_bin, provider, args):
    failures = 0
    for trial in range(1, args.trials + 1):
        for i, row in enumerate(rows, 1):
            started = common.utc_now()
            t0 = time.time()
            stdout = ""
            stderr = ""
            error = ""
            generated = ""
            returncode = -1
            with tempfile.TemporaryDirectory(prefix="oxo-eval-", dir="/tmp") as tmpdir:
                proc = subprocess.run(
                    [
                        oxo_flow_bin,
                        "template",
                        "--ai",
                        row["requirement_text"],
                        "-o",
                        tmpdir,
                    ],
                    capture_output=True,
                    text=True,
                    timeout=600,
                )
                returncode = proc.returncode
                stdout = proc.stdout
                stderr = proc.stderr
                found = first_generated_oxoflow(tmpdir)
                if proc.returncode == 0 and found:
                    generated = trial_file(out_dir, row["id"], trial)
                    shutil.copyfile(found, generated)
                else:
                    failures += 1
                    if proc.returncode != 0:
                        error = f"template exit {proc.returncode}"
                    elif not found:
                        error = "template did not produce a .oxoflow file"
            finished = common.utc_now()
            common.write_json(
                trial_meta_file(out_dir, row["id"], trial),
                {
                    "id": row["id"],
                    "trial": trial,
                    "layer": "workflow",
                    "capture_mode": "oxo-flow template --ai",
                    "provider_kind": provider["kind"] if provider else "unknown",
                    "model_requested": provider["model"] if provider else "unknown",
                    "prompt": row["requirement_text"],
                    "output_file": generated,
                    "output_sha256": common.sha256_file(generated) if generated else "",
                    "started_at": started,
                    "finished_at": finished,
                    "duration_ms": int((time.time() - t0) * 1000),
                    "returncode": returncode,
                    "stdout": stdout,
                    "stderr": stderr,
                    "error": error,
                    "oxo_flow_binary": os.path.abspath(oxo_flow_bin),
                },
            )
            print(f"[trial {trial}/{args.trials}] [{i}/{len(rows)}] {row['id']}: exit {returncode}")
    return failures


def main():
    ap = argparse.ArgumentParser(description=__doc__)
    ap.add_argument("mode", choices=["rule", "workflow"])
    ap.add_argument("--out", required=True, help="output directory")
    ap.add_argument("--gold", help="override gold CSV path")
    ap.add_argument("--oxo-flow", help="path to the oxo-flow binary (workflow mode)")
    ap.add_argument("--limit", type=int, default=0, help="max items (0 = all)")
    ap.add_argument("--trials", type=int, default=1, help="number of repeated trials per item")
    ap.add_argument("--seed", type=int, help="optional seed for providers that support it")
    ap.add_argument("--temperature", type=float, default=0.2, help="sampling temperature (rule mode)")
    ap.add_argument("--max-tokens", type=int, default=4096, help="max response tokens (rule mode)")
    ap.add_argument("--include-unreviewed", action="store_true")
    args = ap.parse_args()

    if args.trials < 1:
        sys.exit("--trials must be >= 1")

    out_dir = os.path.abspath(args.out)
    os.makedirs(out_dir, exist_ok=True)
    rows = common.load_gold(args.mode, args.include_unreviewed, args.gold)
    if args.limit:
        rows = rows[: args.limit]

    provider = common.resolve_provider()
    if provider is None:
        sys.exit("no AI provider configured (see capture_tool.py help)")

    started = common.utc_now()
    if args.mode == "rule":
        failures = capture_rules(rows, out_dir, provider, args)
        capture_mode = "chat"
    else:
        if not args.oxo_flow:
            sys.exit("--oxo-flow <binary> is required for workflow mode")
        failures = capture_workflows(rows, out_dir, args.oxo_flow, provider, args)
        capture_mode = "oxo-flow template --ai"

    common.write_json(
        common.manifest_path(out_dir),
        {
            "benchmark": "oxo-flow-ai-eval",
            "schema_version": 1,
            "layer": args.mode,
            "capture_mode": capture_mode,
            "created_at": started,
            "finished_at": common.utc_now(),
            "repo_root": common.REPO_ROOT,
            "repo_commit": common.repo_commit(),
            "repo_dirty": common.repo_dirty(),
            "gold_path": common.gold_csv_path(args.mode, args.gold),
            "gold_sha256": common.sha256_file(common.gold_csv_path(args.mode, args.gold)),
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
            "failures": failures,
            "output_dir": out_dir,
            "oxo_flow_binary": os.path.abspath(args.oxo_flow) if args.oxo_flow else "",
        },
    )
    print(f"Wrote manifest to {common.manifest_path(out_dir)}")
    if failures:
        sys.exit(f"{failures} capture(s) failed; see per-trial metadata for details")


if __name__ == "__main__":
    main()
