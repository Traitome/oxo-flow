//! Workflow configuration parsing benchmarks.
//!
//! Measures TOML parsing, validation, and serialisation round-trips.
//! Timings are reported for the full `parse → validate → prepare`
//! lifecycle at increasing workflow sizes (1–10 000 rules).

use criterion::{Criterion, black_box, criterion_group, criterion_main};
use oxo_flow_core::config::{Ready, WorkflowConfig, WorkflowState};

/// Generate a TOML workflow with `count` sequential rules and optional pairs.
fn make_workflow_toml(count: usize, with_pairs: bool) -> String {
    let mut toml = format!(
        r#"
[workflow]
name = "bench-{count}"
version = "1.0.0"

{}

[[rules]]
name = "step_0"
input = ["input.txt"]
output = ["{maybe_pairs}step_0_output.txt"]
shell = "echo {{input[0]}} > {{output[0]}}"
threads = 2
"#,
        if with_pairs {
            r#"[[pairs]]
experiment = "tumor/{sample}"
control = "normal/{sample}"
"#
        } else {
            ""
        },
        maybe_pairs = if with_pairs { "{sample}_" } else { "" },
    );
    for i in 1..count {
        toml.push_str(&format!(
            r#"
[[rules]]
name = "step_{i}"
input = ["step_{prev}_output.txt"]
output = ["{pairs}step_{i}_output.txt"]
shell = "echo processed > {{output[0]}}"
threads = 2
"#,
            i = i,
            prev = i - 1,
            pairs = if with_pairs { "{sample}_" } else { "" },
        ));
    }
    toml
}

/// Parse + validate + prepare a workflow (full lifecycle).
fn parse_to_ready(toml: &str) -> WorkflowState<Ready> {
    let config = WorkflowConfig::parse(toml).unwrap();
    WorkflowState::new(config)
        .validate()
        .unwrap()
        .prepare()
        .unwrap()
}

// ---------------------------------------------------------------------------
// Parse only (no validation)
// ---------------------------------------------------------------------------

fn parse_10(c: &mut Criterion) {
    let toml = make_workflow_toml(10, false);
    c.bench_function("parse/10_simple", |b| {
        b.iter(|| {
            black_box(WorkflowConfig::parse(black_box(&toml)).unwrap());
        })
    });
}

fn parse_100(c: &mut Criterion) {
    let toml = make_workflow_toml(100, false);
    c.bench_function("parse/100_simple", |b| {
        b.iter(|| {
            black_box(WorkflowConfig::parse(black_box(&toml)).unwrap());
        })
    });
}

fn parse_1000(c: &mut Criterion) {
    let toml = make_workflow_toml(1000, false);
    c.bench_function("parse/1000_simple", |b| {
        b.iter(|| {
            black_box(WorkflowConfig::parse(black_box(&toml)).unwrap());
        })
    });
}

// ---------------------------------------------------------------------------
// Full lifecycle (parse + validate + prepare)
// ---------------------------------------------------------------------------

fn lifecycle_10(c: &mut Criterion) {
    let toml = make_workflow_toml(10, false);
    c.bench_function("lifecycle/10_simple", |b| {
        b.iter(|| black_box(parse_to_ready(black_box(&toml))))
    });
}

fn lifecycle_100(c: &mut Criterion) {
    let toml = make_workflow_toml(100, false);
    c.bench_function("lifecycle/100_simple", |b| {
        b.iter(|| black_box(parse_to_ready(black_box(&toml))))
    });
}

fn lifecycle_1000(c: &mut Criterion) {
    let toml = make_workflow_toml(1000, false);
    c.bench_function("lifecycle/1000_simple", |b| {
        b.iter(|| black_box(parse_to_ready(black_box(&toml))))
    });
}

// ---------------------------------------------------------------------------
// With wildcard pairs (experiment-control)
// ---------------------------------------------------------------------------

fn lifecycle_with_pairs_10(c: &mut Criterion) {
    let toml = make_workflow_toml(10, true);
    c.bench_function("lifecycle/10_with_pairs", |b| {
        b.iter(|| black_box(parse_to_ready(black_box(&toml))))
    });
}

fn lifecycle_with_pairs_100(c: &mut Criterion) {
    let toml = make_workflow_toml(100, true);
    c.bench_function("lifecycle/100_with_pairs", |b| {
        b.iter(|| black_box(parse_to_ready(black_box(&toml))))
    });
}

// ---------------------------------------------------------------------------
// Serialisation round-trip (format → parse)
// ---------------------------------------------------------------------------

fn serialise_roundtrip_100(c: &mut Criterion) {
    let toml = make_workflow_toml(100, false);
    let config = WorkflowConfig::parse(&toml).unwrap();
    c.bench_function("serialise/roundtrip_100", |b| {
        b.iter(|| {
            let formatted = oxo_flow_core::format::format_workflow(black_box(&config));
            black_box(WorkflowConfig::parse(&formatted).unwrap());
        })
    });
}

// ---------------------------------------------------------------------------
// groups
// ---------------------------------------------------------------------------

criterion_group!(
    benches,
    parse_10,
    parse_100,
    parse_1000,
    lifecycle_10,
    lifecycle_100,
    lifecycle_1000,
    lifecycle_with_pairs_10,
    lifecycle_with_pairs_100,
    serialise_roundtrip_100,
);
criterion_main!(benches);
