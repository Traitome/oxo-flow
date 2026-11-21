#!/usr/bin/env python3
"""Provenance manifest for publication-facing artifact regeneration (#1305).

Ledger §7 wave 5 and §8 Change Discipline require every benchmark artifact to
record command, commit, host, software versions, and inputs. This module emits
a key-ordered (order-independent) JSON manifest for the publication-facing
status reports under ``docs/superpowers/plans/`` and provides a fail-closed
checker.

Honesty note: the committed status reports are hand-authored documents that
embed measured rows; the generators in ``eval/benchmarks`` and
``eval/frontier`` emit the raw measurement *tables*. Neither regenerates the
committed reports byte-for-byte, and this tool does not claim that. It records
the true provenance of the measured numbers that appear in the reports.

Usage:
  python3 eval/benchmarks/provenance_manifest.py --emit eval/benchmarks/provenance_manifest.json
  python3 eval/benchmarks/provenance_manifest.py --check
"""

from __future__ import annotations

import argparse
import json
import os
import re
import sys

REPO_ROOT = os.path.abspath(os.path.join(os.path.dirname(__file__), "..", ".."))
DEFAULT_MANIFEST = os.path.join(REPO_ROOT, "eval", "benchmarks", "provenance_manifest.json")

# Publication-facing reports that must carry provenance for any measured number.
PUBLICATION_ARTIFACTS = [
    "docs/superpowers/plans/cohort-scaling-report.md",
    "docs/superpowers/plans/statistical-benchmark-report.md",
    "docs/superpowers/plans/sus-survey-report.md",
    "docs/superpowers/plans/hardware-scaling-report.md",
]

REQUIRED_ENTRY_KEYS = (
    "source_commit",
    "generator_command",
    "host",
    "tool_versions",
    "inputs",
)

COMMIT_RE = re.compile(r"^[0-9a-f]{7,40}$")
SHA256_RE = re.compile(r"^[0-9a-f]{64}$")
_SEPARATOR_CELL_RE = re.compile(r"^:?-{3,}:?$")
_NUMBER_RE = re.compile(r"^-?\d+(?:\.\d+)?$")


def _is_measured_table_row(line: str) -> bool:
    """True when a markdown table row carries at least one numeric data cell.

    Header rows and ``| :---: |`` separator rows are excluded so that a table
    with no measured values does not require a provenance entry.
    """
    stripped = line.strip()
    if not stripped.startswith("|"):
        return False
    cells = [c.strip() for c in stripped.strip("|").split("|")]
    if not cells:
        return False
    if all(_SEPARATOR_CELL_RE.match(c) for c in cells):
        return False
    for cell in cells:
        token = cell.replace(",", "").replace("+/-", " ").split()
        for piece in token:
            if _NUMBER_RE.match(piece):
                return True
    return False


def report_has_measured_number(text: str) -> bool:
    """Return True when a report body contains a measured markdown table row."""
    return any(_is_measured_table_row(line) for line in text.splitlines())


def _validate_entry(rel_path: str, entry: object, errors: list) -> None:
    if not isinstance(entry, dict):
        errors.append(f"{rel_path}: entry is not a JSON object")
        return
    for key in REQUIRED_ENTRY_KEYS:
        if key not in entry:
            errors.append(f"{rel_path}: missing required key '{key}'")
    commit = entry.get("source_commit")
    if not isinstance(commit, str) or not COMMIT_RE.match(commit):
        errors.append(f"{rel_path}: 'source_commit' is not a 7-40 hex commit: {commit!r}")
    inputs = entry.get("inputs")
    if not isinstance(inputs, list):
        errors.append(f"{rel_path}: 'inputs' must be a list")
    else:
        for item in inputs:
            if not isinstance(item, dict):
                errors.append(f"{rel_path}: input entry is not an object")
                continue
            digest = item.get("sha256")
            if not isinstance(digest, str) or not SHA256_RE.match(digest):
                errors.append(
                    f"{rel_path}: input sha256 is not a 64 hex digest: {digest!r}"
                )


def check_provenance(manifest: object, report_paths: list) -> list:
    """Return a list of error strings; empty means the gate passes.

    ``report_paths`` is a list of ``(repo_relative_path, absolute_path)`` pairs.
    Fails closed when a report carries a measured number but has no provenance
    entry (or its entry is malformed).
    """
    errors: list = []
    if not isinstance(manifest, dict):
        return ["manifest root is not a JSON object"]
    artifacts = manifest.get("artifacts")
    if not isinstance(artifacts, dict):
        return ["manifest 'artifacts' is missing or not an object"]

    for rel_path, abs_path in report_paths:
        if not os.path.isfile(abs_path):
            errors.append(f"{rel_path}: publication artifact not found")
            continue
        with open(abs_path, "r", encoding="utf-8") as fh:
            body = fh.read()
        entry = artifacts.get(rel_path)
        if not report_has_measured_number(body):
            if entry is not None:
                _validate_entry(rel_path, entry, errors)
            continue
        if entry is None:
            errors.append(
                f"{rel_path}: carries a measured number but has no provenance entry"
            )
            continue
        _validate_entry(rel_path, entry, errors)
    return errors


def default_report_paths() -> list:
    return [(rel, os.path.join(REPO_ROOT, rel)) for rel in PUBLICATION_ARTIFACTS]


def build_manifest() -> dict:
    """Build the manifest from the recorded (pinned) provenance of this wave."""
    cohort_entry = {
        "source_commit": "e855fa6a71631237d5973041caf95664af23590d",
        "generator_command": (
            "OXO_FLOW_BIN=target/release/oxo-flow "
            "python3 eval/benchmarks/cohort_driver.py --samples 10"
        ),
        "host": {
            "os": "macOS 26.6.2 (25G83) / Darwin 25.6.0 arm64",
            "cpu": "Apple M3",
            "logical_cores": 8,
            "memory_bytes": 25769803776,
        },
        "tool_versions": {
            "rustc": "1.98.0 (88d9e12ae 2026-08-18)",
            "cargo": "1.98.0",
            "python3": "3.12.4",
        },
        "inputs": [
            {
                "path": "target/release/oxo-flow",
                "sha256": "81bf9bab8fb253b35f1f04575650c7ea573db878a9fd04957378c7788748809f",
            }
        ],
        "note": (
            "Pins the section 2 reproduction block. The section 1 row (118.69 ms) "
            "is UNPINNED and documented as such; it is not attributable to this commit."
        ),
    }
    artifacts = {"docs/superpowers/plans/cohort-scaling-report.md": cohort_entry}
    return {
        "version": 1,
        "description": (
            "Provenance manifest for publication-facing status reports (#1305). "
            "Key-ordered; reports without measured numbers need no entry."
        ),
        "artifacts": dict(sorted(artifacts.items())),
    }


def main() -> int:
    parser = argparse.ArgumentParser(description="Publication-artifact provenance manifest")
    parser.add_argument("--emit", default=None, help="Write the manifest JSON here")
    parser.add_argument("--check", action="store_true", help="Validate the manifest (fail closed)")
    parser.add_argument("--manifest", default=DEFAULT_MANIFEST, help="Manifest path for --check")
    args = parser.parse_args()

    if args.emit:
        manifest = build_manifest()
        os.makedirs(os.path.dirname(os.path.abspath(args.emit)), exist_ok=True)
        with open(args.emit, "w", encoding="utf-8") as fh:
            json.dump(manifest, fh, indent=2, sort_keys=True)
            fh.write("\n")
        print(f"Provenance manifest written to: {args.emit}")
        return 0

    if args.check:
        if not os.path.isfile(args.manifest):
            print(f"provenance-check: manifest not found: {args.manifest}", file=sys.stderr)
            return 1
        try:
            with open(args.manifest, "r", encoding="utf-8") as fh:
                manifest = json.load(fh)
        except json.JSONDecodeError as exc:
            print(f"provenance-check: manifest is not valid JSON: {exc}", file=sys.stderr)
            return 1
        errors = check_provenance(manifest, default_report_paths())
        if errors:
            print("provenance-check: FAILED", file=sys.stderr)
            for err in errors:
                print(f"  - {err}", file=sys.stderr)
            return 1
        print("provenance-check: OK (manifest well-formed; every measured report has provenance)")
        return 0

    parser.print_help()
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
