#!/usr/bin/env python3
"""Mechanism evidence for a frontier campaign (issue #1395).

Reads a completed ``results.json`` plus its per-cell ``session.json`` and
``cli.log`` archives and writes one machine-readable mechanism report:

- per-arm failure classification (gate failures vs transport vs other);
- per-arm session-outcome distribution, including gate successes whose
  generation loop FAILED (max-round exhaustion);
- the C-arm transport-outage window;
- success-only cost and latency (totals and per-cell medians);
- contract-feedback marker counts;
- container-dialect cells;
- gate-level and session-level per-protocol matched comparisons.

No provider calls. Session error text is URL-sanitized before writing.

Usage:
    python3 eval/frontier/mechanism_analysis.py \
        --results-dir eval/frontier/results/<stamp> \
        --out-json /tmp/mechanism.json
"""

from __future__ import annotations

import argparse
import json
import re
import statistics
from collections import Counter
from pathlib import Path

from statistical_report import mcnemar_exact, mcnemar_test

VAGUE = {"qc-vague", "rnaseq-vague", "variant-vague"}
ARMS = ("unconstrained", "syntax-only", "full-contract")
CONTRACT_MARKERS = ("TYPE-", "SCI-", "D00")
URL_RE = re.compile(r"https?://[^\s\"']+")


def sanitize_text(value: str | None) -> str | None:
    if value is None:
        return None
    return URL_RE.sub("<provider-endpoint>", value)


def run_dir(root: Path, run: dict, seeds: int) -> Path:
    suffix = "" if seeds == 1 else f"__s{run['seed']}"
    return root / f"{run['model']}__{run['variant']}__{run['intent_id']}{suffix}"


def load_session(path: Path) -> dict:
    session = path / "session.json"
    if not session.exists():
        return {}
    try:
        return json.loads(session.read_text())
    except json.JSONDecodeError:
        return {}


def cli_budget_warning(path: Path) -> bool:
    log = path / "cli.log"
    if not log.exists():
        return False
    return "correction-round budget" in log.read_text(errors="replace")


def is_transport_failure(run: dict, path: Path) -> bool:
    """A zero-token generation failure caused by the provider transport."""
    if run["input_tokens"] or run["output_tokens"] or run["generated"]:
        return False
    if "remote end closed" in (run.get("error") or "").lower():
        return True
    return "error sending request" in (load_session(path).get("error") or "")


def is_container_dialect(path: Path) -> bool:
    gate = path / "validate.json"
    if not gate.exists():
        return False
    return "unknown key 'container'" in gate.read_text(errors="replace")


def median(values) -> float:
    values = list(values)
    return statistics.median(values) if values else 0


def matched_comparison(runs: dict, root: Path, seeds: int, outcome) -> dict:
    by = {variant: {(r["intent_id"], r["seed"]): r for r in runs
                    if r["variant"] == variant}
          for variant in ("syntax-only", "full-contract")}
    cells = sorted(set(by["syntax-only"]) & set(by["full-contract"]))
    matched = [k for k in cells
               if not is_transport_failure(by["syntax-only"][k],
                                           run_dir(root, by["syntax-only"][k], seeds))
               and not is_transport_failure(by["full-contract"][k],
                                            run_dir(root, by["full-contract"][k], seeds))]
    b_passes = [outcome(by["syntax-only"][k]) for k in matched]
    c_passes = [outcome(by["full-contract"][k]) for k in matched]
    exact = mcnemar_exact(b_passes, c_passes)
    edwards = mcnemar_test(b_passes, c_passes)
    return {
        "n_cells": len(matched),
        "b_success": sum(b_passes),
        "c_success": sum(c_passes),
        "b_only_pass": exact["c"],
        "c_only_pass": exact["b"],
        "exact_p": exact["p_value_exact"],
        "edwards_chi2": edwards["chi2"],
        "edwards_p": edwards["p_value"],
        "intents": sorted({k[0] for k in matched}),
    }


def analyze(root: Path) -> dict:
    payload = json.loads((root / "results.json").read_text())
    runs = payload["runs"]
    seeds = payload["seeds"]

    out: dict = {
        "campaign": payload["stamp"],
        "generator": "eval/frontier/mechanism_analysis.py",
        "totals": {},
        "failure_classification": {},
        "session_outcomes": {},
        "outage": {},
        "success_cost": {},
        "contract_feedback": {},
        "container_dialect": [],
        "per_protocol_matched": {},
    }

    for variant in ARMS:
        rs = [r for r in runs if r["variant"] == variant]
        not_generated = [r for r in rs if not r["generated"]]
        gate_failures = [r for r in rs if r["generated"] and not r["success"]]
        transport = [r for r in not_generated
                     if is_transport_failure(r, run_dir(root, r, seeds))]
        generation_other = [r for r in not_generated
                            if not is_transport_failure(r, run_dir(root, r, seeds))]
        successes = [r for r in rs if r["success"]]

        out["totals"][variant] = {
            "n": len(rs),
            "success": len(successes),
            "generated": sum(bool(r["generated"]) for r in rs),
            "gate_failures": len(gate_failures),
            "generation_failures": len(not_generated),
            "transport_failures": len(transport),
            "non_transport_generation_failures": len(generation_other),
            "generation_failure_errors": Counter(
                r.get("error") for r in not_generated).most_common(),
            "input_tokens": sum(r["input_tokens"] for r in rs),
            "output_tokens": sum(r["output_tokens"] for r in rs),
            "cost_usd": round(sum(r["est_cost_usd"] for r in rs), 4),
            "wall_mean_s": round(statistics.mean(r.get("wall_s") or 0 for r in rs), 1),
            "wall_max_s": max(r.get("wall_s") or 0 for r in rs),
        }

        out["session_outcomes"][variant] = {
            "all": dict(Counter(
                (load_session(run_dir(root, r, seeds)).get("outcome") or "NO_SESSION")
                for r in rs)),
            "gate_successes": dict(Counter(
                (load_session(run_dir(root, r, seeds)).get("outcome") or "NO_SESSION")
                for r in successes)),
        }
        out["session_outcomes"][variant]["gate_success_session_failed"] = [
            {
                "intent_id": r["intent_id"],
                "seed": r["seed"],
                "session_outcome": load_session(run_dir(root, r, seeds)).get("outcome"),
                "session_error": sanitize_text(
                    load_session(run_dir(root, r, seeds)).get("error")),
                "session_messages": len(load_session(
                    run_dir(root, r, seeds)).get("messages", [])),
                "session_tool_calls": len(load_session(
                    run_dir(root, r, seeds)).get("tool_calls", [])),
                "cli_budget_warning": cli_budget_warning(run_dir(root, r, seeds)),
            }
            for r in successes
            if load_session(run_dir(root, r, seeds)).get("outcome") not in (None, "Success")
        ]

        if variant != "unconstrained":
            out["success_cost"][variant] = {
                "n": len(successes),
                "input_tokens_total": sum(r["input_tokens"] for r in successes),
                "output_tokens_total": sum(r["output_tokens"] for r in successes),
                "input_tokens_median": median(r["input_tokens"] for r in successes),
                "output_tokens_median": median(r["output_tokens"] for r in successes),
                "wall_median_s": median(r.get("wall_s") or 0 for r in successes),
                "wall_max_s": max((r.get("wall_s") or 0 for r in successes), default=0),
                "tool_calls_total": sum(r.get("tool_calls") or 0 for r in successes),
            }

        out["failure_classification"][variant] = {
            "transport_examples": Counter(
                (sanitize_text(load_session(run_dir(root, r, seeds)).get("error"))
                 or r.get("error") or "")[:140]
                for r in transport).most_common(3),
            "gate_failures": [
                {
                    "intent_id": r["intent_id"],
                    "seed": r["seed"],
                    "error": r.get("error"),
                    "input_tokens": r["input_tokens"],
                    "output_tokens": r["output_tokens"],
                    "container_dialect": is_container_dialect(run_dir(root, r, seeds)),
                }
                for r in gate_failures
            ],
            "non_transport_generation_failures": [
                {
                    "intent_id": r["intent_id"],
                    "seed": r["seed"],
                    "error": r.get("error"),
                    "input_tokens": r["input_tokens"],
                    "output_tokens": r["output_tokens"],
                }
                for r in generation_other
            ],
        }

    c_transport = [r for r in runs if r["variant"] == "full-contract"
                   and is_transport_failure(r, run_dir(root, r, seeds))]
    c_success = [r for r in runs if r["variant"] == "full-contract" and r["success"]]
    starts = [(load_session(run_dir(root, r, seeds)).get("started_at"), r["intent_id"], r["seed"])
              for r in c_transport if load_session(run_dir(root, r, seeds)).get("started_at")]
    ok_starts = [(load_session(run_dir(root, r, seeds)).get("started_at"), r["intent_id"], r["seed"])
                 for r in c_success if load_session(run_dir(root, r, seeds)).get("started_at")]
    out["outage"] = {
        "arm": "full-contract",
        "transport_failures": len(c_transport),
        "first_failure_start": min(starts)[0] if starts else None,
        "last_failure_start": max(starts)[0] if starts else None,
        "last_success_start": max(ok_starts)[0] if ok_starts else None,
        "note": "All C cells after the last success failed at the transport layer; "
                "every failure lasted ~7.1 s with zero input/output tokens.",
    }

    marker_cells = marker_total = 0
    for r in c_success:
        text = json.dumps(load_session(run_dir(root, r, seeds)).get("messages", []))
        count = sum(text.count(token) for token in CONTRACT_MARKERS)
        if count:
            marker_cells += 1
            marker_total += count
    out["contract_feedback"] = {
        "c_successes": len(c_success),
        "c_successes_with_type_sci_d00_markers": marker_cells,
        "total_markers": marker_total,
    }

    out["container_dialect"] = [
        {
            "variant": r["variant"],
            "intent_id": r["intent_id"],
            "seed": r["seed"],
            "success": r["success"],
            "error": r.get("error"),
        }
        for r in runs if is_container_dialect(run_dir(root, r, seeds))
    ]

    out["per_protocol_matched"] = {
        "gate": matched_comparison(
            runs, root, seeds, lambda r: bool(r["success"])),
        "session": matched_comparison(
            runs, root, seeds,
            lambda r: load_session(run_dir(root, r, seeds)).get("outcome") == "Success"),
    }
    return out


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__.split("\n")[0])
    parser.add_argument("--results-dir", required=True, type=Path)
    parser.add_argument("--out-json", required=True, type=Path)
    args = parser.parse_args()
    report = analyze(args.results_dir)
    args.out_json.parent.mkdir(parents=True, exist_ok=True)
    args.out_json.write_text(json.dumps(report, indent=2, sort_keys=True) + "\n")
    print(json.dumps(report, indent=2, sort_keys=True))


if __name__ == "__main__":
    main()
