# Synthetic Statistical Analysis Fixture (Non-Inferential)

> **NOT EXPERIMENTAL EVIDENCE.** This report was generated from a
> deterministic synthetic fixture to test the analysis code. It must
> not be cited as a benchmark result or used to claim superiority.

## 1. Arms & Pass@1

| Arm | Description | Total Samples | Pass@1 | 95% Bootstrap CI | Mean Tokens | Mean Cost ($) |
| :--- | :--- | :---: | :---: | :---: | :---: | :---: |
| **ARM_A** | Arm A (Unconstrained Baseline) | 36 | **41.7%** | [0.278, 0.583] | 1220 | $0.0031 |
| **ARM_B** | Arm B (Syntax + Structural Feedback Only) | 36 | **83.3%** | [0.694, 0.944] | 5553 | $0.0139 |
| **ARM_C** | Arm C (BioTypes + Scientific Preflight + Deep-Check) | 36 | **100.0%** | [1.000, 1.000] | 6870 | $0.0172 |

## 2. Paired Hypothesis Testing

McNemar's test is applied only after exact cell matching across all arms.
The exact two-sided test is the pre-registered primary (#1395); with few
discordant pairs the asymptotic chi-square is not trustworthy and is kept
only for continuity. The significance column judges the Holm step-down
adjusted EXACT p-value over the 3-contrast family (raw values shown
alongside) so the family-wise error rate stays at alpha=0.05.

| Contrast | Discordant (b/c) | Exact two-tailed $p$ | Holm exact $p$ | $\chi^2$ (df=1) | $\chi^2$ $p$ | Holm $\chi^2$ $p$ | Significant (exact Holm, $\alpha=0.05$) |
| :--- | :---: | :---: | :---: | :---: | :---: | :---: | :---: |
| **ARM B VS ARM A** | 17 / 2 | 7.2861e-04 | 0.0015 | 10.3158 | 0.0013 | 0.0026 | **YES** (exact Holm p < 0.05) |
| **ARM C VS ARM B** | 6 / 0 | 0.0312 | 0.0312 | 4.1667 | 0.0412 | 0.0412 | **YES** (exact Holm p < 0.05) |
| **ARM C VS ARM A** | 21 / 0 | 9.5367e-07 | 3.0000e-06 | 19.0476 | 1.2760e-05 | 3.8000e-05 | **YES** (exact Holm p < 0.05) |

## 3. Intent-level sensitivity (seed-clustered)

Descriptive sensitivity only: each intent is collapsed to one binary
outcome across its seeds and the exact test is repeated with N = number
of intents. `all` = passed every seed, `any` = passed at least one,
`majority` = passed more than half. The cell-level exact test above
remains the pre-registered primary.

| Rule | A vs B (b/c, exact p) | B vs C (b/c, exact p) | A vs C (b/c, exact p) |
| :--- | :---: | :---: | :---: |
| all | 17/2, 7.2861e-04 | 6/0, 0.0312 | 21/0, 9.5367e-07 |
| any | 17/2, 7.2861e-04 | 6/0, 0.0312 | 21/0, 9.5367e-07 |
| majority | 17/2, 7.2861e-04 | 6/0, 0.0312 | 21/0, 9.5367e-07 |

## 4. Interpretation

- Fixture-only output; no scientific conclusion is supported.