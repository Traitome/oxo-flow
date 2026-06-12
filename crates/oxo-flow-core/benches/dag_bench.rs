//! DAG (Directed Acyclic Graph) construction and analysis benchmarks.
//!
//! Measures the time to build, validate, and analyse DAGs at various
//! scales (10 – 10 000 rules) and topologies (chain, tree, star).
//!
//! # Methodology
//!
//! Each benchmark creates rules synthetically and measures only the
//! operation under test.  Rule creation is excluded from measurements.
//! Topologies are designed to exercise different code paths:
//!
//! | Topology | Edges    | What it stresses             |
//! |----------|----------|------------------------------|
//! | Chain    | O(N)     | Sequential dependency chain  |
//! | Tree     | O(N)     | Branching + merging          |
//! | Star     | O(N)     | Fan-out / fan-in bottlenecks |
//!
//! Results are reported as average wall-clock time per iteration and,
//! where applicable, throughput in rules/second.

use criterion::{Criterion, black_box, criterion_group, criterion_main};
use oxo_flow_core::dag::WorkflowDag;
use oxo_flow_core::rule::Rule;

// ---------------------------------------------------------------------------
// Helper: build rules with a given topology
// ---------------------------------------------------------------------------

/// Chain: step_0 → step_1 → … → step_{count-1}
fn chain_rules(count: usize) -> Vec<Rule> {
    let mut rules = Vec::with_capacity(count);
    for i in 0..count {
        let input = if i == 0 {
            vec!["input.txt".to_string()]
        } else {
            vec![format!("step_{}_output.txt", i - 1)]
        };
        rules.push(Rule {
            name: format!("step_{i}"),
            input: input.into(),
            output: vec![format!("step_{i}_output.txt")].into(),
            shell: Some("echo {input[0]} > {output[0]}".to_string()),
            ..Default::default()
        });
    }
    rules
}

/// Balanced binary tree: nodes arranged so that each parent feeds two
/// children.  `levels` determines the depth (2^levels − 1 total nodes).
fn tree_rules(levels: usize) -> Vec<Rule> {
    let count = (1usize << levels) - 1;
    let mut rules = Vec::with_capacity(count);
    for i in 0..count {
        let parent = if i == 0 { None } else { Some((i - 1) / 2) };
        let input = match parent {
            Some(p) => vec![format!("node_{p}_output.txt")],
            None => vec!["input.txt".to_string()],
        };
        rules.push(Rule {
            name: format!("node_{i}"),
            input: input.into(),
            output: vec![format!("node_{i}_output.txt")].into(),
            shell: Some("echo {input[0]} > {output[0]}".to_string()),
            ..Default::default()
        });
    }
    rules
}

/// Star: one root rule produces output consumed by `count` leaf rules.
fn star_rules(count: usize) -> Vec<Rule> {
    let mut rules = Vec::with_capacity(count + 1);
    // Root
    rules.push(Rule {
        name: "root".to_string(),
        input: vec!["input.txt".to_string()].into(),
        output: vec!["root_output.txt".to_string()].into(),
        shell: Some("echo root".to_string()),
        ..Default::default()
    });
    // Leaves
    for i in 0..count {
        rules.push(Rule {
            name: format!("leaf_{i}"),
            input: vec!["root_output.txt".to_string()].into(),
            output: vec![format!("leaf_{i}_output.txt")].into(),
            shell: Some("echo leaf".to_string()),
            ..Default::default()
        });
    }
    rules
}

// ---------------------------------------------------------------------------
// Benchmark helpers
// ---------------------------------------------------------------------------

fn build_and_validate(rules: &[Rule]) {
    let dag = WorkflowDag::from_rules(rules).unwrap();
    dag.validate().unwrap();
}

// ---------------------------------------------------------------------------
// Chain DAG benchmarks
// ---------------------------------------------------------------------------

fn dag_chain_10(c: &mut Criterion) {
    let rules = chain_rules(10);
    c.bench_function("dag/chain_10", |b| {
        b.iter(|| build_and_validate(black_box(&rules)))
    });
}

fn dag_chain_100(c: &mut Criterion) {
    let rules = chain_rules(100);
    c.bench_function("dag/chain_100", |b| {
        b.iter(|| build_and_validate(black_box(&rules)))
    });
}

fn dag_chain_1000(c: &mut Criterion) {
    let rules = chain_rules(1000);
    c.bench_function("dag/chain_1000", |b| {
        b.iter(|| build_and_validate(black_box(&rules)))
    });
}

fn dag_chain_10000(c: &mut Criterion) {
    let rules = chain_rules(10000);
    c.bench_function("dag/chain_10000", |b| {
        b.iter(|| build_and_validate(black_box(&rules)))
    });
}

// ---------------------------------------------------------------------------
// Tree DAG benchmarks
// ---------------------------------------------------------------------------

fn dag_tree_depth_5(c: &mut Criterion) {
    let rules = tree_rules(5);
    c.bench_function("dag/tree_depth_5", |b| {
        b.iter(|| build_and_validate(black_box(&rules)))
    });
}

fn dag_tree_depth_10(c: &mut Criterion) {
    let rules = tree_rules(10);
    c.bench_function("dag/tree_depth_10", |b| {
        b.iter(|| build_and_validate(black_box(&rules)))
    });
}

// ---------------------------------------------------------------------------
// Star DAG benchmarks
// ---------------------------------------------------------------------------

fn dag_star_100(c: &mut Criterion) {
    let rules = star_rules(100);
    c.bench_function("dag/star_100", |b| {
        b.iter(|| build_and_validate(black_box(&rules)))
    });
}

fn dag_star_1000(c: &mut Criterion) {
    let rules = star_rules(1000);
    c.bench_function("dag/star_1000", |b| {
        b.iter(|| build_and_validate(black_box(&rules)))
    });
}

// ---------------------------------------------------------------------------
// DAG analysis operations (on pre-built DAGs)
// ---------------------------------------------------------------------------

fn dag_analysis_1000(c: &mut Criterion) {
    let rules = chain_rules(1000);
    let dag = WorkflowDag::from_rules(&rules).unwrap();

    let mut group = c.benchmark_group("dag/analysis_1000");
    group.bench_function("execution_order", |b| {
        b.iter(|| dag.execution_order().unwrap())
    });
    group.bench_function("parallel_groups", |b| {
        b.iter(|| dag.parallel_groups().unwrap())
    });
    group.bench_function("critical_path", |b| b.iter(|| dag.critical_path().unwrap()));
    group.bench_function("metrics", |b| b.iter(|| dag.metrics().unwrap()));
    group.bench_function("to_ascii", |b| b.iter(|| dag.to_ascii().unwrap()));
    group.finish();
}

fn dag_analysis_10000(c: &mut Criterion) {
    let rules = chain_rules(10000);
    let dag = WorkflowDag::from_rules(&rules).unwrap();

    let mut group = c.benchmark_group("dag/analysis_10000");
    group.bench_function("execution_order", |b| {
        b.iter(|| dag.execution_order().unwrap())
    });
    group.bench_function("parallel_groups", |b| {
        b.iter(|| dag.parallel_groups().unwrap())
    });
    group.bench_function("critical_path", |b| b.iter(|| dag.critical_path().unwrap()));
    group.bench_function("metrics", |b| b.iter(|| dag.metrics().unwrap()));
    group.finish();
}

// ---------------------------------------------------------------------------
// Topology comparison (1000 rules each, construction + validation)
// ---------------------------------------------------------------------------

fn dag_topology_comparison(c: &mut Criterion) {
    let chain = chain_rules(1000);
    let tree = tree_rules(10); // 1023 nodes ≈ 1000
    let star = star_rules(1000);

    let mut group = c.benchmark_group("dag/topology_1k");
    group.bench_function("chain", |b| {
        b.iter(|| build_and_validate(black_box(&chain)))
    });
    group.bench_function("tree", |b| b.iter(|| build_and_validate(black_box(&tree))));
    group.bench_function("star", |b| b.iter(|| build_and_validate(black_box(&star))));
    group.finish();
}

criterion_group!(
    benches,
    dag_chain_10,
    dag_chain_100,
    dag_chain_1000,
    dag_chain_10000,
    dag_tree_depth_5,
    dag_tree_depth_10,
    dag_star_100,
    dag_star_1000,
    dag_analysis_1000,
    dag_analysis_10000,
    dag_topology_comparison,
);
criterion_main!(benches);
