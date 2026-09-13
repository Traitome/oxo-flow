#!/usr/bin/env python3
"""Seeded-failure benchmark for `report --ai` interpretation quality (issue #359).

Method: real `oxo-flow run` executions with known, planted faults produce real
checkpoints; `report --ai` then interprets each checkpoint and the output is
scored against ground truth with fixed regexes — the generation-benchmark
philosophy (deterministic judging, no LLM-as-judge) applied to the
interpretation surface.

Usage:
    # provider env must be exported first (e.g. OXO_FLOW_AI_PROVIDER=claude
    # plus the ANTHROPIC_* trio, or any other provider backend)
    python3 run_bench.py                      # all seeds, stamped run
    python3 run_bench.py --seeds s01,s03      # subset
    python3 run_bench.py --binary /path/oxo-flow

Artifacts land in results/<stamp>/<seed>/ (run stderr, AI prose, report.md)
plus results/<stamp>/results.json and report.md.
"""

from __future__ import annotations

import argparse
import json
import os
import re
import shutil
import subprocess
import sys
import time
from pathlib import Path

HERE = Path(__file__).resolve().parent
SEEDS_DIR = HERE / "seeds"
ANSI = re.compile(r"\x1b\[[0-9;]*m")


def clean(text: str) -> str:
    return ANSI.sub("", text)


def load_manifest(seed_dir: Path) -> dict:
    return json.loads((seed_dir / "manifest.json").read_text())


def materialize(workdir: Path, setup: dict) -> None:
    for rel, content in setup.items():
        path = workdir / rel
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text(content)


def run_cmd(cmd: list[str], cwd: Path, timeout: int) -> tuple[int, str]:
    proc = subprocess.run(
        cmd, cwd=cwd, capture_output=True, text=True, timeout=timeout
    )
    return proc.returncode, clean(proc.stderr)


def score_seed(manifest: dict, prose: str) -> tuple[bool, list[str], list[str]]:
    matched = [p for p in manifest.get("expect_any", []) if re.search(p, prose, re.I)]
    forbid_hit = [p for p in manifest.get("forbid", []) if re.search(p, prose, re.I)]
    hit = bool(matched) and not forbid_hit
    return hit, matched, forbid_hit


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__)
    ap.add_argument("--binary", default=None, help="oxo-flow binary (default: repo target/release or /debug)")
    ap.add_argument("--seeds", default=None, help="comma-separated seed ids (default: all)")
    ap.add_argument("--stamp", default=None, help="results directory label (default: UTC timestamp)")
    ap.add_argument("--timeout", type=int, default=300, help="per-command timeout in seconds")
    args = ap.parse_args()

    binary = Path(
        args.binary
        or next(
            p
            for p in [
                HERE.parent.parent / "target" / "release" / "oxo-flow",
                HERE.parent.parent / "target" / "debug" / "oxo-flow",
            ]
            if p.exists()
        )
    ).resolve()

    provider_ready = bool(os.environ.get("OXO_FLOW_AI_PROVIDER")) or (
        Path.home() / ".oxo-flow" / "ai_config.json"
    ).exists()
    if not provider_ready:
        print(
            "No AI provider configured: export OXO_FLOW_AI_PROVIDER and the "
            "provider's key env vars before running the benchmark.",
            file=sys.stderr,
        )
        return 2

    seed_ids = (
        [s.strip() for s in args.seeds.split(",")]
        if args.seeds
        else sorted(d.name for d in SEEDS_DIR.iterdir() if d.is_dir())
    )
    stamp = args.stamp or time.strftime("%Y%m%d-%H%M%S")
    out_root = HERE / "results" / stamp

    rows = []
    for seed_id in seed_ids:
        seed_dir = SEEDS_DIR / seed_id
        manifest = load_manifest(seed_dir)
        workdir = out_root / seed_id
        workdir.mkdir(parents=True, exist_ok=True)
        shutil.copy(seed_dir / "workflow.oxoflow", workdir / "workflow.oxoflow")
        materialize(workdir, manifest.get("setup", {}))

        run_exit, run_stderr = run_cmd(
            [str(binary), "run", "workflow.oxoflow"], workdir, args.timeout
        )
        (workdir / "run.stderr.log").write_text(run_stderr)
        exit_ok = run_exit == manifest["run_expect_exit"]

        report_cmd = [str(binary), "report", "workflow.oxoflow", "--ai"]
        if manifest["surface"] == "report-failed":
            report_cmd.append("--failed")
        report_cmd += ["--format", "md", "-o", "report.md"]
        report_exit, report_stderr = run_cmd(report_cmd, workdir, args.timeout)
        prose = report_stderr
        (workdir / "ai.txt").write_text(prose)

        hit, matched, forbid_hit = score_seed(manifest, prose)
        broken = not exit_ok or report_exit != 0
        rows.append(
            {
                "id": manifest["id"],
                "surface": manifest["surface"],
                "ground_truth": manifest["ground_truth"],
                "run_exit": run_exit,
                "run_exit_ok": exit_ok,
                "report_exit": report_exit,
                "broken": broken,
                "hit": hit,
                "matched": matched,
                "forbid_hit": forbid_hit,
                "ai_chars": len(prose),
            }
        )

    scored = [r for r in rows if not r["broken"]]
    summary = {
        "stamp": stamp,
        "binary": str(binary),
        "seeds": len(rows),
        "broken": [r["id"] for r in rows if r["broken"]],
        "hit_rate": (
            round(sum(r["hit"] for r in scored) / len(scored), 3) if scored else None
        ),
        "results": rows,
    }
    out_root.mkdir(parents=True, exist_ok=True)
    (out_root / "results.json").write_text(json.dumps(summary, indent=2) + "\n")

    lines = [
        f"# Interpretation benchmark — {stamp}",
        "",
        f"Binary: `{binary}` · seeds: {len(rows)} · hit-rate: **{summary['hit_rate']}**",
        "",
        "| seed | surface | run exit | hit | matched |",
        "|---|---|---|---|---|",
    ]
    for r in rows:
        flag = "✅" if r["hit"] else ("⚠️ broken" if r["broken"] else "❌")
        lines.append(
            f"| {r['id']} | {r['surface']} | {r['run_exit']} | {flag} | "
            f"{', '.join(r['matched']) or '—'} |"
        )
    (out_root / "report.md").write_text("\n".join(lines) + "\n")

    print("\n".join(lines))
    print(f"\nresults: {out_root}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
