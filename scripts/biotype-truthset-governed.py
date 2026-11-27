#!/usr/bin/env python3
"""Run the held-out governed BioType truth set through the real CLI surface.

Issue #1396. The EXPECT source is the sealed pre-run manifest's `cases`
array; there is no second hand-maintained expected-verdict table, so the
sealed list and the executed list cannot drift.

Verdict surface (PR-03 C1): `oxo-flow validate <case> --json`, scored on
the JSON `valid` field. Code cross-check: the union of TYPE-NNN codes on
`validate --json` (fatal codes) and `lint --json` (which also carries the
advisory TYPE-008/009/010 the admission gate drops), compared for set
equality with the manifest's expected codes.

A case whose `validate --json` output cannot be parsed is a NON-VERDICT
(PR-02 C1: any non-verdict makes the campaign INVALID).

Usage:
  python3 scripts/biotype-truthset-governed.py \
      --binary target/debug/oxo-flow \
      --manifest docs/superpowers/preregistration/prerun/C1-governed-2026-11-27.json \
      --out docs/superpowers/audits/data/biotype-truthset-governed-results.json

Exit: 0 always for a completed measurement (setup errors exit 2).
"""

import argparse
import hashlib
import json
import platform
import re
import socket
import subprocess
import sys
from datetime import datetime, timezone
from pathlib import Path

PER_CASE_TIMEOUT_SECS = 60
TYPE_CODE_RE = re.compile(r"TYPE-[A-Z0-9-]+")
REPO = Path(__file__).resolve().parent.parent


def sha256_file(path):
    h = hashlib.sha256()
    with open(path, "rb") as f:
        for chunk in iter(lambda: f.read(1 << 20), b""):
            h.update(chunk)
    return h.hexdigest()


def run_command(binary, subcommand, case_path):
    cmd = [str(binary), subcommand, str(case_path), "--json"]
    try:
        proc = subprocess.run(cmd, capture_output=True, text=True,
                              timeout=PER_CASE_TIMEOUT_SECS)
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
    if not binary.is_file():
        print(f"binary not found: {binary}", file=sys.stderr)
        return 2
    if not manifest_path.is_file():
        print(f"manifest not found: {manifest_path}", file=sys.stderr)
        return 2

    manifest = json.loads(manifest_path.read_text())
    cases = manifest.get("cases") or []
    if not cases:
        print("manifest has no cases", file=sys.stderr)
        return 2

    commit = subprocess.run(["git", "-C", str(REPO), "rev-parse", "HEAD"],
                            capture_output=True, text=True, check=True).stdout.strip()
    version = subprocess.run([str(binary), "--version"], capture_output=True,
                             text=True, check=True).stdout.strip()

    rows = []
    ta = fa = tr = fr = 0
    codes_ok = codes_bad = 0
    non_verdicts = 0
    for entry in cases:
        rel = entry["case"]
        case_path = REPO / rel
        expected = entry["expected"]
        expected_codes = sorted(entry.get("codes") or [])
        note = entry.get("note", "")

        stdout, err = run_command(binary, "validate", case_path)
        doc = parse_json(stdout)
        if doc is None or "valid" not in doc:
            non_verdicts += 1
            rows.append({
                "case": rel, "expected": expected, "actual": "non-verdict",
                "verdict_correct": "NO", "expected_codes": expected_codes,
                "actual_codes": [], "codes_correct": "NO", "note": note,
                "error": err or "validate --json produced no parsable JSON",
            })
            continue
        actual = "accept" if doc.get("valid") is True else "reject"
        if expected == actual:
            correct = "yes"
        else:
            correct = "NO"
        if expected == "accept" and actual == "accept":
            ta += 1
        elif expected == "accept" and actual == "reject":
            fr += 1
        elif expected == "reject" and actual == "reject":
            tr += 1
        elif expected == "reject" and actual == "accept":
            fa += 1

        seen = codes_from(doc, ("errors", "warnings"))
        lint_stdout, lint_err = run_command(binary, "lint", case_path)
        lint_doc = parse_json(lint_stdout)
        if lint_doc is not None:
            seen |= codes_from(lint_doc, ("diagnostics",))
        actual_codes = sorted(seen)
        if expected_codes == actual_codes:
            codes_correct = "yes"
            codes_ok += 1
        else:
            codes_correct = "NO"
            codes_bad += 1
        rows.append({
            "case": rel, "expected": expected, "actual": actual,
            "verdict_correct": correct, "expected_codes": expected_codes,
            "actual_codes": actual_codes, "codes_correct": codes_correct,
            "note": note,
        })

    reject_rows = [r for r in rows if r["expected"] == "reject"]
    result = {
        "format": "oxo-flow.biotype-truthset-governed-results/v1",
        "issue": 1396,
        "campaign": "#1163 governed BioTypes truth set (held-out corpus)",
        "generated_at": datetime.now(timezone.utc).strftime("%Y-%m-%dT%H:%M:%SZ"),
        "commit": commit,
        "engine_commit": manifest.get("engine_commit", commit),
        "binary": str(binary),
        "binary_sha256": sha256_file(binary),
        "engine_version": version,
        "host": {
            "hostname": socket.gethostname(),
            "platform": platform.platform(),
            "machine": platform.machine(),
            "python": platform.python_version(),
        },
        "manifest": str(manifest_path.relative_to(REPO)),
        "manifest_sha256": sha256_file(manifest_path),
        "cases": rows,
        "confusion_matrix": {"true_accept": ta, "false_accept": fa,
                             "true_reject": tr, "false_reject": fr},
        "codes_correct": codes_ok,
        "codes_fail": codes_bad,
        "cases_total": len(rows),
        "non_verdicts": non_verdicts,
        "primary": {
            "definition": "FA / (FA + TR) over expected-reject cases",
            "denominator": len(reject_rows),
            "false_accepts": fa,
            "true_rejects": tr,
        },
    }
    out_path.parent.mkdir(parents=True, exist_ok=True)
    out_path.write_text(json.dumps(result, indent=2, sort_keys=False) + "\n")

    print(f"cases={len(rows)} ta={ta} fa={fa} tr={tr} fr={fr} "
          f"non_verdicts={non_verdicts} codes={codes_ok}/{codes_ok + codes_bad}")
    print(f"primary denominator={len(reject_rows)} false_accepts={fa}")
    print(f"wrote {out_path}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
