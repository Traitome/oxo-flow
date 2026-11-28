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
The continuity-corrected chi-square with Holm correction is the frozen
instrument's PRIMARY inference. The exact two-sided binomial test is
reported as a SENSITIVITY because the discordant count can be small; it
does not replace the primary. The significance column judges the Holm
step-down adjusted primary value over the 3-contrast family so the
family-wise error rate stays at alpha=0.05; raw values are shown alongside.

| Contrast | Discordant (b/c) | $\chi^2$ (df=1) | $\chi^2$ $p$ | Holm $\chi^2$ $p$ (primary) | Exact $p$ (sensitivity) | Exact Holm $p$ (sensitivity) | Significant (primary Holm, $\alpha=0.05$) |
| :--- | :---: | :---: | :---: | :---: | :---: | :---: | :---: |
| **ARM B VS ARM A** | 17 / 2 | 10.3158 | 0.0013 | 0.0026 | 7.2861e-04 | 0.0015 | **YES** (primary Holm p < 0.05) |
| **ARM C VS ARM B** | 6 / 0 | 4.1667 | 0.0412 | 0.0412 | 0.0312 | 0.0312 | **YES** (primary Holm p < 0.05) |
| **ARM C VS ARM A** | 21 / 0 | 19.0476 | 1.2760e-05 | 3.8000e-05 | 9.5367e-07 | 3.0000e-06 | **YES** (primary Holm p < 0.05) |

## 3. Intent-level sensitivity (seed-clustered)

Descriptive sensitivity only: each intent is collapsed to one binary
outcome across its seeds and the exact test is repeated with N = number
of intents. `all` = passed every seed, `any` = passed at least one,
`majority` = passed more than half. The cell-level primary test above
(continuity-corrected chi-square + Holm) remains the frozen primary;
the exact test is the sensitivity.

| Rule | A vs B (b/c, exact p) | B vs C (b/c, exact p) | A vs C (b/c, exact p) |
| :--- | :---: | :---: | :---: |
| all | 17/2, 7.2861e-04 | 6/0, 0.0312 | 21/0, 9.5367e-07 |
| any | 17/2, 7.2861e-04 | 6/0, 0.0312 | 21/0, 9.5367e-07 |
| majority | 17/2, 7.2861e-04 | 6/0, 0.0312 | 21/0, 9.5367e-07 |

## 4. Interpretation

- Fixture-only output; no scientific conclusion is supported.