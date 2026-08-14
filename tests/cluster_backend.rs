//! P1 acceptance tests for issue #78 — static plan + pluggable executors.
//!
//! 1. Local execution and cluster-backend execution (mock SLURM) of the same
//!    workflow produce the same checkpoint semantics.
//! 2. The dry-run preview's will-run set equals what the driver submits.
//! 3. Poll-timeout cancels in-flight jobs; failures propagate.
//! 4. `cluster submit` output stays byte-identical through the trait render.

use oxo_flow_core::backend::ScheduledPlan;
use oxo_flow_core::backend::cluster::ClusterExecutor;
use oxo_flow_core::backend::driver::{BackendDriver, DriverConfig, DriverOptions};
use oxo_flow_core::cluster::{ClusterBackend, ClusterJobConfig};
use oxo_flow_core::config::WorkflowConfig;
use oxo_flow_core::dag::WorkflowDag;
use oxo_flow_core::environment::EnvironmentResolver;
use oxo_flow_core::executor::JobStatus;
use oxo_flow_core::executor::checkpoint::{
    BenchmarkRecord, CheckpointState, snapshot_input_manifest,
};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::process::Command as StdCommand;
use std::sync::Arc;

const WORKFLOW: &str = r#"
[workflow]
name = "parity"

[[sample_groups]]
name = "batch"
samples = ["S1", "S2", "S3"]

[[rules]]
name = "align"
input = ["data/{sample}.fq"]
output = ["aln/{sample}.bam"]
shell = "cp data/{sample}.fq aln/{sample}.bam"

[[rules]]
name = "stats"
input = ["aln/{sample}.bam"]
output = ["stats/{sample}.txt"]
shell = "wc -l aln/{sample}.bam > stats/{sample}.txt"
"#;

fn write_workflow(dir: &Path) {
    std::fs::write(dir.join("wf.oxoflow"), WORKFLOW).unwrap();
    for s in ["S1", "S2", "S3"] {
        let data = dir.join("data");
        std::fs::create_dir_all(&data).unwrap();
        std::fs::write(data.join(format!("{s}.fq")), format!(">{s}\nACGT\n")).unwrap();
    }
}

/// Locate a workspace binary (mirrors the helper in cli_integration.rs).
fn workspace_bin(name: &str) -> PathBuf {
    let target_dir = std::env::current_exe()
        .expect("cannot find current test executable path")
        .parent()
        .expect("no parent dir for test exe")
        .parent()
        .expect("no grandparent dir for test exe")
        .to_path_buf();
    let candidate = target_dir.join(name);
    if candidate.exists() {
        return candidate;
    }
    panic!(
        "could not find binary '{name}' in target directory; run `cargo build --workspace` first"
    );
}

fn fixtures_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/mock-scheduler")
}

fn cluster_config() -> ClusterJobConfig {
    ClusterJobConfig {
        backend: ClusterBackend::Slurm,
        queue: None,
        account: None,
        walltime: None,
        extra_args: vec![],
    }
}

/// Run the driver over a workflow directory with the mock scheduler.
fn run_driver(
    workdir: &Path,
    run_dir: &Path,
    scheduler_state: &Path,
    to_run: &HashSet<String>,
    config: DriverConfig,
) -> Result<Vec<oxo_flow_core::executor::JobRecord>, String> {
    let wf = workdir.join("wf.oxoflow");
    let mut wf_config = WorkflowConfig::from_file(&wf).map_err(|e| e.to_string())?;
    wf_config.apply_defaults();
    wf_config.expand_wildcards().map_err(|e| e.to_string())?;
    let dag = WorkflowDag::from_rules(&wf_config.rules).map_err(|e| e.to_string())?;
    let values: HashMap<String, String> = wf_config
        .config
        .iter()
        .map(|(k, v)| (format!("config.{k}"), v.to_string()))
        .collect();
    let mut plan = ScheduledPlan::build(
        &wf_config,
        &dag,
        workdir,
        &EnvironmentResolver::new(),
        &values,
    )
    .map_err(|e| e.to_string())?;
    let executor = Arc::new(
        ClusterExecutor::new(ClusterBackend::Slurm, cluster_config())
            .with_scheduler_dir(fixtures_dir())
            .with_env("MOCK_SCHEDULER_DIR", &scheduler_state.to_string_lossy()),
    );
    let driver = BackendDriver::new(executor, config);
    let runtime = tokio::runtime::Runtime::new().unwrap();
    runtime
        .block_on(driver.run(
            &mut plan,
            to_run,
            DriverOptions {
                run_dir,
                on_checkpoint: None,
                merge: None,
            },
        ))
        .map_err(|e| e.to_string())
}

fn sorted_completed(checkpoint: &Path) -> Vec<String> {
    let ck = CheckpointState::load_from_file(checkpoint).unwrap();
    let mut v: Vec<String> = ck.completed_rules.into_iter().collect();
    v.sort();
    v
}

/// All files under `dir` except engine internals: (relative path, bytes).
fn payload_files(dir: &Path) -> Vec<(String, Vec<u8>)> {
    fn walk(dir: &Path, base: &Path, out: &mut Vec<(String, Vec<u8>)>) {
        for e in std::fs::read_dir(dir).unwrap().flatten() {
            let p = e.path();
            let rel = p.strip_prefix(base).unwrap().to_string_lossy().to_string();
            if rel == "wf.oxoflow" || rel == ".oxo-flow" || rel.starts_with(".oxo-flow/") {
                continue;
            }
            if p.is_dir() {
                walk(&p, base, out);
            } else {
                out.push((rel, std::fs::read(&p).unwrap()));
            }
        }
    }
    let mut out = Vec::new();
    walk(dir, dir, &mut out);
    out.sort_by(|a, b| a.0.cmp(&b.0));
    out
}

#[test]
fn local_run_and_backend_run_produce_same_checkpoint_semantics() {
    // ── Local path: the real CLI ──────────────────────────────────────
    let dir_a = tempfile::tempdir().unwrap();
    write_workflow(dir_a.path());
    let status = StdCommand::new(workspace_bin("oxo-flow"))
        .args(["run", "wf.oxoflow", "-j", "2"])
        .current_dir(dir_a.path())
        .status()
        .unwrap();
    assert!(status.success(), "local run failed");
    let completed_a = sorted_completed(&dir_a.path().join(".oxo-flow/checkpoint.json"));
    assert_eq!(completed_a.len(), 6, "expected align+stats × 3 samples");

    // ── Backend path: driver + mock SLURM ─────────────────────────────
    let dir_b = tempfile::tempdir().unwrap();
    write_workflow(dir_b.path());
    let state = tempfile::tempdir().unwrap();
    let run_dir = tempfile::tempdir().unwrap();
    let to_run: HashSet<String> = {
        let wf = dir_b.path().join("wf.oxoflow");
        let mut config = WorkflowConfig::from_file(&wf).unwrap();
        config.apply_defaults();
        config.expand_wildcards().unwrap();
        let dag = WorkflowDag::from_rules(&config.rules).unwrap();
        dag.execution_order().unwrap().into_iter().collect()
    };
    let records = run_driver(
        dir_b.path(),
        run_dir.path(),
        state.path(),
        &to_run,
        DriverConfig {
            max_submitted: 4,
            poll_interval: std::time::Duration::from_millis(50),
            poll_timeout: Some(std::time::Duration::from_secs(30)),
        },
    )
    .unwrap();
    assert!(
        records.iter().all(|r| r.status == JobStatus::Success),
        "{records:?}"
    );

    // Mirror the local path's checkpoint recording.
    let mut ck = CheckpointState::new();
    let wf = dir_b.path().join("wf.oxoflow");
    let mut config = WorkflowConfig::from_file(&wf).unwrap();
    config.apply_defaults();
    config.expand_wildcards().unwrap();
    let values: HashMap<String, String> = config
        .config
        .iter()
        .map(|(k, v)| (format!("config.{k}"), v.to_string()))
        .collect();
    for r in &records {
        let wall = r
            .finished_at
            .zip(r.started_at)
            .map(|(f, s)| (f - s).num_milliseconds() as f64 / 1000.0)
            .unwrap_or(0.0);
        ck.mark_completed(
            &r.rule,
            BenchmarkRecord {
                rule: r.rule.clone(),
                wall_time_secs: wall,
                max_memory_mb: None,
                cpu_seconds: None,
                retries: 0,
            },
        );
        if let Some(rule) = config.get_rule(&r.rule)
            && let Some(manifest) = snapshot_input_manifest(
                rule,
                dir_b.path(),
                &values,
                &oxo_flow_core::storage::StorageResolver::with_local(),
            )
            .unwrap()
        {
            ck.record_input_manifest(&r.rule, manifest);
        }
    }
    ck.save_to_file(&dir_b.path().join(".oxo-flow/checkpoint.json"))
        .unwrap();
    let completed_b = sorted_completed(&dir_b.path().join(".oxo-flow/checkpoint.json"));

    // ── Parity assertions ─────────────────────────────────────────────
    assert_eq!(completed_a, completed_b);
    let ck_a =
        CheckpointState::load_from_file(&dir_a.path().join(".oxo-flow/checkpoint.json")).unwrap();
    let ck_b =
        CheckpointState::load_from_file(&dir_b.path().join(".oxo-flow/checkpoint.json")).unwrap();
    fn keys<T>(m: &HashMap<String, T>) -> Vec<String> {
        let mut v: Vec<String> = m.keys().cloned().collect();
        v.sort();
        v
    }
    assert_eq!(keys(&ck_a.benchmarks), keys(&ck_b.benchmarks));
    assert_eq!(keys(&ck_a.input_manifests), keys(&ck_b.input_manifests));
    assert_eq!(payload_files(dir_a.path()), payload_files(dir_b.path()));
}

#[test]
fn dry_run_will_run_set_equals_driver_submitted_set() {
    // ── dry-run JSON on the real CLI ──────────────────────────────────
    let dir_a = tempfile::tempdir().unwrap();
    write_workflow(dir_a.path());
    let out = StdCommand::new(workspace_bin("oxo-flow"))
        .args(["dry-run", "wf.oxoflow", "--json"])
        .current_dir(dir_a.path())
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "dry-run failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let json: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    let plan = json["checkpoint_preview"]["plan"].as_array().unwrap();
    let mut will_run: Vec<String> = plan
        .iter()
        .filter(|p| {
            let s = p["status"].as_str().unwrap();
            s != "skip" && s != "skip-when-condition"
        })
        .map(|p| p["name"].as_str().unwrap().to_string())
        .collect();
    will_run.sort();
    assert_eq!(will_run.len(), 6, "fresh workdir: everything runs");

    // ── Driver with the same will-run set ─────────────────────────────
    let dir_b = tempfile::tempdir().unwrap();
    write_workflow(dir_b.path());
    let state = tempfile::tempdir().unwrap();
    let run_dir = tempfile::tempdir().unwrap();
    let to_run: HashSet<String> = will_run.iter().cloned().collect();
    run_driver(
        dir_b.path(),
        run_dir.path(),
        state.path(),
        &to_run,
        DriverConfig {
            max_submitted: 4,
            poll_interval: std::time::Duration::from_millis(50),
            poll_timeout: Some(std::time::Duration::from_secs(30)),
        },
    )
    .unwrap();

    let events = std::fs::read_to_string(run_dir.path().join("events.jsonl")).unwrap();
    let mut submitted: Vec<String> = events
        .lines()
        .filter_map(|l| {
            let v: serde_json::Value = serde_json::from_str(l).ok()?;
            (v["t"] == "SUBMITTED").then(|| v["rule"].as_str().unwrap().to_string())
        })
        .collect();
    submitted.sort();
    assert_eq!(submitted, will_run);
}

#[test]
fn poll_timeout_cancels_inflight_jobs() {
    let dir = tempfile::tempdir().unwrap();
    let workflow = r#"
[workflow]
name = "timeout"

[[rules]]
name = "slow"
shell = "sleep 30"
output = ["slow.done"]

[[rules]]
name = "fail"
shell = "exit 3"
output = ["fail.done"]
"#;
    std::fs::write(dir.path().join("wf.oxoflow"), workflow).unwrap();
    let state = tempfile::tempdir().unwrap();
    let run_dir = tempfile::tempdir().unwrap();
    let to_run: HashSet<String> = ["slow".to_string(), "fail".to_string()]
        .into_iter()
        .collect();
    let err = run_driver(
        dir.path(),
        run_dir.path(),
        state.path(),
        &to_run,
        DriverConfig {
            max_submitted: 2,
            poll_interval: std::time::Duration::from_millis(50),
            poll_timeout: Some(std::time::Duration::from_secs(1)),
        },
    )
    .unwrap_err();
    assert!(err.contains("poll timeout"), "{err}");

    // The still-running job was cancelled.
    let slow_job_id = std::fs::read_to_string(run_dir.path().join("jobs/slow/job.id")).unwrap();
    let job_state =
        std::fs::read_to_string(state.path().join(format!("jobs/{slow_job_id}/state"))).unwrap();
    assert_eq!(job_state.trim(), "CANCELLED");
}

#[test]
fn cluster_submit_renders_through_trait_byte_identical() {
    let dir = tempfile::tempdir().unwrap();
    // Wildcard-free workflow: `cluster submit` does not expand wildcards
    // today (that fix belongs to #74 phase 1) — parity is asserted on the
    // template rules themselves.
    let workflow = r#"
[workflow]
name = "render-parity"

[[rules]]
name = "prep"
shell = "cat raw.txt > prep.txt"
output = ["prep.txt"]

[[rules]]
name = "finish"
input = ["prep.txt"]
shell = "wc -l prep.txt > finish.txt"
output = ["finish.txt"]
"#;
    std::fs::write(dir.path().join("wf.oxoflow"), workflow).unwrap();
    let out_dir = dir.path().join("scripts");
    let status = StdCommand::new(workspace_bin("oxo-flow"))
        .args(["cluster", "submit", "wf.oxoflow", "-b", "slurm", "-o"])
        .arg(&out_dir)
        .current_dir(dir.path())
        .status()
        .unwrap();
    assert!(status.success(), "cluster submit failed");
    // logs/ created before the scripts are written (issue #74 phase-1 note 2)
    assert!(dir.path().join("logs").is_dir());

    // Recompute what the old path would have produced and compare bytes.
    let wf = dir.path().join("wf.oxoflow");
    let mut config = WorkflowConfig::from_file(&wf).unwrap();
    config.apply_defaults();
    config.expand_wildcards().unwrap();
    let dag = WorkflowDag::from_rules(&config.rules).unwrap();
    let order = dag.execution_order().unwrap();
    let env_resolver = EnvironmentResolver::new();
    let values: HashMap<String, String> = config
        .config
        .iter()
        .map(|(k, v)| (format!("config.{k}"), v.to_string()))
        .collect();
    let mut compared = 0;
    for rule_name in order {
        let rule = config.get_rule(&rule_name).unwrap();
        let Some(shell_cmd) = oxo_flow_core::executor::process::build_execution_command(
            rule,
            &values,
            &config.workflow.interpreter_map,
        ) else {
            continue;
        };
        let wrapped = env_resolver
            .wrap_command(
                &shell_cmd,
                &rule.environment,
                Some(&rule.resources),
                Path::new("."),
            )
            .unwrap();
        let expected = oxo_flow_core::cluster::generate_submit_script(
            &ClusterBackend::Slurm,
            rule,
            &wrapped,
            &cluster_config(),
        );
        let actual = std::fs::read_to_string(out_dir.join(format!("{rule_name}.sh"))).unwrap();
        assert_eq!(actual, expected, "render diverged for rule '{rule_name}'");
        compared += 1;
    }
    assert!(compared > 0);
}
