//! Wildcard expansion benchmarks.
//!
//! Measures expansion time for pattern substitution and Cartesian-product
//! generation at increasing dimensions.  These operations are at the heart
//! of sample-multiplication in bioinformatics workflows.
//!
//! # Methodology
//!
//! Benchmarks cover three core wildcard operations:
//!
//! | Operation           | Input                          | Output    |
//! |---------------------|--------------------------------|-----------|
//! | `expand_pattern`    | 1 pattern + values             | 1 string  |
//! | `cartesian_expand`  | 1 pattern + variable lists     | N strings |
//! | `pattern_to_regex`  | 1 pattern                      | 1 regex   |
//!
//! Variable counts are chosen to reflect real pipeline sizes: 10–1000
//! samples, 2 read ends, and up to 23 chromosomes for WGS workloads.

use criterion::{Criterion, black_box, criterion_group, criterion_main};
use oxo_flow_core::wildcard::{WildcardValues, cartesian_expand, expand_pattern, pattern_to_regex};
use std::collections::HashMap;

// ---------------------------------------------------------------------------
// expand_pattern — single substitution
// ---------------------------------------------------------------------------

fn expand_basic(c: &mut Criterion) {
    let mut values = WildcardValues::new();
    values.insert("sample".to_string(), "SAMPLE_001".to_string());
    let pattern = "results/{sample}/aligned.bam";
    c.bench_function("wildcard/expand_basic", |b| {
        b.iter(|| expand_pattern(black_box(pattern), black_box(&values)).unwrap())
    });
}

fn expand_multi_var(c: &mut Criterion) {
    let mut values = WildcardValues::new();
    values.insert("sample".to_string(), "S001".to_string());
    values.insert("read".to_string(), "1".to_string());
    values.insert("chr".to_string(), "chr1".to_string());
    let pattern = "results/{sample}/{chr}/reads_R{read}.fastq.gz";
    c.bench_function("wildcard/expand_multi_var", |b| {
        b.iter(|| expand_pattern(black_box(pattern), black_box(&values)).unwrap())
    });
}

fn expand_nested_path(c: &mut Criterion) {
    let mut values = WildcardValues::new();
    values.insert("sample".to_string(), "Project_ABC/S001".to_string());
    values.insert("chr".to_string(), "chr1".to_string());
    let pattern = "analysis/{sample}/alignment/{chr}/output.bam";
    c.bench_function("wildcard/expand_nested_path", |b| {
        b.iter(|| expand_pattern(black_box(pattern), black_box(&values)).unwrap())
    });
}

// ---------------------------------------------------------------------------
// cartesian_expand — combinatorial patterns
// ---------------------------------------------------------------------------

fn cartesian_10_samples_2_reads(c: &mut Criterion) {
    let mut vars = HashMap::new();
    vars.insert(
        "sample".to_string(),
        (0..10).map(|i| format!("S{i:03}")).collect(),
    );
    vars.insert("read".to_string(), vec!["1".to_string(), "2".to_string()]);
    let pattern = "data/{sample}/reads_R{read}.fastq.gz";
    c.bench_function("wildcard/cartesian_10x2", |b| {
        b.iter(|| cartesian_expand(black_box(pattern), black_box(&vars)))
    });
}

fn cartesian_100_samples_2_reads_23_chr(c: &mut Criterion) {
    let mut vars = HashMap::new();
    vars.insert(
        "sample".to_string(),
        (0..100).map(|i| format!("S{i:03}")).collect(),
    );
    vars.insert("read".to_string(), vec!["1".to_string(), "2".to_string()]);
    vars.insert(
        "chr".to_string(),
        (1..24).map(|i| format!("chr{i}")).collect(),
    );
    let pattern = "results/{sample}/{chr}/reads_R{read}.fastq.gz";
    c.bench_function("wildcard/cartesian_100x2x23", |b| {
        b.iter(|| cartesian_expand(black_box(pattern), black_box(&vars)))
    });
}

fn cartesian_1000_samples_2_reads(c: &mut Criterion) {
    let mut vars = HashMap::new();
    vars.insert(
        "sample".to_string(),
        (0..1000).map(|i| format!("S{i:04}")).collect(),
    );
    vars.insert("read".to_string(), vec!["1".to_string(), "2".to_string()]);
    let pattern = "data/{sample}/reads_R{read}.fastq.gz";
    c.bench_function("wildcard/cartesian_1000x2", |b| {
        b.iter(|| cartesian_expand(black_box(pattern), black_box(&vars)))
    });
}

// ---------------------------------------------------------------------------
// pattern_to_regex — regex conversion
// ---------------------------------------------------------------------------

fn regex_simple(c: &mut Criterion) {
    c.bench_function("wildcard/regex_simple", |b| {
        b.iter(|| pattern_to_regex(black_box("reads/{sample}.fastq.gz")).unwrap())
    });
}

fn regex_multi_var(c: &mut Criterion) {
    c.bench_function("wildcard/regex_multi", |b| {
        b.iter(|| {
            pattern_to_regex(black_box("data/{sample}/{chr}/reads_R{read}.fastq.gz")).unwrap()
        })
    });
}

fn regex_constrained(c: &mut Criterion) {
    c.bench_function("wildcard/regex_constrained", |b| {
        b.iter(|| {
            pattern_to_regex(black_box(
                "data/{sample: S\\\\d+}/reads_R{read: [12]}.fastq.gz",
            ))
            .unwrap()
        })
    });
}

// ---------------------------------------------------------------------------
// groups
// ---------------------------------------------------------------------------

criterion_group!(
    benches,
    expand_basic,
    expand_multi_var,
    expand_nested_path,
    cartesian_10_samples_2_reads,
    cartesian_100_samples_2_reads_23_chr,
    cartesian_1000_samples_2_reads,
    regex_simple,
    regex_multi_var,
    regex_constrained,
);
criterion_main!(benches);
