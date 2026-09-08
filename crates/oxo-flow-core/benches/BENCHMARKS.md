# Benchmark Suite

This directory contains the oxo-flow benchmark suite, engineered for
performance regression tracking and scientific publication.

## Structure

Every file below is declared as a `[[bench]] harness = false` target in
`Cargo.toml`; criterion supplies `main`, libtest does not wrap it. A file
missing from that list still compiles but reports "0 tests" instead of
running — keep this table in sync when benches are added or removed.

| File | Criterion functions | Measurements | Scope |
|---|---|---|---|
| `dag_bench.rs` | 11 | 20 | DAG construction, validation, analysis (chain/tree/star, 10–10k rules) |
| `wildcard_bench.rs` | 9 | 9 | Pattern expansion, Cartesian product, regex conversion |
| `parsing_bench.rs` | 9 | 9 | TOML parse, full lifecycle, serialisation round-trip |
| `scheduling_bench.rs` | 5 | 7 | Ready-rule queries, strategy comparison, simulation |
| `dag_scale_bench.rs` | 1 | 12 | Build/order/groups across 10–500 rule chains (3 sites × 4 sizes) |
| `api_response_bench.rs` | 1 | 5 | API status JSON serialisation/parse at 10/200 nodes |
| `diagnostics_bench.rs` | 1 | 2 | Log-pattern matching at 3/1000 logs |
| `wildcard_large_bench.rs` | 1 | 2 | Large Cartesian expansion (10/200 combos) |

**Total: 38 criterion functions / 66 measurements** across 8 binary targets.
(Criterion functions are the entry points registered in `criterion_group!`;
measurements count each `bench_function` site after loop expansion.)

## Design

### Topologies

| Topology | Rules | Edges | What it stresses |
|---|---|---|---|
| Chain | N | N–1 | Sequential dependency resolution |
| Tree (balanced) | 2^L–1 | 2^L–2 | Branching/merging |
| Star | N+1 | N | Fan-out/fan-in bottlenecks |

### Metrics

All benchmarks report average wall-clock time per iteration via
[criterion.rs](https://github.com/bheisler/criterion.rs).  Where relevant,
throughput (rules/second) is derived by dividing iteration count by time.

### Strategy

| Scheduler strategy | Method | Priority |
|---|---|---|
| Default | `ready_rules` | None (arbitrary order) |
| Prioritised | `ready_rules_prioritized` | Explicit rule priority |
| Critical path | `ready_rules_critical_path` | Path + priority + name |

## Usage

```bash
# Run all benchmarks
cargo bench -p oxo-flow-core

# Run a specific group
cargo bench -p oxo-flow-core -- dag/

# Save baseline for regression comparison
cargo bench -p oxo-flow-core -- --save-baseline baseline
cargo bench -p oxo-flow-core -- --baseline baseline
```

## CI Integration

The `make bench` target saves a baseline and reports any regressions.
Results are cached under `target/criterion/`.  Baseline files should be
committed to the repository when a release is cut.

## Reproducibility

- Pin the Rust toolchain version in `rust-toolchain.toml`
- Use `--profile=time` for maximum measurement accuracy
- Isolate benchmarks on dedicated hardware for publication-quality numbers
- For cross-engine comparison, see `docs/guide/src/reference/benchmarking.md`
