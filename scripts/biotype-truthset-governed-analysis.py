#!/usr/bin/env python3
"""Sealed analysis for the #1163 governed BioType truth set (issue #1396).

Computes the primary endpoint `FA / (FA + TR)` over expected-reject cases,
its exact two-sided 95% Clopper-Pearson interval, and the frozen PR-06
§C1 decision mapping:

  INVALID first  -- N_primary < N_min (sub-reason minimum_sample_not_reached,
                    reported as UNDERPOWERED) or any non-verdict
                    (sub-reason non_verdict_limit).
  POSITIVE       -- exact 95% upper bound U <= tau_FA.
  NEGATIVE       -- exact 95% lower bound L > tau_FA.
  INCONCLUSIVE   -- L <= tau_FA < U.

The interval is computed from the binomial CDF by deterministic bisection
(stdlib only; no scipy). This script is sealed (SHA-256) in the pre-run
manifest before the confirmatory run, per PR-01 §0 rule 9.

The frozen constants `TAU_FA` and `N_MIN` below are the only thresholds
this script can use: the optional `--tau`/`--n-min` overrides of the
original sealed version were removed after independent review (amendment
4), so a sealed-mode invocation cannot recompute the decision with
non-frozen constants.

Usage:
  python3 scripts/biotype-truthset-governed-analysis.py \
      docs/superpowers/audits/data/biotype-truthset-governed-results.json
"""

import argparse
import hashlib
import json
import math
import sys
from pathlib import Path

TAU_FA = 0.05      # PR-06 §C1, frozen 2026-11-27 (amendment 2)
N_MIN = 73         # PR-06 §C1, frozen 2026-11-27 (amendment 2)
ALPHA = 0.05       # two-sided 95% interval
BISECTION_ITERS = 200
EPS = 1e-12


def binomial_cdf(k, n, p):
    """P(X <= k) for X ~ Binomial(n, p), exact integer coefficients."""
    if k < 0:
        return 0.0
    if k >= n:
        return 1.0
    if p <= 0.0:
        return 1.0
    if p >= 1.0:
        return 0.0
    q = 1.0 - p
    total = 0.0
    for i in range(0, k + 1):
        total += math.comb(n, i) * (p ** i) * (q ** (n - i))
    return total


def clopper_pearson(x, n, alpha=ALPHA):
    """Exact two-sided Clopper-Pearson interval (x successes in n trials)."""
    if n == 0:
        return 0.0, 1.0

    # Lower: solve P(X >= x; n, p) = alpha/2, i.e.
    # 1 - P(X <= x-1; n, p) = alpha/2. Monotone increasing in p.
    if x <= 0:
        lower = 0.0
    else:
        lo, hi = 0.0, 1.0
        for _ in range(BISECTION_ITERS):
            mid = (lo + hi) / 2.0
            f = (1.0 - binomial_cdf(x - 1, n, mid)) - alpha / 2.0
            if f < 0.0:
                lo = mid
            else:
                hi = mid
            if hi - lo < EPS:
                break
        lower = (lo + hi) / 2.0

    # Upper: solve P(X <= x; n, p) = alpha/2. Monotone decreasing in p.
    if x >= n:
        upper = 1.0
    else:
        lo, hi = 0.0, 1.0
        for _ in range(BISECTION_ITERS):
            mid = (lo + hi) / 2.0
            g = binomial_cdf(x, n, mid) - alpha / 2.0
            if g > 0.0:
                lo = mid
            else:
                hi = mid
            if hi - lo < EPS:
                break
        upper = (lo + hi) / 2.0

    return lower, upper


def sha256_file(path):
    h = hashlib.sha256()
    with open(path, "rb") as f:
        for chunk in iter(lambda: f.read(1 << 20), b""):
            h.update(chunk)
    return h.hexdigest()


def fmt_pct(x):
    return f"{100.0 * x:.4f}%"


def decide(fa, n_primary, non_verdicts, tau=TAU_FA, n_min=N_MIN):
    lower, upper = clopper_pearson(fa, n_primary)
    if n_primary < n_min:
        category = "INVALID"
        sub_reason = "minimum_sample_not_reached"
        report_label = "UNDERPOWERED"
    elif non_verdicts > 0:
        category = "INVALID"
        sub_reason = "non_verdict_limit"
        report_label = "INVALID"
    elif upper <= tau:
        category = "POSITIVE"
        sub_reason = None
        report_label = "POSITIVE"
    elif lower > tau:
        category = "NEGATIVE"
        sub_reason = None
        report_label = "NEGATIVE"
    else:
        category = "INCONCLUSIVE"
        sub_reason = None
        report_label = "INCONCLUSIVE"
    return {
        "category": category,
        "report_label": report_label,
        "invalid_sub_reason": sub_reason,
        "tau_fa": tau,
        "n_min": n_min,
        "primary_denominator": n_primary,
        "false_accepts": fa,
        "point_estimate": (fa / n_primary) if n_primary else None,
        "ci_lower": lower,
        "ci_upper": upper,
        "ci_level": 1.0 - ALPHA,
        "ci_method": "Clopper-Pearson exact (two-sided)",
        "non_verdicts": non_verdicts,
        "decision_rule": (
            "INVALID first (N<N_min -> UNDERPOWERED, or any non-verdict); "
            "POSITIVE iff U<=tau; NEGATIVE iff L>tau; else INCONCLUSIVE"
        ),
    }


def statement(decision):
    p = decision["point_estimate"]
    lo = decision["ci_lower"]
    hi = decision["ci_upper"]
    n = decision["primary_denominator"]
    fa = decision["false_accepts"]
    tr = n - fa
    cat = decision["category"]
    if cat == "POSITIVE":
        return (
            f"On the frozen held-out truth set of {n} expected-reject cases, "
            f"the BioTypes checker false-accepted {fa} of {fa + tr} "
            f"expected-reject cases ({fmt_pct(p)}, exact 95% CI "
            f"[{fmt_pct(lo)}, {fmt_pct(hi)}]), within the pre-registered "
            f"bound tau_FA = {decision['tau_fa']}. This validates the checker "
            f"on this set; it does not establish biological truth or coverage "
            f"outside the listed dimensions.")
    if cat == "NEGATIVE":
        return (
            f"On the frozen held-out truth set of {n} expected-reject cases, "
            f"the BioTypes checker false-accepted {fa} of {fa + tr} "
            f"expected-reject cases ({fmt_pct(p)}, exact 95% CI "
            f"[{fmt_pct(lo)}, {fmt_pct(hi)}]), which does not meet the "
            f"pre-registered threshold tau_FA = {decision['tau_fa']}. The "
            f"checker is therefore not validated on this truth set; no "
            f"accuracy claim beyond the reported cells is supported.")
    if cat == "INCONCLUSIVE":
        return (
            f"On the frozen held-out truth set of {n} expected-reject cases, "
            f"the BioTypes checker false-accepted {fa} of {fa + tr} "
            f"expected-reject cases ({fmt_pct(p)}, exact 95% CI "
            f"[{fmt_pct(lo)}, {fmt_pct(hi)}]). The interval is neither "
            f"entirely at or below tau_FA = {decision['tau_fa']} nor entirely "
            f"above it, so no capability claim is supported.")
    return (
        f"The campaign is INVALID ({decision['invalid_sub_reason']}): the "
        f"pre-declared validity conditions were not met, so no endpoint "
        f"estimate is presented as the campaign result.")


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("results")
    ap.add_argument("--out", default=None)
    args = ap.parse_args()

    results_path = Path(args.results)
    data = json.loads(results_path.read_text())
    primary = data["primary"]
    fa = int(primary["false_accepts"])
    n_primary = int(primary["denominator"])
    non_verdicts = int(data.get("non_verdicts", 0))

    decision = decide(fa, n_primary, non_verdicts)
    worst_case = decide(fa + non_verdicts, n_primary + non_verdicts,
                        non_verdicts) if non_verdicts else None
    out = {
        "format": "oxo-flow.biotype-truthset-governed-decision/v1",
        "issue": 1396,
        "campaign": data.get("campaign"),
        "results_file": str(results_path),
        "results_sha256": sha256_file(results_path),
        "results_commit": data.get("commit"),
        "binary_sha256": data.get("binary_sha256"),
        "confusion_matrix": data.get("confusion_matrix"),
        "primary": primary,
        "decision": decision,
        "worst_case_sensitivity": worst_case,
        "statement": statement(decision),
    }
    text = json.dumps(out, indent=2, sort_keys=False) + "\n"
    if args.out:
        Path(args.out).write_text(text)
    print(text)
    return 0


if __name__ == "__main__":
    sys.exit(main())
