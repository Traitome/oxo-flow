#!/usr/bin/env python3
"""Regeneration gate for the publication-facing reports' generator outputs (#1305).

Ledger §7 wave 5 / issue #1305 acceptance 2: a committed, documented
regeneration command that re-runs the ``eval/benchmarks`` + ``eval/frontier``
generators and either byte-reproduces the committed artifacts or fails closed
with a diff. ``make reports-regen`` runs this checker (wired into ``ci``).

What is gated (deterministic on ANY host, provider-free, offline):

  * ``sus_survey_analysis.py``            -> byte-diff of the markdown report
  * ``statistical_report.py``             -> byte-diff of the markdown report
      (``--synthetic-fixture``, seeded RNG 12345/42)
  * ``hardware_scaling.py``               -> selective JSON diff of the model
      projection (``--allow-simulation``); host identity excluded, see below
  * ``cohort_driver.py``                  -> selective JSON diff of the plan
      contract (``--samples 10``, real ``dry-run --json`` through the built
      binary); latency/throughput excluded, see below

What is deliberately NOT byte-gated, stated honestly:

  * the hand-authored report bodies under ``docs/superpowers/plans/`` are NOT
    generator output (ledger wave-5 honesty finding, PR #1307): the generators
    emit raw tables, the reports state evidence status. Their attribution is
    gated by ``make provenance-check`` instead.
  * ``host_inventory`` in the hardware JSON is HOST IDENTITY (cores, memory,
    OS), not a measurement — excluded by name, never compared.
  * ``expansion_latency_ms`` / ``throughput_nodes_per_sec`` in the cohort JSON
    are wall-clock timings of the local build — host- and build-dependent, so
    cross-host byte equality is impossible; excluded by name. The deterministic
    plan contract (``num_samples``/``dag_nodes``/parse flags) IS gated.

Fail-closed rules:

  * every compared JSON object must have EXACTLY the expected key set: a key
    the generator adds that is neither compared nor explicitly excluded is an
    error, so new fields can never silently escape the gate;
  * any byte or value drift exits 1 with a unified diff / per-key report;
  * a missing engine binary for the cohort arm is an error (``make ci`` builds
    before this gate runs; ``eval-lint`` has the same requirement).

Usage:
  python3 eval/benchmarks/reports_regen.py --check   # gate (exit 1 on drift)
  python3 eval/benchmarks/reports_regen.py --emit    # refresh snapshots after
                                                     # a DELIBERATE generator
                                                     # change; review the diff
"""

from __future__ import annotations

import argparse
import difflib
import json
import os
import subprocess
import sys
import tempfile

REPO_ROOT = os.path.abspath(os.path.join(os.path.dirname(__file__), "..", ".."))
SNAPSHOT_DIR = os.path.join(REPO_ROOT, "eval", "benchmarks", "snapshots")

# sys.path for eval/scripts/common.py (find_engine_binary) — reused, not retyped.
sys.path.insert(0, os.path.join(REPO_ROOT, "eval", "scripts"))
import common  # noqa: E402  (eval/scripts/common.py)


def _run(cmd):
    """Run a generator from the repo root; raise on failure."""
    proc = subprocess.run(
        cmd, cwd=REPO_ROOT, capture_output=True, text=True, timeout=900
    )
    if proc.returncode != 0:
        raise RuntimeError(
            "generator failed (%s): exit %d\n%s"
            % (" ".join(cmd), proc.returncode, (proc.stderr or proc.stdout)[-800:])
        )


def _execute(cmd, scratch, name):
    out = os.path.join(scratch, name)
    _run([c if c != "@@" else out for c in cmd])
    with open(out, "r", encoding="utf-8") as fh:
        return fh.read()


def _diff_text(expected: str, actual: str, label: str):
    if expected == actual:
        return None
    lines = list(
        difflib.unified_diff(
            expected.splitlines(),
            actual.splitlines(),
            fromfile="committed/" + label,
            tofile="regenerated/" + label,
            lineterm="",
        )
    )
    return "\n".join(lines[:60]) or "(empty diff)"


def _diff_compared_keys(expected_text, actual_text, label, compared, excluded):
    """Compare only ``compared`` keys; require compared+excluded == key set."""
    expected = json.loads(expected_text)
    actual = json.loads(actual_text)
    problems = []
    for obj_name, obj in (("snapshot", expected), ("regenerated", actual)):
        extra = set(obj) - compared - excluded
        if extra:
            problems.append(
                "%s %s: new key(s) %s are neither compared nor explicitly "
                "excluded — extend reports_regen.py deliberately"
                % (label, obj_name, sorted(extra))
            )
        missing = compared - set(obj)
        if missing:
            problems.append(
                "%s %s: compared key(s) %s absent" % (label, obj_name, sorted(missing))
            )
    if problems:
        return "\n".join(problems)
    diffs = []
    for key in sorted(compared):
        if expected[key] != actual[key]:
            diffs.append(
                "%s: %s\n  committed: %s\n  regenerated: %s"
                % (label, key, json.dumps(expected[key], sort_keys=True)[:300],
                   json.dumps(actual[key], sort_keys=True)[:300])
            )
    return "\n".join(diffs) if diffs else None


# ── check table ──────────────────────────────────────────────────────────────
# "@@" in a command is the placeholder for this check's output path.

MARKDOWN_CHECKS = [
    (
        [sys.executable, "eval/benchmarks/sus_survey_analysis.py", "--out-md", "@@"],
        "sus_survey_harness_check.md",
    ),
    (
        [sys.executable, "eval/frontier/statistical_report.py",
         "--synthetic-fixture", "--out-md", "@@"],
        "statistical_synthetic_fixture.md",
    ),
]

JSON_CHECKS = [
    (
        [sys.executable, "eval/benchmarks/hardware_scaling.py",
         "--allow-simulation", "--out-json", "@@"],
        "hardware_scaling_projection.json",
        {"evidence_mode", "inferential", "interpretation_warning",
         "target_topology", "results"},
        {"host_inventory"},  # host identity, not a measurement
    ),
]

# The cohort summary nests its rows under "results", so the per-result
# latency/throughput exclusions are named inside _cohort_payload.
COHORT_CMD = [sys.executable, "eval/benchmarks/cohort_driver.py",
              "--samples", "10", "--out-json", "@@"]
COHORT_SNAPSHOT = "cohort_scaling_n10.json"
COHORT_EXCLUDED_ROW_FIELDS = {"expansion_latency_ms", "throughput_nodes_per_sec"}


def _cohort_payload(text: str) -> dict:
    """Flatten the cohort summary to a dict of deterministic keys only."""
    summary = json.loads(text)
    extra_top = set(summary) - {"benchmark", "results"}
    if extra_top:
        raise RuntimeError(
            "cohort_scaling_n10.json: unexpected top-level key(s) %s — extend "
            "reports_regen.py deliberately" % sorted(extra_top)
        )
    compared = {"benchmark": summary["benchmark"],
                "results_count": len(summary["results"])}
    for row in summary["results"]:
        size = row["num_samples"]
        extra = set(row) - set(COHORT_EXCLUDED_ROW_FIELDS) - {
            "num_samples", "dag_nodes", "parse_success", "measurement_valid",
        }
        if extra:
            raise RuntimeError(
                "cohort results[%d]: new field(s) %s are neither compared nor "
                "excluded — extend reports_regen.py deliberately"
                % (size, sorted(extra))
            )
        compared["results[%d].dag_nodes" % size] = row["dag_nodes"]
        compared["results[%d].parse_success" % size] = row["parse_success"]
        compared["results[%d].measurement_valid" % size] = row["measurement_valid"]
    return compared


def _emit(scratch: str) -> int:
    os.makedirs(SNAPSHOT_DIR, exist_ok=True)
    for cmd, name in MARKDOWN_CHECKS:
        body = _execute(cmd, scratch, name)
        with open(_snapshot(name), "w", encoding="utf-8") as fh:
            fh.write(body)
        print("emitted snapshot: %s" % _snapshot(name))
    for cmd, name, _compared, _excluded in JSON_CHECKS:
        body = _execute(cmd, scratch, name)
        with open(_snapshot(name), "w", encoding="utf-8") as fh:
            fh.write(body)
        print("emitted snapshot: %s" % _snapshot(name))
    body = _execute(COHORT_CMD, scratch, COHORT_SNAPSHOT)
    # _cohort_payload also enforces the key-set rule before anything is written.
    _cohort_payload(body)
    with open(_snapshot(COHORT_SNAPSHOT), "w", encoding="utf-8") as fh:
        fh.write(body)
    print("emitted snapshot: %s" % _snapshot(COHORT_SNAPSHOT))
    return 0


def _snapshot(name: str) -> str:
    return os.path.join(SNAPSHOT_DIR, name)


def _check(scratch: str) -> int:
    problems = []
    for cmd, name in MARKDOWN_CHECKS:
        actual = _execute(cmd, scratch, name)
        with open(_snapshot(name), "r", encoding="utf-8") as fh:
            expected = fh.read()
        diff = _diff_text(expected, actual, name)
        if diff:
            problems.append(diff)
        else:
            print("reports-regen: byte-identical: %s" % name)
    for cmd, name, compared, excluded in JSON_CHECKS:
        actual = _execute(cmd, scratch, name)
        with open(_snapshot(name), "r", encoding="utf-8") as fh:
            expected = fh.read()
        diff = _diff_compared_keys(expected, actual, name, compared, excluded)
        if diff:
            problems.append(diff)
        else:
            print("reports-regen: deterministic keys identical: %s" % name)
    actual = _execute(COHORT_CMD, scratch, COHORT_SNAPSHOT)
    with open(_snapshot(COHORT_SNAPSHOT), "r", encoding="utf-8") as fh:
        expected = fh.read()
    try:
        exp_payload = json.dumps(_cohort_payload(expected), sort_keys=True, indent=1)
        act_payload = json.dumps(_cohort_payload(actual), sort_keys=True, indent=1)
        diff = _diff_text(exp_payload, act_payload, COHORT_SNAPSHOT)
    except RuntimeError as exc:
        diff = str(exc)
    if diff:
        problems.append(diff)
    else:
        print("reports-regen: deterministic plan contract identical: %s"
              % COHORT_SNAPSHOT)
    if problems:
        print("reports-regen: FAILED — committed snapshots drifted from the "
              "generators", file=sys.stderr)
        for p in problems:
            print("---\n%s" % p, file=sys.stderr)
        print("If the generator change is deliberate, refresh with "
              "`python3 eval/benchmarks/reports_regen.py --emit`, review the "
              "snapshot diff, and commit it.", file=sys.stderr)
        return 1
    print("reports-regen: OK (all deterministic generator outputs reproduce)")
    return 0


def main() -> int:
    parser = argparse.ArgumentParser(
        description="Regeneration gate for publication-facing report generators (#1305)"
    )
    group = parser.add_mutually_exclusive_group(required=True)
    group.add_argument("--check", action="store_true", help="fail closed on drift")
    group.add_argument("--emit", action="store_true",
                       help="refresh committed snapshots (deliberate changes only)")
    args = parser.parse_args()

    # The cohort arm needs the real engine binary (same requirement as
    # eval-lint); resolve through the shared helper, never a local copy.
    try:
        common.find_engine_binary()
    except FileNotFoundError as exc:
        print("reports-regen: engine binary missing (%s); build it first "
              "(make build / cargo build -p oxo-flow-cli)" % exc, file=sys.stderr)
        return 1

    with tempfile.TemporaryDirectory(prefix="oxoflow_reports_regen_") as scratch:
        if args.emit:
            return _emit(scratch)
        return _check(scratch)


if __name__ == "__main__":
    raise SystemExit(main())
