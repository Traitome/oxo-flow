# Benchmark Suite

This directory contains the oxo-flow benchmark suite, engineered for
performance regression tracking and scientific publication.

## Structure

| File | Benchmarks | Scope |
|---|---|---|
| `dag_bench.rs` | 12 | DAG construction, validation, analysis (chain/tree/star, 10–10k rules) |
| `wildcard_bench.rs` | 9 | Pattern expansion, Cartesian product, regex conversion |
| `parsing_bench.rs` | 9 | TOML parse, full lifecycle, serialisation round-trip |
| `scheduling_bench.rs` | 5 | Ready-rule queries, strategy comparison, simulation |

**Total: 35 benchmark functions** across 4 binary targets.

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
