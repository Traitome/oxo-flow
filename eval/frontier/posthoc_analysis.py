#!/usr/bin/env python3
"""Post-hoc intent-validity subset analysis for a frontier campaign (#1395).

Runs the frozen statistical analysis over explicit intent populations:

  all36        - the frozen pre-registered population (vague double-counted)
  specified33  - vague excluded (the README-claimed reporting contract)
  sound15      - specified intents the independent audit calls SOUND
  vague3       - the three deliberately vague intents, descriptive only

Writes one JSON with per-subset metrics, comparisons, intent-level
sensitivity, and corrected tier tables. No provider calls.

Usage:
    python3 eval/frontier/posthoc_analysis.py \
        --results-dir eval/frontier/results/<stamp> \
        --out-json /tmp/posthoc.json
"""

from __future__ import annotations

import argparse
import json
import tempfile
from pathlib import Path

from statistical_report import analyze_arms, load_benchmark_results

VAGUE = {"qc-vague", "rnaseq-vague", "variant-vague"}

# Independent audit verdicts: 15 specified SOUND (+3 vague SOUND).
SOUND_SPECIFIED = {
    "qc-fastp-multiqc",
    "qc-fastqc",
    "qc-nanoq",
    "rnaseq-star",
    "rnaseq-hisat2",
    "rnaseq-salmon",
    "germline-variant",
    "somatic-mutect2",
    "chipseq-macs2",
    "atacseq",
    "metagenomics-kraken2",
    "assembly-spades",
    "assembly-flye-polish",
    "spatial-visium",
    "single-cell-multiome",
}

SUBSETS = ("all36", "specified33", "sound15", "vague3")
ARMS = ("unconstrained", "syntax-only", "full-contract")


def tier_table(runs: list[dict]) -> dict:
    table = {arm: {} for arm in ARMS}
    for arm in ARMS:
        for tier in ("easy", "medium", "hard"):
            rs = [r for r in runs if r["variant"] == arm and r["tier"] == tier]
            table[arm][tier] = {
                "pass": sum(bool(r["success"]) for r in rs),
                "n": len(rs),
            }
    return table


def subset_intents(name: str, all_intents: set[str]) -> set[str]:
    if name == "all36":
        return set(all_intents)
    if name == "specified33":
        return set(all_intents) - VAGUE
    if name == "sound15":
        return set(SOUND_SPECIFIED) & set(all_intents)
    if name == "vague3":
        return set(VAGUE) & set(all_intents)
    raise ValueError(f"unknown subset: {name}")


def analyze(root: Path) -> dict:
    payload = json.loads((root / "results.json").read_text())
    all_intents = {r["intent_id"] for r in payload["runs"]}
    out = {
        "campaign": payload["stamp"],
        "generator": "eval/frontier/posthoc_analysis.py",
        "subsets": {},
    }
    for name in SUBSETS:
        ids = subset_intents(name, all_intents)
        runs = [r for r in payload["runs"] if r["intent_id"] in ids]
        with tempfile.TemporaryDirectory() as tmp:
            (Path(tmp) / "results.json").write_text(json.dumps({"runs": runs}))
            dataset = load_benchmark_results(Path(tmp))
            report = analyze_arms(dataset)
        report.pop("source_files", None)
        out["subsets"][name] = {
            "intents": sorted(ids),
            "n_intents": len(ids),
            "n_cells_per_arm": len(ids) * payload["seeds"],
            "metrics": report["metrics"],
            "comparisons": report["comparisons"],
            "intent_level_sensitivity": report["intent_level_sensitivity"],
            "tiers": tier_table(runs),
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
    for name, sub in report["subsets"].items():
        print(f"{name}: {sub['n_intents']} intents, "
              f"{sub['n_cells_per_arm']} cells/arm")
    print(f"wrote {args.out_json}")


if __name__ == "__main__":
    main()
