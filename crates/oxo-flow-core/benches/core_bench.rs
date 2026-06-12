use criterion::{Criterion, black_box, criterion_group, criterion_main};
use oxo_flow_core::config::WorkflowConfig;
use oxo_flow_core::dag::WorkflowDag;
use oxo_flow_core::rule::Rule;

// ---------------------------------------------------------------------------
// DAG construction benchmarks
// ---------------------------------------------------------------------------

/// Build a chain of `count` rules: step_0 -> step_1 -> ... -> step_{count-1}.
fn build_chain_rules(count: usize) -> Vec<Rule> {
    let mut rules = Vec::with_capacity(count);
    for i in 0..count {
        let prev_output = if i > 0 {
            format!("step_{}_output.txt", i - 1)
        } else {
            "input.txt".to_string()
        };
        rules.push(Rule {
            name: format!("step_{i}"),
            input: vec![prev_output].into(),
            output: vec![format!("step_{i}_output.txt")].into(),
            shell: Some("echo {input[0]} > {output[0]}".to_string()),
            ..Default::default()
        });
    }
    rules
}

fn bench_dag_chain_10(c: &mut Criterion) {
    let rules = build_chain_rules(10);
    c.bench_function("dag_chain_10_rules", |b| {
        b.iter(|| {
            let dag = WorkflowDag::from_rules(black_box(&rules)).unwrap();
            black_box(dag.validate().unwrap());
        })
    });
}

fn bench_dag_chain_100(c: &mut Criterion) {
    let rules = build_chain_rules(100);
    c.bench_function("dag_chain_100_rules", |b| {
        b.iter(|| {
            let dag = WorkflowDag::from_rules(black_box(&rules)).unwrap();
            black_box(dag.validate().unwrap());
        })
    });
}

fn bench_dag_chain_1000(c: &mut Criterion) {
    let rules = build_chain_rules(1000);
    c.bench_function("dag_chain_1000_rules", |b| {
        b.iter(|| {
            let dag = WorkflowDag::from_rules(black_box(&rules)).unwrap();
            black_box(dag.validate().unwrap());
        })
    });
}

fn bench_dag_execution_order_100(c: &mut Criterion) {
    let rules = build_chain_rules(100);
    let dag = WorkflowDag::from_rules(&rules).unwrap();
    c.bench_function("dag_execution_order_100_rules", |b| {
        b.iter(|| {
            black_box(dag.execution_order().unwrap());
        })
    });
}

fn bench_dag_parallel_groups_100(c: &mut Criterion) {
    let rules = build_chain_rules(100);
    let dag = WorkflowDag::from_rules(&rules).unwrap();
    c.bench_function("dag_parallel_groups_100_rules", |b| {
        b.iter(|| {
            black_box(dag.parallel_groups().unwrap());
        })
    });
}

// ---------------------------------------------------------------------------
// WorkflowConfig parsing benchmarks
// ---------------------------------------------------------------------------

fn make_workflow_toml(rule_count: usize) -> String {
    let mut toml = format!(
        r#"
[workflow]
name = "bench-workflow"
version = "1.0.0"
"#
    );
    for i in 0..rule_count {
        toml.push_str(&format!(
            r#"
[[rules]]
name = "step_{i}"
input = ["input_{i}.txt"]
output = ["output_{i}.txt"]
shell = "echo processed_{i} > output_{i}.txt"
threads = 2
"#,
        ));
    }
    toml
}

fn bench_parse_10_rules(c: &mut Criterion) {
    let toml = make_workflow_toml(10);
    c.bench_function("parse_10_rules", |b| {
        b.iter(|| {
            let config = WorkflowConfig::parse(black_box(&toml)).unwrap();
            black_box(config.validate().unwrap());
        })
    });
}

fn bench_parse_100_rules(c: &mut Criterion) {
    let toml = make_workflow_toml(100);
    c.bench_function("parse_100_rules", |b| {
        b.iter(|| {
            let config = WorkflowConfig::parse(black_box(&toml)).unwrap();
            black_box(config.validate().unwrap());
        })
    });
}

fn bench_expand_1000_wildcards(c: &mut Criterion) {
    use oxo_flow_core::wildcard::cartesian_expand;
    use std::collections::HashMap;

    let mut variables = HashMap::new();
    variables.insert(
        "sample".to_string(),
        (0..100).map(|i| format!("S{i:03}")).collect(),
    );
    variables.insert("read".to_string(), vec!["1".to_string(), "2".to_string()]);
    variables.insert(
        "chr".to_string(),
        (1..6).map(|i| format!("chr{i}")).collect(),
    );

    let pattern = "results/{sample}/{chr}/reads_R{read}.fastq.gz";

    c.bench_function("cartesian_expand_1000_combinations", |b| {
        b.iter(|| {
            let result = cartesian_expand(black_box(pattern), black_box(&variables));
            black_box(result.len());
        })
    });
}

criterion_group!(
    benches,
    bench_dag_chain_10,
    bench_dag_chain_100,
    bench_dag_chain_1000,
    bench_dag_execution_order_100,
    bench_dag_parallel_groups_100,
    bench_parse_10_rules,
    bench_parse_100_rules,
    bench_expand_1000_wildcards,
);
criterion_main!(benches);
