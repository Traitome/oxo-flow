#!/usr/bin/env python3
"""Run the post-hoc governed supplement through the real CLI surface.

Issue #1396. These are DESCRIPTIVE acceptance controls authored after the
primary outcome was seen. They are not part of the pre-registered
confirmatory set and never enter the false-accept denominator.

Unlike the sealed primary runner, this runner enforces BOTH the per-case
subprocess timeout (60 s) and the total campaign deadline (7200 s), and it
records the elapsed wall time in the result file.

Usage:
  python3 scripts/biotype-truthset-governed-supplement.py \
      --binary target/debug/oxo-flow \
      --manifest docs/superpowers/preregistration/prerun/C1-governed-supplement-2026-11-28.json \
      --out docs/superpowers/audits/data/biotype-truthset-governed-supplement-results.json

Exit: 0 for a completed measurement (setup errors 2). If the total deadline
is exceeded, remaining cases are recorded as non-verdicts with reason
"total_deadline_exceeded" and the run still exits 0 (measurement).
"""

import argparse
import hashlib
import json
import platform
import re
import socket
import subprocess
import sys
import time
from datetime import datetime, timezone
from pathlib import Path

PER_CASE_TIMEOUT_SECS = 60
TOTAL_TIMEOUT_SECS = 7200
TYPE_CODE_RE = re.compile(r"TYPE-[A-Z0-9-]+")
REPO = Path(__file__).resolve().parent.parent


def sha256_file(path):
    h = hashlib.sha256()
    with open(path, "rb") as f:
        for chunk in iter(lambda: f.read(1 << 20), b""):
            h.update(chunk)
    return h.hexdigest()


def run_command(binary, subcommand, case_path, timeout):
    cmd = [str(binary), subcommand, str(case_path), "--json"]
    try:
        proc = subprocess.run(cmd, capture_output=True, text=True, timeout=timeout)
    except subprocess.TimeoutExpired:
        return None, "timeout"
    return proc.stdout, None


def parse_json(text):
    if not text:
        return None
    try:
        return json.loads(text)
    except json.JSONDecodeError:
        return None


def codes_from(doc, container_keys):
    seen = set()
    for key in container_keys:
        for item in doc.get(key) or []:
            code = item.get("code") if isinstance(item, dict) else None
            if code and TYPE_CODE_RE.fullmatch(code):
                seen.add(code)
    return seen


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--binary", required=True)
    ap.add_argument("--manifest", required=True)
    ap.add_argument("--out", required=True)
    args = ap.parse_args()

    binary = Path(args.binary).resolve()
    manifest_path = Path(args.manifest).resolve()
    out_path = Path(args.out).resolve()
    if not binary.is_file() or not manifest_path.is_file():
        print("binary or manifest missing", file=sys.stderr)
        return 2

    manifest = json.loads(manifest_path.read_text())
    cases = manifest.get("cases") or []
    if not cases:
        print("manifest has no cases", file=sys.stderr)
        return 2

    started = time.monotonic()
    generated_at = datetime.now(timezone.utc).strftime("%Y-%m-%dT%H:%M:%SZ")
    commit = subprocess.run(["git", "-C", str(REPO), "rev-parse", "HEAD"],
                            capture_output=True, text=True, check=True).stdout.strip()
    version = subprocess.run([str(binary), "--version"], capture_output=True,
                             text=True, check=True).stdout.strip()

    rows = []
    accepted = rejected = non_verdicts = codes_ok = codes_bad = 0
    for entry in cases:
        rel = entry["case"]
        case_path = REPO / rel
        expected_codes = sorted(entry.get("codes") or [])
        elapsed = time.monotonic() - started
        remaining = TOTAL_TIMEOUT_SECS - elapsed
        if remaining <= 0:
            non_verdicts += 1
            rows.append({
                "case": rel, "expected": entry["expected"], "actual": "non-verdict",
                "verdict_correct": "NO", "expected_codes": expected_codes,
                "actual_codes": [], "codes_correct": "NO", "note": entry.get("note", ""),
                "error": "total_deadline_exceeded",
            })
            continue
        timeout = min(PER_CASE_TIMEOUT_SECS, remaining)
        stdout, err = run_command(binary, "validate", case_path, timeout)
        doc = parse_json(stdout)
        if doc is None or "valid" not in doc:
            non_verdicts += 1
            rows.append({
                "case": rel, "expected": entry["expected"], "actual": "non-verdict",
                "verdict_correct": "NO", "expected_codes": expected_codes,
                "actual_codes": [], "codes_correct": "NO", "note": entry.get("note", ""),
                "error": err or "validate --json produced no parsable JSON",
            })
            continue
        actual = "accept" if doc.get("valid") is True else "reject"
        if actual == "accept":
            accepted += 1
        else:
            rejected += 1
        correct = "yes" if actual == entry["expected"] else "NO"
        seen = codes_from(doc, ("errors", "warnings"))
        lint_stdout, _ = run_command(binary, "lint", case_path,
                                     min(PER_CASE_TIMEOUT_SECS, max(1.0, TOTAL_TIMEOUT_SECS - (time.monotonic() - started))))
        lint_doc = parse_json(lint_stdout)
        if lint_doc is not None:
            seen |= codes_from(lint_doc, ("diagnostics",))
        actual_codes = sorted(seen)
        codes_correct = "yes" if expected_codes == actual_codes else "NO"
        if codes_correct == "yes":
            codes_ok += 1
        else:
            codes_bad += 1
        rows.append({
            "case": rel, "expected": entry["expected"], "actual": actual,
            "verdict_correct": correct, "expected_codes": expected_codes,
            "actual_codes": actual_codes, "codes_correct": codes_correct,
            "note": entry.get("note", ""),
        })

    elapsed = time.monotonic() - started
    result = {
        "format": "oxo-flow.biotype-truthset-governed-supplement-results/v1",
        "issue": 1396,
        "campaign": "#1163 post-hoc governed supplement (descriptive; not confirmatory)",
        "role": "post-hoc descriptive acceptance controls; never in the FA denominator",
        "generated_at": generated_at,
        "commit": commit,
        "binary": str(binary),
        "binary_sha256": sha256_file(binary),
        "engine_version": version,
        "manifest": str(manifest_path.relative_to(REPO)),
        "manifest_sha256": sha256_file(manifest_path),
        "host": {
            "hostname": socket.gethostname(),
            "platform": platform.platform(),
            "machine": platform.machine(),
            "python": platform.python_version(),
        },
        "budget": {
            "per_case_timeout_secs": PER_CASE_TIMEOUT_SECS,
            "total_timeout_secs": TOTAL_TIMEOUT_SECS,
            "elapsed_secs": round(elapsed, 3),
            "enforced": True,
        },
        "cases": rows,
        "summary": {
            "cases_total": len(rows),
            "accepted": accepted,
            "rejected": rejected,
            "non_verdicts": non_verdicts,
            "codes_correct": codes_ok,
            "codes_fail": codes_bad,
        },
    }
    out_path.parent.mkdir(parents=True, exist_ok=True)
    out_path.write_text(json.dumps(result, indent=2) + "\n")
    print(f"supplement cases={len(rows)} accepted={accepted} rejected={rejected} "
          f"non_verdicts={non_verdicts} codes={codes_ok}/{codes_ok + codes_bad} "
          f"elapsed={elapsed:.1f}s")
    print(f"wrote {out_path}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
