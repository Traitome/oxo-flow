//! Benchmark: Wildcard expansion at scale.
//!
//! Goal: <500ms for 10,000 combinations.

use std::collections::HashMap;

use criterion::{BenchmarkId, Criterion, black_box, criterion_group, criterion_main};
use oxo_flow_core::wildcard;

fn bench_wildcard_large(c: &mut Criterion) {
    let mut group = c.benchmark_group("wildcard_large");

    // Small: 10 samples
    let mut vars_small: HashMap<String, Vec<String>> = HashMap::new();
    vars_small.insert(
        "sample".into(),
        (1..=10).map(|i| format!("sample{i:02}")).collect(),
    );

    group.bench_function(BenchmarkId::new("expand", "10_combos"), |b| {
        b.iter(|| {
            let result = wildcard::cartesian_expand("{sample}", &vars_small);
            black_box(result)
        })
    });

    // Large: 100 × 2 = 200 expansions
    let mut vars_large: HashMap<String, Vec<String>> = HashMap::new();
    vars_large.insert(
        "sample".into(),
        (1..=100).map(|i| format!("sample{i:03}")).collect(),
    );
    vars_large.insert("lane".into(), vec!["L001".into(), "L002".into()]);

    group.bench_function(BenchmarkId::new("expand", "200_combos"), |b| {
        b.iter(|| {
            let result = wildcard::cartesian_expand("{sample}_{lane}", &vars_large);
            black_box(result)
        })
    });

    group.finish();
}

criterion_group!(benches, bench_wildcard_large);
criterion_main!(benches);
