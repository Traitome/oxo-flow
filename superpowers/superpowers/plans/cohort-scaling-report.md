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

## 2. Provenance of the §1 row (UNPINNED)

The single `N=10` row in §1 is **UNPINNED** and fails §3 steps 1 and 4 below:
no source commit, no binary checksum, no host/OS/CPU record, and no tool
versions were captured when it was produced. It is therefore **not
reproducible** and must not be cited as a control number.

A pinned local reproduction was taken on a release build to show the harness is
reproducible in principle (all values below were captured on 2026-11-21):

| Field (per §3 step 1 and step 4) | Value |
| :--- | :--- |
| Source commit | `e855fa6a71631237d5973041caf95664af23590d` |
| Binary path | `target/release/oxo-flow` |
| Binary SHA-256 | `81bf9bab8fb253b35f1f04575650c7ea573db878a9fd04957378c7788748809f` |
| Host / OS | Apple M3 / macOS 26.6.2 (25G83) / Darwin 25.6.0 arm64 |
| CPU / memory | 8 logical cores / 24 GiB RAM |
| rustc / cargo | 1.98.0 (88d9e12ae 2026-08-18) / 1.98.0 |
| python3 | 3.12.4 |
| Command | `OXO_FLOW_BIN=target/release/oxo-flow python3 eval/benchmarks/cohort_driver.py --samples 10` |

Pinned reproduction, six consecutive runs, same binary:

| Cohort size ($N$) | Concrete plan nodes | Process wall time (ms) | Nodes per wall-clock second |
| :---: | :---: | :---: | :---: |
| 10 | 30 | 9.63 | 3116.8 |
| 10 | 30 | 9.76 | 3074.7 |
| 10 | 30 | 9.79 | 3064.6 |
| 10 | 30 | 9.55 | 3142.3 |
| 10 | 30 | 9.50 | 3159.5 |
| 10 | 30 | 9.75 | 3076.0 |

The pinned release build runs roughly 12x faster than the §1 row (~9.5-9.8 ms
vs 118.69 ms). The §1 number therefore came from a different and unrecorded
build (most plausibly a debug profile or a loaded host) and cannot be treated
as a baseline. Only the pinned block above is attributable to the recorded
commit and checksum.

## 3. Required Scaling Study

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

Steps 1 and 4 are now satisfied for the pinned reproduction in §2. Steps 2,
3, and 5-9 remain open: this is still one small cohort, not a sweep.

## 4. Current Decision

Cohort scaling remains **Simulation or smoke test only**. Large-cohort and production claims require a separate measured campaign.
