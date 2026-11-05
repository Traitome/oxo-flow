# Cohort Expansion Pilot Status

> **Status**: single local CLI smoke test, not a scaling benchmark.
>
> **Evidence source**: `oxo-flow dry-run --json` through `eval/benchmarks/cohort_driver.py`.
>
> **Claim boundary**: the result demonstrates that the harness and real plan parser agree for one small cohort. It does not establish large-cohort scalability, production throughput, or cross-platform performance.

## 1. Measured Pilot

| Cohort size ($N$) | Concrete plan nodes | Process wall time (ms) | Nodes per wall-clock second |
| :---: | :---: | :---: | :---: |
| 10 | 30 | 118.69 | 252.8 |

Interpretation:

- Plan nodes are the length of the emitted `dry-run --json` plan.
- The node count is measured, not inferred from cohort size.
- The wall time includes process startup, parsing, validation, expansion, DAG construction, and JSON serialization.
- A single run has no confidence interval and is not suitable for a performance claim.

## 2. Required Scaling Study

Before reporting scaling behavior:

1. Build a release binary and record its source commit and checksum.
2. Run the full pre-declared cohort sweep with repeated trials and fixed seeds.
3. Separate process startup from internal expansion timing.
4. Record host, operating system, memory, filesystem, and CPU topology.
5. Preserve raw JSON for every run.
6. Report medians or means with confidence intervals and failed runs.
7. Test the same workflow under controlled host load.
8. Validate that the plan's dependency structure and rule semantics remain correct, not only that node counts increase.
9. Re-run representative plans rather than treating dry-run expansion as execution evidence.

## 3. Current Decision

Cohort scaling remains **Simulation or smoke test only**. Large-cohort and production claims require a separate measured campaign.
