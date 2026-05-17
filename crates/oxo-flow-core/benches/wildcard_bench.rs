use criterion::{Criterion, black_box, criterion_group, criterion_main};
use oxo_flow_core::wildcard::{WildcardValues, cartesian_expand, expand_pattern};
use std::collections::HashMap;

fn bench_expand_pattern(c: &mut Criterion) {
    let mut values = WildcardValues::new();
    values.insert("sample".to_string(), "SAMPLE_001".to_string());
    values.insert("read".to_string(), "1".to_string());
    values.insert("chr".to_string(), "chr1".to_string());

    let pattern = "results/{sample}/{chr}/reads_R{read}.fastq.gz";

    c.bench_function("expand_pattern", |b| {
        b.iter(|| {
            expand_pattern(black_box(pattern), black_box(&values)).unwrap();
        })
    });
}

fn bench_cartesian_expand(c: &mut Criterion) {
    let mut variables = HashMap::new();
    variables.insert(
        "sample".to_string(),
        (0..10).map(|i| format!("S{:03}", i)).collect(),
    );
    variables.insert("read".to_string(), vec!["1".to_string(), "2".to_string()]);
    variables.insert(
        "chr".to_string(),
        (1..23).map(|i| format!("chr{}", i)).collect(),
    );

    let pattern = "results/{sample}/{chr}/reads_R{read}.fastq.gz";

    c.bench_function("cartesian_expand_large", |b| {
        b.iter(|| {
            cartesian_expand(black_box(pattern), black_box(&variables));
        })
    });
}

criterion_group!(benches, bench_expand_pattern, bench_cartesian_expand);
criterion_main!(benches);
