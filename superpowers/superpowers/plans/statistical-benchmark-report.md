# Statistical Benchmark Evidence Status

> **Status**: no confirmatory statistical result.
>
> **Evidence mode**: tooling validation only.
>
> **Important**: this document must not be cited as evidence of superiority over Nextflow, Snakemake, or another `oxo-flow` configuration.

## 1. Purpose

The statistical harness is designed to analyze matched Arms A, B, and C after a pre-registered benchmark campaign:

- **Arm A**: unconstrained generation.
- **Arm B**: syntax/schema feedback only.
- **Arm C**: full declared-contract and scientific-preflight feedback.

The harness fails closed when matched three-arm measurements are absent.

## 2. What Is Implemented

- Paired cell matching by intent, model, and seed.
- `pass@k` computation when the required repeated samples exist.
- Bootstrap confidence intervals.
- McNemar analysis for paired binary outcomes.
- Cost and token summaries.
- Explicit non-inferential mode for deterministic synthetic fixtures.

These are analysis capabilities, not experimental findings.

## 3. Direct Systems Comparison Required

An AI-synthesis benchmark is not a replacement benchmark. Any claim that
`oxo-flow` can substitute for Nextflow or Snakemake also requires a direct,
pre-registered comparison using the same workflow intent, inputs, reference
data, container digests, host, resource limits, scheduler, and cache state.
The comparison must retain per-run engine versions, commands, wall time, peak
memory, disk I/O, completion, declared-output equivalence, restart behavior,
failures, and exclusions. Unsupported constructs are coverage limits and may
not be silently dropped.

## 4. Evidence Required Before Reporting Results

The confirmatory campaign must attach:

1. A pre-registration of primary outcomes, exclusions, stopping rules, and analysis.
2. The exact held-out intent corpus and a leakage audit.
3. Fixed model names, provider versions, prompts, tool policies, token budgets, and seeds.
4. Matched per-item outputs for every arm.
5. Raw success/failure judgments with human adjudication where automation is insufficient.
6. Token and monetary cost inputs with provider pricing provenance.
7. An independent reanalysis from the raw data.
8. Negative results and unsupported cells, without post-hoc deletion.

## 5. Prohibited Claims

Do not report any of the following without measured matched data:

- a pass-rate superiority claim;
- a statistically significant improvement;
- a self-healing success rate;
- a cost advantage;
- a claim that biological errors are eliminated;
- a claim that a synthetic fixture predicts production or publication performance.

## 6. Current Decision

The confirmatory benchmark remains **Planned / blocked**. The next admissible output is a pre-registration and data manifest, not a results table.
