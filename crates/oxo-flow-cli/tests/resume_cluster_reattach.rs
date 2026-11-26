//! End-to-end `resume` re-attach tests (issue #1427) against the repository's
//! mock SLURM scheduler. Every test drives the real `oxo-flow` binary with
//! `PATH` pointing at `tests/fixtures/mock-scheduler`, so submission,
//! polling, accounting, cancellation, and checkpoint bookkeeping all run the
//! production code paths — no live scheduler is involved.
use oxo_flow_core::executor::checkpoint::{CheckpointState, ClusterJobRecord, ClusterRunRecord};
use std::path::{Path, PathBuf};
use std::process::Command;

fn oxo_bin() -> &'static str {
    env!("CARGO_BIN_EXE_oxo-flow")
}

fn mock_scheduler_dir() -> PathBuf {
    // <repo>/crates/oxo-flow-cli → <repo>/tests/fixtures/mock-scheduler
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .join("tests/fixtures/mock-scheduler")
}

/// A two-rule workflow: `done` is pre-completed by the checkpoint, `next`
/// consumes its output so a cluster run submits exactly one job.
const TWO_STEP: &str = r#"
[workflow]
name = "reattach-demo"

[[rules]]
name = "done"
output = ["done.txt"]
shell = "echo done > done.txt"

[[rules]]
name = "next"
input = ["done.txt"]
output = ["out.txt"]
shell = "echo ok > out.txt"
"#;

/// A one-rule workflow whose only rule is recorded as an unsettled cluster
/// job by the checkpoint.
const ONE_STEP: &str = r#"
[workflow]
name = "reattach-single"

[[rules]]
name = "work"
output = ["out.txt"]
shell = "echo ok > out.txt"
"#;

fn write_profile(dir: &Path) {
    std::fs::create_dir_all(dir.join("profiles")).unwrap();
    std::fs::write(
        dir.join("profiles/slurm.toml"),
        "[cluster]\nbackend = \"slurm\"\npoll_interval = \"1s\"\n",
    )
    .unwrap();
}

fn base_checkpoint(workflow: &Path, workdir: &Path) -> CheckpointState {
    let mut ck = CheckpointState::new();
    ck.set_workflow_path(workflow);
    ck.set_workdir(workdir);
    ck.set_cluster_run(Some(ClusterRunRecord {
        profile: "slurm".to_string(),
        backend: "slurm".to_string(),
        queue: None,
        account: None,
        run_dir: Some(workdir.join(".oxo-flow/runs/test").display().to_string()),
    }));
    ck
}

fn record_job(ck: &mut CheckpointState, rule: &str, job_id: &str, array_base: Option<&str>) {
    ck.mark_running(rule);
    ck.record_cluster_job(
        rule,
        ClusterJobRecord {
            job_id: job_id.to_string(),
            array_base: array_base.map(str::to_string),
            backend: "slurm".to_string(),
            submitted_at: chrono::Utc::now(),
        },
    );
}

fn save_checkpoint(ck: &CheckpointState, dir: &Path) -> PathBuf {
    let path = dir.join("checkpoint.json");
    ck.save_to_file(&path).unwrap();
    path
}

/// Seed the mock scheduler with one finished/live job.
fn seed_job(state: &Path, id: &str, status: &str, exit_code: i32) {
    let dir = state.join("jobs").join(id);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("state"), status).unwrap();
    std::fs::write(dir.join("exit_code"), exit_code.to_string()).unwrap();
}

fn job_ids(state: &Path) -> Vec<String> {
    let mut ids: Vec<String> = std::fs::read_dir(state.join("jobs"))
        .map(|entries| {
            entries
                .filter_map(|e| e.ok())
                .filter(|e| e.file_type().map(|t| t.is_dir()).unwrap_or(false))
                .map(|e| e.file_name().to_string_lossy().into_owned())
                .collect()
        })
        .unwrap_or_default();
    ids.sort();
    ids
}

fn job_state(state: &Path, id: &str) -> String {
    std::fs::read_to_string(state.join("jobs").join(id).join("state"))
        .unwrap_or_default()
        .trim()
        .to_string()
}

/// Run `oxo-flow <args>` in `dir` with the mock scheduler on `PATH`.
///
/// `state` is the mock scheduler's job store; `None` leaves
/// `MOCK_SCHEDULER_DIR` unset (every scheduler invocation fails, the
/// "scheduler unreachable" case). Terminal jobs are hidden from the live
/// queue so settlement exercises the accounting path, exactly like a real
/// scheduler.
fn run_resume(dir: &Path, args: &[&str], state: Option<&Path>) -> (bool, String, String) {
    let path = format!(
        "{}:{}",
        mock_scheduler_dir().display(),
        std::env::var("PATH").unwrap_or_default()
    );
    let mut cmd = Command::new(oxo_bin());
    cmd.args(args)
        .current_dir(dir)
        .env("PATH", path)
        .env("MOCK_SQUEUE_HIDE_TERMINAL", "1")
        .env_remove("MOCK_SCHEDULER_DIR");
    if let Some(state) = state {
        cmd.env("MOCK_SCHEDULER_DIR", state);
    }
    let out = cmd.output().unwrap();
    (
        out.status.success(),
        String::from_utf8_lossy(&out.stdout).to_string(),
        String::from_utf8_lossy(&out.stderr).to_string(),
    )
}

fn json(stdout: &str) -> serde_json::Value {
    serde_json::from_str(stdout).unwrap_or_else(|e| panic!("stdout is not JSON ({e}):\n{stdout}"))
}

/// No profile is recorded and none can be found: fail closed BEFORE the
/// running set is cleared or any rule executes locally, naming the recorded
/// job and both escape hatches.
#[test]
fn resume_without_a_profile_fails_closed_and_runs_nothing() {
    let dir = tempfile::tempdir().unwrap();
    let wf = dir.path().join("w.oxoflow");
    std::fs::write(&wf, ONE_STEP).unwrap();
    let mut ck = base_checkpoint(&wf, dir.path());
    record_job(&mut ck, "work", "101", None);
    // The checkpoint is readable, but no profiles/ directory exists.
    let cp = save_checkpoint(&ck, dir.path());
    let before = std::fs::read_to_string(&cp).unwrap();

    let (ok, stdout, stderr) = run_resume(
        dir.path(),
        &["resume", cp.to_str().unwrap(), "--json"],
        Some(&dir.path().join("state")),
    );
    assert!(!ok, "must fail closed; stderr:\n{stderr}");
    let doc = json(&stdout);
    let message = doc["errors"][0]["message"].as_str().unwrap_or_default();
    assert!(message.contains("101"), "names the job:\n{message}");
    assert!(message.contains("--profile"), "{message}");
    assert!(message.contains("--abandon-jobs"), "{message}");
    assert!(
        !dir.path().join("out.txt").exists(),
        "no rule may execute locally"
    );
    assert_eq!(
        std::fs::read_to_string(&cp).unwrap(),
        before,
        "the fail-closed path must not rewrite the checkpoint"
    );
}

/// The recorded profile is the default: a terminal-success job is adopted
/// (outputs verified) and the rule is NOT resubmitted.
#[test]
fn resume_adopts_a_terminal_success_without_resubmitting() {
    let dir = tempfile::tempdir().unwrap();
    let wf = dir.path().join("w.oxoflow");
    std::fs::write(&wf, ONE_STEP).unwrap();
    write_profile(dir.path());
    // The job finished and left the live queue; accounting knows it. Its
    // declared output exists.
    let state = dir.path().join("state");
    seed_job(&state, "1", "COMPLETED", 0);
    std::fs::write(dir.path().join("out.txt"), "ok").unwrap();
    let mut ck = base_checkpoint(&wf, dir.path());
    record_job(&mut ck, "work", "1", None);
    let cp = save_checkpoint(&ck, dir.path());

    // No --profile: the checkpoint's recorded profile must be used.
    let (ok, stdout, stderr) = run_resume(
        dir.path(),
        &["resume", cp.to_str().unwrap(), "--json"],
        Some(&state),
    );
    assert!(ok, "adoption succeeds; stderr:\n{stderr}");
    let doc = json(&stdout);
    assert_eq!(doc["status"], "completed");
    assert_eq!(doc["adopted"], 1, "the job was adopted: {doc}");
    assert_eq!(doc["cancelled_jobs"], 0);

    let after = CheckpointState::load_from_file(&cp).unwrap();
    assert!(after.completed_rules.contains("work"));
    assert!(
        after.cluster_jobs.is_empty(),
        "the settled job id is cleared"
    );
    assert_eq!(job_ids(&state), vec!["1".to_string()], "no resubmission");
}

/// A terminal failure is recorded and re-run through the cluster: the
/// scheduler receives exactly one new submission and the rule completes.
#[test]
fn resume_reruns_a_terminal_failure() {
    let dir = tempfile::tempdir().unwrap();
    let wf = dir.path().join("w.oxoflow");
    std::fs::write(&wf, ONE_STEP).unwrap();
    write_profile(dir.path());
    let state = dir.path().join("state");
    seed_job(&state, "1", "FAILED", 1);
    let mut ck = base_checkpoint(&wf, dir.path());
    record_job(&mut ck, "work", "1", None);
    let cp = save_checkpoint(&ck, dir.path());

    let (ok, stdout, stderr) = run_resume(
        dir.path(),
        &["resume", cp.to_str().unwrap(), "--json"],
        Some(&state),
    );
    assert!(ok, "the rerun succeeds; stderr:\n{stderr}");
    let doc = json(&stdout);
    assert_eq!(doc["status"], "completed");
    assert_eq!(doc["adopted"], 0);
    assert!(
        dir.path().join("out.txt").exists(),
        "the rule re-ran on the cluster"
    );
    let after = CheckpointState::load_from_file(&cp).unwrap();
    assert!(after.completed_rules.contains("work"));
    assert!(after.cluster_jobs.is_empty());
    assert_eq!(
        job_ids(&state),
        vec!["1".to_string(), "2".to_string()],
        "exactly one new submission"
    );
}

/// `--abandon-jobs` cancels a live job through the same backend/queue
/// mapping, then reruns its rule.
#[test]
fn resume_abandon_jobs_cancels_then_reruns() {
    let dir = tempfile::tempdir().unwrap();
    let wf = dir.path().join("w.oxoflow");
    std::fs::write(&wf, ONE_STEP).unwrap();
    write_profile(dir.path());
    let state = dir.path().join("state");
    seed_job(&state, "1", "RUNNING", 0);
    let mut ck = base_checkpoint(&wf, dir.path());
    record_job(&mut ck, "work", "1", None);
    let cp = save_checkpoint(&ck, dir.path());

    let (ok, stdout, stderr) = run_resume(
        dir.path(),
        &[
            "resume",
            cp.to_str().unwrap(),
            "--abandon-jobs",
            "--profile",
            "slurm",
            "--json",
        ],
        Some(&state),
    );
    assert!(ok, "abandon + rerun succeeds; stderr:\n{stderr}");
    let doc = json(&stdout);
    assert_eq!(doc["status"], "completed");
    assert_eq!(
        doc["cancelled_jobs"], 1,
        "the live job was cancelled: {doc}"
    );
    assert_eq!(job_state(&state, "1"), "CANCELLED");
    assert!(dir.path().join("out.txt").exists(), "the rule re-ran");
    assert_eq!(
        job_ids(&state),
        vec!["1".to_string(), "2".to_string()],
        "one new submission after the cancel"
    );
}

/// A scheduler that cannot confirm the recorded job (here: every scheduler
/// command fails, so the job is invisible to both the live queue and
/// accounting) fails closed: no rerun, no checkpoint rewrite.
#[test]
fn resume_fails_closed_when_the_scheduler_cannot_confirm_the_job() {
    let dir = tempfile::tempdir().unwrap();
    let wf = dir.path().join("w.oxoflow");
    std::fs::write(&wf, ONE_STEP).unwrap();
    write_profile(dir.path());
    let mut ck = base_checkpoint(&wf, dir.path());
    record_job(&mut ck, "work", "1", None);
    let cp = save_checkpoint(&ck, dir.path());
    let before = std::fs::read_to_string(&cp).unwrap();

    // MOCK_SCHEDULER_DIR unset: every mock scheduler command fails, so the
    // job's state is unknowable. (The SLURM adapter deliberately reads a
    // failed squeue as "absent from the live queue" — a finished job makes
    // real squeue exit non-zero — so the fail-closed verdict is the same one
    // an unreadable accounting store produces.)
    let (ok, stdout, stderr) = run_resume(
        dir.path(),
        &["resume", cp.to_str().unwrap(), "--json"],
        None,
    );
    assert!(!ok, "must fail closed; stderr:\n{stderr}");
    let doc = json(&stdout);
    let message = doc["errors"][0]["message"].as_str().unwrap_or_default();
    assert!(
        message.contains("cannot confirm the state"),
        "the unknown-state failure must surface:\n{message}"
    );
    assert!(message.contains("--abandon-jobs"), "{message}");
    assert!(
        !dir.path().join("out.txt").exists(),
        "nothing may run while a job's state is unknown"
    );
    assert_eq!(
        std::fs::read_to_string(&cp).unwrap(),
        before,
        "the checkpoint must be untouched"
    );
}

/// A cluster checkpoint with no unsettled job still resumes through its
/// recorded profile — the `profile: None` fallback that used to execute the
/// remaining rules locally.
#[test]
fn resume_without_a_profile_keeps_the_recorded_cluster_route() {
    let dir = tempfile::tempdir().unwrap();
    let wf = dir.path().join("w.oxoflow");
    std::fs::write(&wf, TWO_STEP).unwrap();
    write_profile(dir.path());
    std::fs::write(dir.path().join("done.txt"), "done").unwrap();
    let state = dir.path().join("state");
    std::fs::create_dir_all(state.join("jobs")).unwrap();
    let mut ck = base_checkpoint(&wf, dir.path());
    ck.mark_completed(
        "done",
        oxo_flow_core::executor::checkpoint::BenchmarkRecord {
            rule: "done".to_string(),
            wall_time_secs: 0.1,
            max_memory_mb: None,
            memory_limit_mb: None,
            cpu_seconds: None,
            retries: 0,
            recorded_as: None,
        },
    );
    let cp = save_checkpoint(&ck, dir.path());

    let (ok, stdout, stderr) = run_resume(
        dir.path(),
        &["resume", cp.to_str().unwrap(), "--json"],
        Some(&state),
    );
    assert!(ok, "cluster resume succeeds; stderr:\n{stderr}");
    let doc = json(&stdout);
    assert_eq!(doc["status"], "completed");
    assert!(
        stderr.contains("recorded cluster profile 'slurm'"),
        "the recorded route must be narrated:\n{stderr}"
    );
    assert!(dir.path().join("out.txt").exists(), "the pending rule ran");
    // A scheduler submission proves the cluster path was taken — the local
    // executor would never create a mock job dir.
    assert_eq!(
        job_ids(&state).len(),
        1,
        "the pending rule was submitted to the scheduler, not run locally"
    );
}
