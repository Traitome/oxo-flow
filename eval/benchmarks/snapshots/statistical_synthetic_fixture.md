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
The significance column judges the Holm step-down adjusted p-value over the
3-contrast family (raw p-values shown alongside) so the family-wise error
rate stays at alpha=0.05.

| Contrast | Discordant Pairs (b/c) | $\chi^2$ (df=1) | Two-Tailed $p$-value | Holm $p$-value | Odds Ratio | Significant ($\alpha=0.05$) |
| :--- | :---: | :---: | :---: | :---: | :---: | :---: |
| **ARM B VS ARM A** | 17 / 2 | 10.3158 | 0.0013 | 0.0026 | 8.5 | **YES** (holm p < 0.05) |
| **ARM C VS ARM B** | 6 / 0 | 4.1667 | 0.0412 | 0.0412 | inf | **YES** (holm p < 0.05) |
| **ARM C VS ARM A** | 21 / 0 | 19.0476 | 1.2760e-05 | 3.8000e-05 | inf | **YES** (holm p < 0.05) |

## 3. Interpretation

- Fixture-only output; no scientific conclusion is supported.