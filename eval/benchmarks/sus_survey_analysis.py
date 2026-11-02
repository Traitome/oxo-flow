#!/usr/bin/env python3
"""Synthetic System Usability Scale (SUS) harness-validation tool.

This tool exercises deterministic questionnaire-scoring code on generic synthetic
data. It computes SUS values, Sauro-Lewis grade bands, and NASA-TLX aggregates,
but it does not estimate empirical human-factors outcomes or compare workflow
engines.

Usage:
  python3 eval/benchmarks/sus_survey_analysis.py [--cases 30] [--out-md report.md] [--out-json report.json]
"""

from __future__ import annotations

import argparse
import json
import math
import random
from pathlib import Path
from typing import Any, Dict, List, Tuple


def calculate_sus_score(responses: List[int]) -> float:
    """Calculate the standardized SUS score from ten Likert responses (1-5)."""
    assert len(responses) == 10, "SUS requires exactly 10 responses"
    total_contribution = 0
    for idx, response in enumerate(responses):
        if idx % 2 == 0:
            total_contribution += response - 1
        else:
            total_contribution += 5 - response
    return total_contribution * 2.5


def sus_to_grade_and_adjective(score: float) -> Tuple[str, str]:
    """Map a SUS score to the Sauro-Lewis grade bands."""
    if score >= 84.1:
        return "A+", "Best Imaginable"
    if score >= 80.8:
        return "A", "Excellent"
    if score >= 78.9:
        return "A-", "Good"
    if score >= 74.1:
        return "B", "Good"
    if score >= 68.0:
        return "C", "OK"
    if score >= 51.7:
        return "D", "Poor"
    return "F", "Worst Imaginable"


def mean_and_std(data: List[float]) -> Tuple[float, float]:
    """Compute the sample mean and sample standard deviation."""
    n = len(data)
    if n == 0:
        return 0.0, 0.0
    mean_value = sum(data) / n
    if n == 1:
        return mean_value, 0.0
    variance = sum((value - mean_value) ** 2 for value in data) / (n - 1)
    return mean_value, math.sqrt(variance)


def generate_synthetic_fixture(n_cases: int = 30) -> Dict[str, Any]:
    """Generate one generic fixture stream; these are not study observations."""
    if n_cases < 2:
        raise ValueError("synthetic fixture requires at least two cases")

    rng = random.Random(999)
    responses = []
    tlx_values = []
    ttfp_values = []
    for _ in range(n_cases):
        response_vector = [
            rng.choice([3, 4, 5]),
            rng.choice([1, 2, 3]),
            rng.choice([3, 4, 5]),
            rng.choice([1, 2, 3]),
            rng.choice([3, 4, 5]),
            rng.choice([1, 2, 3]),
            rng.choice([3, 4, 5]),
            rng.choice([1, 2, 3]),
            rng.choice([3, 4, 5]),
            rng.choice([1, 2, 3]),
        ]
        responses.append(response_vector)
        tlx_values.append(round(rng.gauss(50.0, 10.0), 1))
        ttfp_values.append(round(rng.gauss(600.0, 90.0), 1))

    return {
        "sus_scores": [calculate_sus_score(vector) for vector in responses],
        "sus_responses": responses,
        "nasa_tlx": tlx_values,
        "ttfp_secs": ttfp_values,
    }


def analyze_synthetic_fixture(dataset: Dict[str, Any]) -> Dict[str, Any]:
    """Summarize one fixture stream without estimating effects or testing hypotheses."""
    sus_mean, sus_sd = mean_and_std(dataset["sus_scores"])
    tlx_mean, tlx_sd = mean_and_std(dataset["nasa_tlx"])
    ttfp_mean, ttfp_sd = mean_and_std(dataset["ttfp_secs"])
    grade, adjective = sus_to_grade_and_adjective(sus_mean)

    return {
        "evidence_status": "simulation_only",
        "inferential_analysis_performed": False,
        "fixture_cases": len(dataset["sus_scores"]),
        "metrics": {
            "sus_mean": round(sus_mean, 1),
            "sus_std": round(sus_sd, 1),
            "grade": grade,
            "adjective": adjective,
            "nasa_tlx_mean": round(tlx_mean, 1),
            "nasa_tlx_std": round(tlx_sd, 1),
            "ttfp_mean_secs": round(ttfp_mean, 1),
            "ttfp_std_secs": round(ttfp_sd, 1),
        },
    }


def render_markdown_report(report: Dict[str, Any]) -> str:
    """Render a fixture-validation report that cannot be mistaken for a study."""
    if (
        report.get("evidence_status") != "simulation_only"
        or report.get("inferential_analysis_performed") is not False
    ):
        raise ValueError("only non-inferential synthetic reports may be rendered")

    metrics = report["metrics"]
    ttfp_minutes = (
        f"{metrics['ttfp_mean_secs'] / 60.0:.1f} +/- "
        f"{metrics['ttfp_std_secs'] / 60.0:.1f} min"
    )
    return "\n".join(
        [
            "# Synthetic Human-Factors Harness Check",
            "",
            "> **SIMULATION ONLY**: all values below are fixed synthetic test vectors.",
            "> They verify questionnaire arithmetic and aggregation code. They are not",
            "> measurements, projections, effect estimates, or evidence about any workflow",
            "> engine. This report performs no hypothesis test and supports no comparative",
            "> claim.",
            "",
            "## 1. Generic Fixture Output",
            "",
            "This one-row report only demonstrates that parsing, reverse scoring, means,",
            "and standard deviations execute. It does not describe a system or population.",
            "",
            "| Fixture | Synthetic SUS Value | Scoring-Code Grade | Fixture TLX Value | Fixture TTFP Value |",
            "| :--- | :---: | :---: | :---: | :---: |",
            f"| {report['fixture_cases']} generic cases | "
            f"{metrics['sus_mean']:.1f} +/- {metrics['sus_std']:.1f} | "
            f"{metrics['grade']} | {metrics['nasa_tlx_mean']:.1f} +/- "
            f"{metrics['nasa_tlx_std']:.1f} | {ttfp_minutes} |",
            "",
            "## 2. Interpretation Boundary",
            "",
            "- This file cannot be cited as a usability result.",
            "- No confidence interval, effect size, hypothesis test, or causal statement is produced.",
            "- A real evaluation requires a pre-registered protocol, ethics approval, complete task",
            "  outcomes, missing-data rules, and an independent analysis.",
        ]
    )


def main() -> None:
    parser = argparse.ArgumentParser(description="Synthetic SUS scoring-fixture harness")
    parser.add_argument("--cases", type=int, default=30, help="Number of synthetic fixture cases")
    parser.add_argument("--out-json", default=None, help="Export path for the fixture JSON")
    parser.add_argument(
        "--out-md",
        default=None,
        help="Export path for the non-inferential Markdown report",
    )
    args = parser.parse_args()

    dataset = generate_synthetic_fixture(n_cases=args.cases)
    report = analyze_synthetic_fixture(dataset)
    metrics = report["metrics"]

    print("=" * 80)
    print("SYNTHETIC SUS HARNESS CHECK - NOT A STUDY OR EMPIRICAL ESTIMATE")
    print(f"Synthetic fixture cases: {report['fixture_cases']}")
    print("=" * 80)
    print(
        f"Generic fixture | SUS: {metrics['sus_mean']:4.1f} +/- "
        f"{metrics['sus_std']:3.1f} ({metrics['grade']}) | "
        f"TLX: {metrics['nasa_tlx_mean']:4.1f} | "
        f"TTFP: {metrics['ttfp_mean_secs'] / 60.0:4.1f} min"
    )
    print("-" * 80)
    print("No hypothesis test or comparative result was produced.")
    print("=" * 80)

    if args.out_json:
        out_json = Path(args.out_json)
        out_json.parent.mkdir(parents=True, exist_ok=True)
        out_json.write_text(json.dumps(report, indent=2) + "\n")
        print(f"Fixture JSON exported to: {out_json}")

    if args.out_md:
        out_markdown = Path(args.out_md)
        out_markdown.parent.mkdir(parents=True, exist_ok=True)
        out_markdown.write_text(render_markdown_report(report))
        print(f"Non-inferential Markdown exported to: {out_markdown}")


if __name__ == "__main__":
    main()
