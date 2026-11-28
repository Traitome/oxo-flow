# Paired Statistical Benchmark Report

> Paired cells: 108. Source files: 1.

## 1. Arms & Pass@1

| Arm | Description | Total Samples | Pass@1 | 95% Bootstrap CI | Mean Tokens | Mean Cost ($) |
| :--- | :--- | :---: | :---: | :---: | :---: | :---: |
| **ARM_A** | Arm A (Unconstrained Baseline) | 108 | **0.0%** | [0.000, 0.000] | 3507 | $0.0037 |
| **ARM_B** | Arm B (Syntax + Structural Feedback Only) | 108 | **85.2%** | [0.787, 0.917] | 69170 | $0.0293 |
| **ARM_C** | Arm C (BioTypes + Scientific Preflight + Deep-Check) | 108 | **45.4%** | [0.361, 0.546] | 60127 | $0.0258 |

## 2. Paired Hypothesis Testing

McNemar's test is applied only after exact cell matching across all arms.
The exact two-sided test is the pre-registered primary (#1395); with few
discordant pairs the asymptotic chi-square is not trustworthy and is kept
only for continuity. The significance column judges the Holm step-down
adjusted EXACT p-value over the 3-contrast family (raw values shown
alongside) so the family-wise error rate stays at alpha=0.05.

| Contrast | Discordant (b/c) | Exact two-tailed $p$ | Holm exact $p$ | $\chi^2$ (df=1) | $\chi^2$ $p$ | Holm $\chi^2$ $p$ | Significant (exact Holm, $\alpha=0.05$) |
| :--- | :---: | :---: | :---: | :---: | :---: | :---: | :---: |
| **ARM B VS ARM A** | 92 / 0 | 4.0390e-28 | 0.0000e+00 | 90.0109 | 2.4091e-21 | 0.0000e+00 | **YES** (exact Holm p < 0.05) |
| **ARM C VS ARM B** | 2 / 45 | 1.6044e-11 | 0.0000e+00 | 37.5319 | 9.0280e-10 | 0.0000e+00 | **YES** (exact Holm p < 0.05) |
| **ARM C VS ARM A** | 49 / 0 | 3.5527e-15 | 0.0000e+00 | 47.0204 | 7.0675e-12 | 0.0000e+00 | **YES** (exact Holm p < 0.05) |

## 3. Intent-level sensitivity (seed-clustered)

Descriptive sensitivity only: each intent is collapsed to one binary
outcome across its seeds and the exact test is repeated with N = number
of intents. `all` = passed every seed, `any` = passed at least one,
`majority` = passed more than half. The cell-level exact test above
remains the pre-registered primary.

| Rule | A vs B (b/c, exact p) | B vs C (b/c, exact p) | A vs C (b/c, exact p) |
| :--- | :---: | :---: | :---: |
| all | 26/0, 2.9802e-08 | 2/12, 0.0129 | 16/0, 3.0518e-05 |
| any | 34/0, 1.1642e-10 | 0/17, 1.5259e-05 | 17/0, 1.5259e-05 |
| majority | 32/0, 4.6566e-10 | 0/16, 3.0518e-05 | 16/0, 3.0518e-05 |

## 4. Interpretation

- Results are descriptive for the matched cells listed in the source files.
- Significance does not establish generalization beyond the sampled intents, models, seeds, and execution environment.
- Repair capability must be reported separately from static detection.