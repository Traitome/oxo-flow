//! Benchmark: DAG construction scalability.
//!
//! Goal: <50ms for 500 rules.

use criterion::{BenchmarkId, Criterion, black_box, criterion_group, criterion_main};
use oxo_flow_core::WorkflowConfig;
use oxo_flow_core::dag::WorkflowDag;

fn build_rules_toml(count: usize) -> String {
    let mut toml = String::from("[workflow]\nname = \"bench\"\nversion = \"0.1.0\"\n");
    for i in 0..count {
        let input = if i > 0 {
            format!("input = [\"output_{}.txt\"]\n", i - 1)
        } else {
            String::new()
        };
        toml.push_str(&format!(
            "[[rules]]\nname = \"rule_{i}\"\nshell = \"echo step_{i}\"\n{input}output = [\"output_{i}.txt\"]\n"
        ));
    }
    toml
}

fn bench_dag_scale(c: &mut Criterion) {
    let mut group = c.benchmark_group("dag_scale");

    let toml_10 = build_rules_toml(10);
    let toml_50 = build_rules_toml(50);
    let toml_100 = build_rules_toml(100);
    let toml_500 = build_rules_toml(500);

    let wf_10 = WorkflowConfig::parse(&toml_10).unwrap();
    let wf_50 = WorkflowConfig::parse(&toml_50).unwrap();
    let wf_100 = WorkflowConfig::parse(&toml_100).unwrap();
    let wf_500 = WorkflowConfig::parse(&toml_500).unwrap();

    for (name, wf) in [
        ("10_rules", &wf_10),
        ("50_rules", &wf_50),
        ("100_rules", &wf_100),
        ("500_rules", &wf_500),
    ] {
        group.bench_function(BenchmarkId::new("dag_build", name), |b| {
            b.iter(|| {
                let dag = WorkflowDag::from_rules(&wf.rules).unwrap();
                black_box(dag)
            })
        });

        group.bench_function(BenchmarkId::new("topological_order", name), |b| {
            let dag = WorkflowDag::from_rules(&wf.rules).unwrap();
            b.iter(|| {
                let order = dag.execution_order().unwrap();
                black_box(order)
            })
        });

        group.bench_function(BenchmarkId::new("parallel_groups", name), |b| {
            let dag = WorkflowDag::from_rules(&wf.rules).unwrap();
            b.iter(|| {
                let groups = dag.parallel_groups().unwrap();
                black_box(groups)
            })
        });
    }

    group.finish();
}

criterion_group!(benches, bench_dag_scale);
criterion_main!(benches);
