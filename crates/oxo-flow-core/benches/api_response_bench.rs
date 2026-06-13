//! Benchmark: API response serialization speed.
//!
//! Goal: p50 <5ms, p99 <50ms for typical API responses.

use criterion::{BenchmarkId, Criterion, black_box, criterion_group, criterion_main};
use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize)]
struct RunStatusResponse {
    status: String,
    phase: String,
    nodes: Vec<NodeStatusItem>,
    timeline: Vec<TimelineEvent>,
}

#[derive(Serialize, Deserialize)]
struct NodeStatusItem {
    rule: String,
    status: String,
    started_at: Option<String>,
    duration_ms: Option<u64>,
    exit_code: Option<i32>,
}

#[derive(Serialize, Deserialize)]
struct TimelineEvent {
    timestamp: String,
    event: String,
    node: Option<String>,
    message: Option<String>,
}

#[derive(Serialize, Deserialize)]
struct DagStatusResponse {
    nodes: Vec<DagNode>,
    edges: Vec<DagEdge>,
    parallel_groups: Vec<Vec<String>>,
    critical_path: Vec<String>,
    metrics: DagMetrics,
}

#[derive(Serialize, Deserialize)]
struct DagNode {
    id: String,
    label: String,
    status: String,
    color: String,
    duration_ms: Option<u64>,
    exit_code: Option<i32>,
}

#[derive(Serialize, Deserialize)]
struct DagEdge {
    source: String,
    target: String,
}

#[derive(Serialize, Deserialize)]
struct DagMetrics {
    total_nodes: usize,
    completed_nodes: usize,
    failed_nodes: usize,
    running_nodes: usize,
    pending_nodes: usize,
    eta_ms: Option<u64>,
}

fn build_run_status(nodes: usize) -> RunStatusResponse {
    RunStatusResponse {
        status: "running".into(),
        phase: "executing".into(),
        nodes: (0..nodes)
            .map(|i| NodeStatusItem {
                rule: format!("rule_{i:03}"),
                status: if i % 3 == 0 {
                    "running".into()
                } else if i % 5 == 0 {
                    "failed".into()
                } else {
                    "success".into()
                },
                started_at: Some("2024-01-01T00:00:00Z".into()),
                duration_ms: Some((i * 100) as u64),
                exit_code: Some(if i % 5 == 0 { 1 } else { 0 }),
            })
            .collect(),
        timeline: vec![
            TimelineEvent {
                timestamp: "2024-01-01T00:00:00Z".into(),
                event: "started".into(),
                node: Some("rule_000".into()),
                message: Some("run started".into()),
            },
            TimelineEvent {
                timestamp: "2024-01-01T00:00:05Z".into(),
                event: "completed".into(),
                node: Some("rule_000".into()),
                message: Some("rule completed".into()),
            },
        ],
    }
}

fn build_dag_status(nodes: usize) -> DagStatusResponse {
    let dag_nodes: Vec<DagNode> = (0..nodes)
        .map(|i| DagNode {
            id: format!("rule_{i:03}"),
            label: format!("Rule {i:03}"),
            status: "success".into(),
            color: "green".into(),
            duration_ms: Some(100),
            exit_code: Some(0),
        })
        .collect();
    let edges: Vec<DagEdge> = (1..nodes)
        .map(|i| DagEdge {
            source: format!("rule_{:03}", i - 1),
            target: format!("rule_{i:03}"),
        })
        .collect();
    let node_ids: Vec<String> = dag_nodes.iter().map(|n| n.id.clone()).collect();
    let critical_path: Vec<String> = (0..nodes).map(|i| format!("rule_{i:03}")).collect();
    DagStatusResponse {
        nodes: dag_nodes,
        edges,
        parallel_groups: vec![node_ids],
        critical_path,
        metrics: DagMetrics {
            total_nodes: nodes,
            completed_nodes: nodes,
            failed_nodes: 0,
            running_nodes: 0,
            pending_nodes: 0,
            eta_ms: None,
        },
    }
}

fn bench_api_serialization(c: &mut Criterion) {
    let mut group = c.benchmark_group("api_response");

    let status_10 = build_run_status(10);
    group.bench_function(BenchmarkId::new("run_status_json", "10_nodes"), |b| {
        b.iter(|| {
            let json = serde_json::to_string(&status_10).unwrap();
            black_box(json)
        })
    });

    let status_200 = build_run_status(200);
    group.bench_function(BenchmarkId::new("run_status_json", "200_nodes"), |b| {
        b.iter(|| {
            let json = serde_json::to_string(&status_200).unwrap();
            black_box(json)
        })
    });

    let dag_10 = build_dag_status(10);
    group.bench_function(BenchmarkId::new("dag_status_json", "10_nodes"), |b| {
        b.iter(|| {
            let json = serde_json::to_string(&dag_10).unwrap();
            black_box(json)
        })
    });

    let dag_200 = build_dag_status(200);
    group.bench_function(BenchmarkId::new("dag_status_json", "200_nodes"), |b| {
        b.iter(|| {
            let json = serde_json::to_string(&dag_200).unwrap();
            black_box(json)
        })
    });

    let json_10 = serde_json::to_string(&status_10).unwrap();
    group.bench_function(BenchmarkId::new("run_status_parse", "10_nodes"), |b| {
        b.iter(|| {
            let resp: RunStatusResponse = serde_json::from_str(&json_10).unwrap();
            black_box(resp)
        })
    });

    group.finish();
}

criterion_group!(benches, bench_api_serialization);
criterion_main!(benches);
