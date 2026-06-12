//! Schedule resolution benchmarks.
use criterion::{Criterion, black_box, criterion_group, criterion_main};
use oxo_flow_core::dag::WorkflowDag;
use oxo_flow_core::executor::{JobRecord, JobStatus};
use oxo_flow_core::rule::Rule;
use oxo_flow_core::scheduler::SchedulerState;

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

fn setup(count: usize) -> (WorkflowDag, SchedulerState, Vec<Rule>) {
    let rules = chain_rules(count);
    let dag = WorkflowDag::from_rules(&rules).unwrap();
    let names: Vec<&str> = rules.iter().map(|r| r.name.as_str()).collect();
    let state = SchedulerState::new(&names);
    (dag, state, rules)
}

fn ready_chain_10(c: &mut Criterion) {
    let (dag, state, _) = setup(10);
    c.bench_function("scheduler/ready_chain_10", |b| {
        b.iter(|| state.ready_rules(black_box(&dag)).unwrap())
    });
}

fn ready_chain_100(c: &mut Criterion) {
    let (dag, state, _) = setup(100);
    c.bench_function("scheduler/ready_chain_100", |b| {
        b.iter(|| state.ready_rules(black_box(&dag)).unwrap())
    });
}

fn ready_chain_1000(c: &mut Criterion) {
    let (dag, state, _) = setup(1000);
    c.bench_function("scheduler/ready_chain_1000", |b| {
        b.iter(|| state.ready_rules(black_box(&dag)).unwrap())
    });
}

fn strategies_1000(c: &mut Criterion) {
    let (dag, state, rules) = setup(1000);
    let mut group = c.benchmark_group("scheduler/strategies_1000");
    group.bench_function("ready_rules", |b| {
        b.iter(|| state.ready_rules(black_box(&dag)).unwrap())
    });
    group.bench_function("prioritised", |b| {
        b.iter(|| {
            state
                .ready_rules_prioritized(black_box(&dag), black_box(&rules))
                .unwrap()
        })
    });
    group.bench_function("critical_path", |b| {
        b.iter(|| {
            state
                .ready_rules_critical_path(black_box(&dag), black_box(&rules))
                .unwrap()
        })
    });
    group.finish();
}

fn simulate_100(c: &mut Criterion) {
    let rules = chain_rules(100);
    let dag = WorkflowDag::from_rules(&rules).unwrap();
    let names: Vec<&str> = rules.iter().map(|r| r.name.as_str()).collect();
    let now = chrono::Utc::now();
    c.bench_function("scheduler/simulate_100", |b| {
        b.iter(|| {
            let mut state = SchedulerState::new(&names);
            loop {
                let ready = state.ready_rules(&dag).unwrap();
                if ready.is_empty() {
                    break;
                }
                for name in ready {
                    state.mark_running(&name);
                    state.mark_completed(JobRecord {
                        rule: name,
                        status: JobStatus::Success,
                        started_at: Some(now),
                        finished_at: Some(now),
                        exit_code: Some(0),
                        stdout: None,
                        stderr: None,
                        command: None,
                        retries: 0,
                        timeout: None,
                        skip_reason: None,
                    });
                }
            }
            black_box(state.is_complete());
        })
    });
}

criterion_group!(
    benches,
    ready_chain_10,
    ready_chain_100,
    ready_chain_1000,
    strategies_1000,
    simulate_100
);
criterion_main!(benches);
