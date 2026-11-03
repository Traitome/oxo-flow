#!/usr/bin/env python3
"""Paired statistical analysis for oxo-flow benchmark results.

The inferential path fails closed: it requires measured ``results.json`` files
and exactly matched cells for all three arms. A deterministic synthetic fixture
is available only behind ``--synthetic-fixture`` for testing the analysis code;
fixture output is explicitly non-inferential.

Metrics:
1. pass@k estimation (unbiased combinatorial estimator, Chen et al. (HumanEval)).
2. Empirical bootstrap 95% confidence intervals (percentile bootstrap).
3. McNemar's paired test with continuity correction & two-tailed p-values.
4. Token expenditure, cache hit ratios, and amortized computational costs.

Comparison Arms:
- Arm A: Naive Baseline (Single-shot prompt, zero domain knowledge, no static gates).
- Arm B: CLI Agent (Multi-turn tool-calling loop, Bioconda/Bio.tools retrieval).
- Arm C: Type-Directed Continuum (BioTypes static lattice and scientific preflight).

Usage:
  python3 eval/frontier/statistical_report.py --results-dir DIR [--out-json PATH] [--out-md PATH]
  python3 eval/frontier/statistical_report.py --synthetic-fixture  # test-double only
"""

from __future__ import annotations

import argparse
import json
import math
import random
import sys
from dataclasses import asdict, dataclass
from pathlib import Path
from typing import Any, Dict, List, Optional, Tuple


class AnalysisError(ValueError):
    """Raised when an inferential report would be unsupported by the data."""


def pass_at_k(n: int, c: int, k: int) -> float:
    """Computes unbiased pass@k estimator (Chen et al. (HumanEval)).

    pass@k = 1 - prod_{i=1}^k (n - c - i + 1) / (n - i + 1)

    Degenerate inputs fail closed: an empty set (n <= 0) or k > n cannot
    draw k samples, so the estimate is 0.0. That domain guard must run
    before the `n - c < k` shortcut, or pass_at_k(0, 0, 1) would return 1.0.
    """
    if k > n or n <= 0:
        return 0.0
    if n - c < k:
        return 1.0
    product = 1.0
    for i in range(1, k + 1):
        product *= (n - c - i + 1) / (n - i + 1)
    return 1.0 - product


def bootstrap_ci(
    data: List[float],
    rounds: int = 2000,
    alpha: float = 0.05,
    seed: int = 42,
) -> Tuple[float, float, float]:
    """Computes empirical percentile bootstrap confidence interval [lower, upper] and mean."""
    if not data:
        return 0.0, 0.0, 0.0
    n = len(data)
    mean_val = sum(data) / n
    if n == 1:
        return mean_val, mean_val, mean_val

    rng = random.Random(seed)
    boot_means = []
    for _ in range(rounds):
        sample = [rng.choice(data) for _ in range(n)]
        boot_means.append(sum(sample) / n)

    boot_means.sort()
    low_idx = int((alpha / 2.0) * rounds)
    high_idx = int((1.0 - alpha / 2.0) * rounds)
    low_idx = max(0, min(low_idx, rounds - 1))
    high_idx = max(0, min(high_idx, rounds - 1))

    return mean_val, boot_means[low_idx], boot_means[high_idx]


def erfc_approx(x: float) -> float:
    """Complementary error function approximation (Abramowitz & Stegun 7.1.26)."""
    if x < 0:
        return 2.0 - erfc_approx(-x)
    p = 0.3275911
    a1 = 0.254829592
    a2 = -0.284496736
    a3 = 1.421413741
    a4 = -1.453152027
    a5 = 1.061405429
    t = 1.0 / (1.0 + p * x)
    poly = t * (a1 + t * (a2 + t * (a3 + t * (a4 + t * a5))))
    return poly * math.exp(-x * x)


def chi2_p_value_df1(chi2_stat: float) -> float:
    """Computes two-tailed p-value for 1 degree-of-freedom chi-squared statistic."""
    if chi2_stat <= 0:
        return 1.0
    # For df=1, chi-square p-value equals erfc(sqrt(chi2 / 2))
    return erfc_approx(math.sqrt(chi2_stat / 2.0))


def mcnemar_test(arm1_passes: List[bool], arm2_passes: List[bool]) -> Dict[str, Any]:
    """Computes McNemar's test for paired binary classification with continuity correction.

    Contingency matrix:
      b: arm1=False, arm2=True (discordant in favor of arm2)
      c: arm1=True, arm2=False (discordant in favor of arm1)
    """
    assert len(arm1_passes) == len(arm2_passes), "Arm sample lengths must match"
    a = b = c = d = 0
    for p1, p2 in zip(arm1_passes, arm2_passes):
        if p1 and p2:
            a += 1
        elif not p1 and p2:
            b += 1
        elif p1 and not p2:
            c += 1
        else:
            d += 1

    discordant = b + c
    if discordant == 0:
        return {
            "a": a,
            "b": b,
            "c": c,
            "d": d,
            "chi2": 0.0,
            "p_value": 1.0,
            "odds_ratio": 1.0,
            "significant": False,
        }

    # Edwards continuity correction
    chi2 = ((abs(b - c) - 1.0) ** 2) / discordant
    p_val = chi2_p_value_df1(chi2)
    odds_ratio = (b / c) if c > 0 else float("inf")

    return {
        "a": a,
        "b": b,
        "c": c,
        "d": d,
        "chi2": round(chi2, 4),
        "p_value": p_val,
        "odds_ratio": round(odds_ratio, 3) if odds_ratio != float("inf") else "inf",
        "significant": p_val < 0.05,
    }


def holm_adjust_p_values(comparisons: Dict[str, Dict[str, Any]]) -> None:
    """Adds Holm step-down adjusted p-values to a fixed family of tests.

    Mutates each comparison in place with ``p_value_holm`` (monotone step-down
    over raw p-values) and flips ``significant`` to judge the adjusted value —
    the significance column then controls the family-wise error rate, not the
    per-test rate.
    """
    m = len(comparisons)
    ordered = sorted(comparisons.items(), key=lambda kv: kv[1]["p_value"])
    running_max = 0.0
    for rank, (_name, res) in enumerate(ordered):
        adjusted = min(1.0, (m - rank) * res["p_value"])
        running_max = max(running_max, adjusted)  # enforce monotonicity
        res["p_value_holm"] = round(running_max, 6)
        res["significant"] = res["p_value_holm"] < 0.05


def generate_synthetic_benchmark_dataset(n_intents: int = 36) -> Dict[str, Any]:
    """Generate a deterministic test-double dataset for analysis-code tests."""
    rng = random.Random(12345)

    arm_a_results = []  # Naive baseline
    arm_b_results = []  # CLI agent
    arm_c_results = []  # Type-directed continuum

    for i in range(n_intents):
        # Arm A: ~30% pass rate (frequent syntax / biological typing errors)
        pass_a = rng.random() < 0.32
        tokens_a = rng.randint(800, 1600)
        cost_a = (tokens_a / 1_000_000) * 2.50

        # Arm B: ~75% pass rate (tool search grounded, but lacks type unification)
        pass_b = rng.random() < 0.78
        tokens_b = rng.randint(3500, 8000)
        cost_b = (tokens_b / 1_000_000) * 2.50

        # Arm C fixture: high pass rate to exercise a three-arm comparison.
        pass_c = rng.random() < 0.98
        tokens_c = rng.randint(4000, 9500)
        cost_c = (tokens_c / 1_000_000) * 2.50

        cell = {
            "intent_id": f"synthetic-{i:03d}",
            "seed": 1,
            "model": "synthetic-fixture",
        }
        arm_a_results.append({**cell, "pass": pass_a, "tokens": tokens_a, "cost": cost_a})
        arm_b_results.append({**cell, "pass": pass_b, "tokens": tokens_b, "cost": cost_b})
        arm_c_results.append({**cell, "pass": pass_c, "tokens": tokens_c, "cost": cost_c})

    return {
        "arm_a": arm_a_results,
        "arm_b": arm_b_results,
        "arm_c": arm_c_results,
        "evidence_mode": "synthetic_fixture",
        "inferential": False,
        "source_files": [],
        "interpretation": (
            "Deterministic test double only. These values were generated to "
            "exercise the analysis code and are not experimental evidence."
        ),
    }


def load_benchmark_results(results_dir: Path) -> Dict[str, Any]:
    """Load real benchmark results produced by run_eval.py.

    Reads every ``results.json`` under *results_dir* (one per campaign
    stamp) and maps each run's ``variant`` to an arm:

    - ``naive-prompt`` -> arm_a  (Naive Baseline)
    - ``cli``          -> arm_b  (CLI Agent)
    - ``full``         -> arm_c  (Type-Directed Continuum / Scientist Team)

    Runs whose variant is not recognised are skipped with a warning.
    """
    # variant -> (arm_key, arm_label)
    VARIANT_MAP = {
        "naive-prompt": "arm_a",
        "cli": "arm_b",
        "full": "arm_c",
    }

    results_files = sorted(results_dir.rglob("results.json"))
    if not results_files:
        raise FileNotFoundError(
            f"No results.json files found under {results_dir}. "
            "Run eval/frontier/run_eval.py first or pass --results-dir."
        )

    dataset: Dict[str, Any] = {
        "arm_a": [],
        "arm_b": [],
        "arm_c": [],
        "evidence_mode": "measured_results",
        "inferential": True,
        "source_files": [str(path) for path in results_files],
    }

    for rfile in results_files:
        payload = json.loads(rfile.read_text())
        for run in payload.get("runs", []):
            variant = run.get("variant", "")
            arm = VARIANT_MAP.get(variant)
            if arm is None:
                print(f"  warning: skipping unknown variant '{variant}' in {rfile}")
                continue
            tokens = run.get("input_tokens", 0) + run.get("output_tokens", 0)
            dataset[arm].append({
                "intent_id": run.get("intent_id"),
                "seed": run.get("seed"),
                "model": run.get("model"),
                "pass": bool(run.get("success", False)),
                "tokens": tokens,
                "cost": run.get("est_cost_usd", 0.0),
            })

    return dataset


def _pair_key(row: Dict[str, Any]) -> Tuple[str, str, int]:
    model = row.get("model")
    intent_id = row.get("intent_id")
    seed = row.get("seed")
    if not isinstance(model, str) or not model:
        raise AnalysisError(f"row is missing model identity: {row}")
    if not isinstance(intent_id, str) or not intent_id:
        raise AnalysisError(f"row is missing intent_id: {row}")
    if not isinstance(seed, int):
        raise AnalysisError(f"row is missing integer seed: {row}")
    return model, intent_id, seed


def validate_paired_dataset(dataset: Dict[str, Any]) -> List[Tuple[str, str, int]]:
    """Require identical, unique ``(model, intent, seed)`` cells in every arm."""
    expected: Optional[set[Tuple[str, str, int]]] = None
    for arm in ("arm_a", "arm_b", "arm_c"):
        rows = dataset.get(arm, [])
        if not rows:
            raise AnalysisError(f"{arm} has no benchmark runs")
        keys: set[Tuple[str, str, int]] = set()
        for row in rows:
            key = _pair_key(row)
            if key in keys:
                raise AnalysisError(f"duplicate paired cell in {arm}: {key}")
            keys.add(key)
        if expected is None:
            expected = keys
        elif keys != expected:
            missing = sorted(expected - keys)
            extra = sorted(keys - expected)
            raise AnalysisError(
                f"{arm} is not paired with arm_a; missing={missing}, extra={extra}"
            )
    return sorted(expected or set())


def analyze_arms(dataset: Dict[str, Any], bootstrap_rounds: int = 2000) -> Dict[str, Any]:
    """Performs full statistical evaluation across arms."""
    pair_keys = validate_paired_dataset(dataset)
    arms = ["arm_a", "arm_b", "arm_c"]
    arm_names = {
        "arm_a": "Arm A (Naive Baseline)",
        "arm_b": "Arm B (CLI Agent)",
        "arm_c": "Arm C (Type-Directed Continuum)",
    }

    metrics = {}
    for arm in arms:
        data = dataset[arm]
        passes = [1.0 if x["pass"] else 0.0 for x in data]
        tokens = [float(x["tokens"]) for x in data]
        costs = [float(x["cost"]) for x in data]

        p_mean, p_low, p_high = bootstrap_ci(passes, rounds=bootstrap_rounds)
        t_mean, t_low, t_high = bootstrap_ci(tokens, rounds=bootstrap_rounds)
        c_mean, c_low, c_high = bootstrap_ci(costs, rounds=bootstrap_rounds)

        pass_count = sum(1 for x in data if x["pass"])
        total = len(data)

        metrics[arm] = {
            "name": arm_names[arm],
            "total_samples": total,
            "pass_count": pass_count,
            "pass_rate": round(pass_count / total, 4),
            "pass_at_1": round(pass_at_k(total, pass_count, 1), 4),
            "pass_rate_95_ci": [round(p_low, 4), round(p_high, 4)],
            "mean_tokens": round(t_mean, 1),
            "tokens_95_ci": [round(t_low, 1), round(t_high, 1)],
            "mean_cost_usd": round(c_mean, 5),
            "cost_95_ci": [round(c_low, 5), round(c_high, 5)],
        }

    # Statistical hypothesis comparisons
    by_key = {
        arm: {_pair_key(row): row for row in dataset[arm]}
        for arm in arms
    }
    passes_a = [by_key["arm_a"][key]["pass"] for key in pair_keys]
    passes_b = [by_key["arm_b"][key]["pass"] for key in pair_keys]
    passes_c = [by_key["arm_c"][key]["pass"] for key in pair_keys]

    comparisons = {
        "arm_b_vs_arm_a": mcnemar_test(passes_a, passes_b),
        "arm_c_vs_arm_b": mcnemar_test(passes_b, passes_c),
        "arm_c_vs_arm_a": mcnemar_test(passes_a, passes_c),
    }

    # Holm step-down correction over the fixed 3-contrast family: three
    # correlated McNemar tests at alpha=0.05 inflate the family-wise error
    # rate up to ~3x without it. Raw p-values are kept alongside.
    holm_adjust_p_values(comparisons)

    return {
        "evidence_mode": dataset["evidence_mode"],
        "inferential": bool(dataset["inferential"]),
        "source_files": dataset.get("source_files", []),
        "paired_cells": len(pair_keys),
        "interpretation": dataset.get("interpretation"),
        "metrics": metrics,
        "comparisons": comparisons,
    }


def render_markdown_report(report: Dict[str, Any]) -> str:
    """Render a report whose evidence status is unambiguous."""
    m = report["metrics"]
    c = report["comparisons"]
    synthetic = report["evidence_mode"] == "synthetic_fixture"

    md = [
        (
            "# Synthetic Statistical Analysis Fixture (Non-Inferential)"
            if synthetic
            else "# Paired Statistical Benchmark Report"
        ),
        "",
    ]
    if synthetic:
        md.extend([
            "> **NOT EXPERIMENTAL EVIDENCE.** This report was generated from a",
            "> deterministic synthetic fixture to test the analysis code. It must",
            "> not be cited as a benchmark result or used to claim superiority.",
            "",
        ])
    else:
        md.extend([
            f"> Paired cells: {report['paired_cells']}. Source files: "
            f"{len(report['source_files'])}.",
            "",
        ])

    md.extend([
        "## 1. Arms & Pass@1",
        "",
        "| Arm | Description | Total Samples | Pass@1 | 95% Bootstrap CI | Mean Tokens | Mean Cost ($) |",
        "| :--- | :--- | :---: | :---: | :---: | :---: | :---: |",
    ])

    for arm_id in ["arm_a", "arm_b", "arm_c"]:
        info = m[arm_id]
        ci = f"[{info['pass_rate_95_ci'][0]:.3f}, {info['pass_rate_95_ci'][1]:.3f}]"
        t_ci = f"{info['mean_tokens']:.0f}"
        cost_str = f"${info['mean_cost_usd']:.4f}"
        md.append(
            f"| **{arm_id.upper()}** | {info['name']} | {info['total_samples']} | "
            f"**{info['pass_at_1']*100:.1f}%** | {ci} | {t_ci} | {cost_str} |"
        )

    md.extend([
        "",
        "## 2. Paired Hypothesis Testing",
        "",
        "McNemar's test is applied only after exact cell matching across all arms.",
        "The significance column judges the Holm step-down adjusted p-value over the",
        "3-contrast family (raw p-values shown alongside) so the family-wise error",
        "rate stays at alpha=0.05.",
        "",
        "| Contrast | Discordant Pairs (b/c) | $\\chi^2$ (df=1) | Two-Tailed $p$-value | Holm $p$-value | Odds Ratio | Significant ($\\alpha=0.05$) |",
        "| :--- | :---: | :---: | :---: | :---: | :---: | :---: |",
    ])

    for pair, res in c.items():
        label = pair.replace("_", " ").upper()
        p_str = f"{res['p_value']:.4e}" if res["p_value"] < 0.001 else f"{res['p_value']:.4f}"
        holm_str = (
            f"{res['p_value_holm']:.4e}" if res["p_value_holm"] < 0.001 else f"{res['p_value_holm']:.4f}"
        )
        sig_str = "**YES** (holm p < 0.05)" if res["significant"] else "NO"
        md.append(
            f"| **{label}** | {res['b']} / {res['c']} | {res['chi2']} | {p_str} | {holm_str} | {res['odds_ratio']} | {sig_str} |"
        )

    if synthetic:
        md.extend([
            "",
            "## 3. Interpretation",
            "",
            "- Fixture-only output; no scientific conclusion is supported.",
        ])
    else:
        md.extend([
            "",
            "## 3. Interpretation",
            "",
            "- Results are descriptive for the matched cells listed in the source files.",
            "- Significance does not establish generalization beyond the sampled intents, models, seeds, and execution environment.",
            "- Repair capability must be reported separately from static detection.",
        ])

    return "\n".join(md)


def main() -> None:
    parser = argparse.ArgumentParser(description="Statistical evaluation & hypothesis testing harness")
    parser.add_argument("--results-dir", default=None, help="Directory containing eval results")
    parser.add_argument("--out-json", default=None, help="Path to export JSON statistical report")
    parser.add_argument("--out-md", default=None, help="Path to export Markdown publication report")
    parser.add_argument("--bootstrap-rounds", type=int, default=2000, help="Number of bootstrap resamples")
    parser.add_argument(
        "--synthetic-fixture",
        action="store_true",
        help="Use deterministic fake data to test this script; never inferential",
    )
    args = parser.parse_args()

    if bool(args.results_dir) == args.synthetic_fixture:
        parser.error("choose exactly one of --results-dir or --synthetic-fixture")

    try:
        if args.results_dir:
            dataset = load_benchmark_results(Path(args.results_dir))
            print(f"Loaded measured benchmark results from: {args.results_dir}")
        else:
            dataset = generate_synthetic_benchmark_dataset(n_intents=36)
            print("WARNING: synthetic fixture mode is non-inferential")
        report = analyze_arms(dataset, bootstrap_rounds=args.bootstrap_rounds)
    except (AnalysisError, FileNotFoundError) as exc:
        print(f"error: {exc}", file=sys.stderr)
        raise SystemExit(2) from exc

    print("=" * 80)
    print(
        "oxo-flow Synthetic Fixture (NON-INFERENTIAL)"
        if report["evidence_mode"] == "synthetic_fixture"
        else "oxo-flow Paired Statistical Benchmark Report"
    )
    print("=" * 80)
    for arm_id, info in report["metrics"].items():
        ci = info["pass_rate_95_ci"]
        print(
            f"{info['name']:<35} | Pass@1: {info['pass_at_1']*100:5.1f}% "
            f"(95% CI: [{ci[0]*100:.1f}%, {ci[1]*100:.1f}%]) "
            f"| Tokens: {info['mean_tokens']:6.0f} | Cost: ${info['mean_cost_usd']:.4f}"
        )

    print("-" * 80)
    print("Paired McNemar Hypothesis Tests:")
    for pair, res in report["comparisons"].items():
        sig = "SIGNIFICANT" if res["significant"] else "NOT SIGNIFICANT"
        p_val = res["p_value"]
        print(f"  {pair:<18}: chi2={res['chi2']:<6.2f} p={p_val:<10.4e} OR={res['odds_ratio']} ({sig})")
    print("=" * 80)

    if args.out_json:
        out_j = Path(args.out_json)
        out_j.parent.mkdir(parents=True, exist_ok=True)
        out_j.write_text(json.dumps(report, indent=2))
        print(f"Statistical JSON exported to: {out_j}")

    if args.out_md:
        out_m = Path(args.out_md)
        out_m.parent.mkdir(parents=True, exist_ok=True)
        out_m.write_text(render_markdown_report(report))
        print(f"Publication Markdown exported to: {out_m}")


if __name__ == "__main__":
    main()
