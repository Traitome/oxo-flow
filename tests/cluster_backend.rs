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
    write_samples(dir);
}

fn write_samples(dir: &Path) {
    for s in ["S1", "S2", "S3"] {
        let data = dir.join("data");
        std::fs::create_dir_all(&data).unwrap();
        std::fs::write(data.join(format!("{s}.fq")), format!(">{s}\nACGT\n")).unwrap();
    }
}

/// A single-rule workflow whose shell exits with `code`.
fn write_failing_workflow(dir: &Path, code: i32) {
    let wf = format!(
        r#"
[workflow]
name = "fail"
version = "0.1.0"

[config]
sample_pattern = "data/{{sample}}.fq"

[[rules]]
name = "align"
input = ["data/{{sample}}.fq"]
output = ["aln/{{sample}}.bam"]
shell = "exit {code}"
"#
    );
    std::fs::write(dir.join("wf.oxoflow"), wf).unwrap();
    write_samples(dir);
}

/// The `to_run` set for every rule instance in the workflow.
fn all_rules(workdir: &Path) -> HashSet<String> {
    let mut config = WorkflowConfig::from_file(&workdir.join("wf.oxoflow")).unwrap();
    config.apply_defaults();
    config.expand_wildcards().unwrap();
    let dag = WorkflowDag::from_rules(&config.rules).unwrap();
    dag.execution_order().unwrap().into_iter().collect()
}

fn fast_driver_config() -> DriverConfig {
    DriverConfig {
        max_submitted: 4,
        max_array_size: 1001,
        no_arrays: false,
        poll_interval: std::time::Duration::from_millis(50),
        poll_timeout: Some(std::time::Duration::from_secs(30)),
        unknown_settle_grace: std::time::Duration::from_secs(90),
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
    run_driver_with_env(workdir, run_dir, scheduler_state, to_run, config, &[])
}

fn run_driver_with_env(
    workdir: &Path,
    run_dir: &Path,
    scheduler_state: &Path,
    to_run: &HashSet<String>,
    config: DriverConfig,
    extra_env: &[(&str, &str)],
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
    let mut executor = ClusterExecutor::new(ClusterBackend::Slurm, cluster_config())
        .with_scheduler_dir(fixtures_dir())
        .with_env("MOCK_SCHEDULER_DIR", &scheduler_state.to_string_lossy());
    for (k, v) in extra_env {
        executor = executor.with_env(k, v);
    }
    let executor = Arc::new(executor);
    let driver = BackendDriver::new(executor, config);
    let runtime = tokio::runtime::Runtime::new().unwrap();
    runtime
        .block_on(driver.run(
            &mut plan,
            to_run,
            DriverOptions {
                sensitive_values: &[],
                on_submit: None,
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
            max_array_size: 1001,
            no_arrays: false,
            poll_interval: std::time::Duration::from_millis(50),
            poll_timeout: Some(std::time::Duration::from_secs(30)),
            unknown_settle_grace: std::time::Duration::from_secs(90),
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
                memory_limit_mb: None,
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
            max_array_size: 1001,
            no_arrays: false,
            poll_interval: std::time::Duration::from_millis(50),
            poll_timeout: Some(std::time::Duration::from_secs(30)),
            unknown_settle_grace: std::time::Duration::from_secs(90),
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
            max_array_size: 1001,
            no_arrays: false,
            poll_interval: std::time::Duration::from_millis(50),
            poll_timeout: Some(std::time::Duration::from_secs(1)),
            unknown_settle_grace: std::time::Duration::from_secs(90),
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
            oxo_flow_core::scheduler::ResourceLimits {
                threads: 4,
                memory_mb: 8192,
            },
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
        let base = oxo_flow_core::cluster::generate_submit_script(
            &ClusterBackend::Slurm,
            rule,
            &wrapped,
            &cluster_config(),
        );
        // The trait path additionally pins the working directory (audit G9):
        // the directive lands right after the shebang.
        let chdir = dir.path().canonicalize().unwrap().display().to_string();
        let chdir_directive = format!("#SBATCH --chdir={chdir}");
        let mut lines: Vec<&str> = base.lines().collect();
        lines.insert(1, &chdir_directive);
        let expected = lines.join("\n");
        let actual = std::fs::read_to_string(out_dir.join(format!("{rule_name}.sh"))).unwrap();
        assert_eq!(actual, expected, "render diverged for rule '{rule_name}'");
        compared += 1;
    }
    assert!(compared > 0);
}

#[tokio::test]
async fn cluster_logs_command_uses_mock_sacct() {
    // `cluster logs` is the last CLI stub resolved (issue #67 §4): SLURM
    // fetches the accounting record through the shared ExecutorBackend::logs
    // implementation — the same path BackendDriver uses.
    let state = tempfile::tempdir().unwrap();
    let job_dir = state.path().join("jobs/12345");
    std::fs::create_dir_all(&job_dir).unwrap();
    std::fs::write(job_dir.join("state"), "COMPLETED").unwrap();
    std::fs::write(job_dir.join("exit_code"), "0:0").unwrap();

    let mut cmd = std::process::Command::new(workspace_bin("oxo-flow"));
    let out = cmd
        .args(["cluster", "logs", "--backend", "slurm", "12345"])
        .env("MOCK_SCHEDULER_DIR", state.path())
        .env(
            "PATH",
            format!(
                "{}:{}",
                fixtures_dir().display(),
                std::env::var("PATH").unwrap()
            ),
        )
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(out.status.success(), "cluster logs failed: {stderr}");
    assert!(
        stdout.contains("12345|COMPLETED|0:0|00:00:05"),
        "got: {stdout}"
    );
    // The step row is where a real sacct puts MaxRSS / TotalCPU.
    assert!(
        stdout.contains("12345.batch|COMPLETED|0:0|00:00:05|1234K|00:00:04"),
        "got: {stdout}"
    );

    // Unknown backend falls back to SLURM; missing scheduler binary fails
    // loudly instead of silently succeeding.
    let mut bad = std::process::Command::new(workspace_bin("oxo-flow"));
    let out = bad
        .args(["cluster", "logs", "--backend", "slurm", "99999"])
        .env("MOCK_SCHEDULER_DIR", state.path())
        .env(
            "PATH",
            format!(
                "{}:{}",
                fixtures_dir().display(),
                std::env::var("PATH").unwrap()
            ),
        )
        .output()
        .unwrap();
    assert!(!out.status.success(), "unknown job id should fail");
}

#[test]
fn driver_settles_jobs_that_vanish_from_the_live_queue_via_accounting() {
    // Real SLURM drops finished jobs from squeue immediately; the driver
    // must settle them from sacct instead of polling forever. The mock
    // replicates that with MOCK_SQUEUE_HIDE_TERMINAL=1 (terminal states are
    // omitted from squeue but still readable from sacct).
    let dir = tempfile::tempdir().unwrap();
    write_workflow(dir.path());
    let state = tempfile::tempdir().unwrap();
    let run_dir = tempfile::tempdir().unwrap();
    let to_run: HashSet<String> = {
        let wf = dir.path().join("wf.oxoflow");
        let mut config = WorkflowConfig::from_file(&wf).unwrap();
        config.apply_defaults();
        config.expand_wildcards().unwrap();
        let dag = WorkflowDag::from_rules(&config.rules).unwrap();
        dag.execution_order().unwrap().into_iter().collect()
    };
    let records = run_driver_with_env(
        dir.path(),
        run_dir.path(),
        state.path(),
        &to_run,
        DriverConfig {
            max_submitted: 4,
            max_array_size: 1001,
            no_arrays: false,
            poll_interval: std::time::Duration::from_millis(50),
            poll_timeout: Some(std::time::Duration::from_secs(30)),
            unknown_settle_grace: std::time::Duration::from_secs(90),
        },
        &[("MOCK_SQUEUE_HIDE_TERMINAL", "1")],
    )
    .expect("driver must settle vanished jobs from sacct");
    assert!(
        records.iter().all(|r| r.status == JobStatus::Success),
        "{records:?}"
    );
    assert_eq!(records.len(), 6, "align+stats × 3 samples");
}

#[test]
fn failed_cluster_job_records_the_scheduler_exit_code() {
    // A cluster failure used to report exit code 1 whatever the rule did, so
    // a run that died at `exit 7` was indistinguishable from one that died at
    // `exit 1`. The accounting store knows the real code — read it.
    let dir = tempfile::tempdir().unwrap();
    write_failing_workflow(dir.path(), 7);
    let state = tempfile::tempdir().unwrap();
    let run_dir = tempfile::tempdir().unwrap();

    let records = run_driver(
        dir.path(),
        run_dir.path(),
        state.path(),
        &all_rules(dir.path()),
        fast_driver_config(),
    )
    .expect("driver runs to completion even when every job fails");

    assert!(
        records.iter().all(|r| r.status == JobStatus::Failed),
        "{records:?}"
    );
    for r in &records {
        assert_eq!(r.exit_code, Some(7), "{}: {:?}", r.rule, r.exit_code);
    }
}

#[test]
fn run_directory_records_what_each_job_cost() {
    // Everything a failure report needs has to survive in the run directory:
    // when each event happened, how long the job ran, how much memory it
    // took, and how much of the elapsed time was queue wait rather than work.
    let dir = tempfile::tempdir().unwrap();
    write_workflow(dir.path());
    let state = tempfile::tempdir().unwrap();
    let run_dir = tempfile::tempdir().unwrap();

    let records = run_driver_with_env(
        dir.path(),
        run_dir.path(),
        state.path(),
        &all_rules(dir.path()),
        fast_driver_config(),
        // A memory figure large enough that MB rounding cannot hide a
        // parsing mistake.
        &[
            ("MOCK_SACCT_MAXRSS", "2G"),
            ("MOCK_SACCT_ELAPSED", "00:00:05"),
        ],
    )
    .expect("driver completes");

    // Records carry the measurements, not None.
    for r in &records {
        assert_eq!(r.max_rss_mb, Some(2048), "{}: {:?}", r.rule, r.max_rss_mb);
        assert_eq!(r.cpu_seconds, Some(4.0), "{}: {:?}", r.rule, r.cpu_seconds);
        // started_at is recomputed from the scheduler's Elapsed, so the
        // recorded wall time is the job's RUNTIME — not runtime plus the
        // time it sat in the queue.
        let (start, finish) = (r.started_at.unwrap(), r.finished_at.unwrap());
        let wall = (finish - start).num_seconds();
        assert!((4..=6).contains(&wall), "{}: wall {wall}s", r.rule);
    }

    // Every event line is timestamped.
    let events = std::fs::read_to_string(run_dir.path().join("events.jsonl")).unwrap();
    assert!(!events.trim().is_empty(), "no events written");
    for line in events.lines() {
        let v: serde_json::Value = serde_json::from_str(line).unwrap_or_else(|e| {
            panic!("event line is not JSON ({e}): {line}");
        });
        let ts = v["ts"].as_str().unwrap_or_else(|| panic!("no ts: {line}"));
        chrono::DateTime::parse_from_rfc3339(ts)
            .unwrap_or_else(|e| panic!("ts is not RFC 3339 ({e}): {line}"));
    }

    // status.json is self-contained: state, cost, and the queue/runtime split.
    let status: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(run_dir.path().join("jobs/align_batch_S1/status.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(status["state"], "COMPLETED");
    assert_eq!(status["exit_code"], 0);
    assert_eq!(status["elapsed_secs"], 5);
    assert_eq!(status["max_rss_mb"], 2048);
    assert!(status["submitted_at"].is_string(), "{status}");
    assert!(status["finished_at"].is_string(), "{status}");
    assert!(
        status["queue_wait_secs"].is_number(),
        "queue wait must be derivable from the run dir alone: {status}"
    );
}
