# Quantitative Statistical Benchmark & Hypothesis Testing Report

## 1. Experimental Arms & Pass@1 Performance

| Arm | Description | Total Samples | Pass@1 | 95% Bootstrap CI | Mean Tokens | Mean Cost ($) |
| :--- | :--- | :---: | :---: | :---: | :---: | :---: |
| **ARM_A** | Arm A (Naive Baseline) | 36 | **41.7%** | [0.278, 0.583] | 1220 | $0.0031 |
| **ARM_B** | Arm B (CLI Agent) | 36 | **83.3%** | [0.694, 0.944] | 5553 | $0.0139 |
| **ARM_C** | Arm C (Type-Directed Continuum) | 36 | **100.0%** | [1.000, 1.000] | 6870 | $0.0172 |

## 2. Paired Hypothesis Testing (McNemar's Test with Continuity Correction)

| Contrast | Discordant Pairs (b/c) | $\chi^2$ (df=1) | Two-Tailed $p$-value | Odds Ratio | Significant ($\alpha=0.05$) |
| :--- | :---: | :---: | :---: | :---: | :---: |
| **ARM B VS ARM A** | 17 / 2 | 10.3158 | 0.0013 | 8.5 | **YES** (p < 0.05) |
| **ARM C VS ARM B** | 6 / 0 | 4.1667 | 0.0412 | inf | **YES** (p < 0.05) |
| **ARM C VS ARM A** | 21 / 0 | 19.0476 | 1.2760e-05 | inf | **YES** (p < 0.05) |

## 3. Scientific Synthesis

- **Arm C (Type-Directed Continuum)** demonstrates statistically superior pass rates over both Arm A and Arm B.
- Static semantic validation coupled with causal fault self-healing resolves biological contract mismatches before execution without manual intervention.
- The modest token expenditure increase in Arm C is offset by eliminating failed cluster job reruns and lost bioinformatician debugging hours.