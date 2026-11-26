//! End-to-end `resume` re-attach tests (issue #1427) against the repository's
//! mock SLURM scheduler. Every test drives the real `oxo-flow` binary with
//! `PATH` pointing at `tests/fixtures/mock-scheduler`, so submission,
//! polling, accounting, cancellation, and checkpoint bookkeeping all run the
//! production code paths — no live scheduler is involved.
use oxo_flow_core::executor::checkpoint::{
    CheckpointState, ClusterJobRecord, ClusterRunRecord, OutputManifestEntry, compute_file_checksum,
};
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

/// Two independent rules: enough to record a mixed-backend pair or an array's
/// two elements without any dependency between them.
const TWO_RULES: &str = r#"
[workflow]
name = "reattach-two"

[[rules]]
name = "r0"
output = ["out0.txt"]
shell = "echo ok > out0.txt"

[[rules]]
name = "r1"
output = ["out1.txt"]
shell = "echo ok > out1.txt"
"#;

/// `b` consumes `a`'s output, so re-running `b` while `a`'s recorded job is
/// still live is exactly the concurrent-writer hazard re-attach must avoid.
const DEPENDENT_PAIR: &str = r#"
[workflow]
name = "rerun-crash"

[[rules]]
name = "a"
output = ["a.txt"]
shell = "echo a > a.txt"

[[rules]]
name = "b"
input = ["a.txt"]
output = ["b.txt"]
shell = "echo b > b.txt"
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

/// A recorded output manifest that still matches the settled outputs
/// authorizes adoption (issue #1426) — content, not just existence.
#[test]
fn resume_adopts_a_terminal_success_whose_manifest_verifies() {
    let dir = tempfile::tempdir().unwrap();
    let wf = dir.path().join("w.oxoflow");
    std::fs::write(&wf, ONE_STEP).unwrap();
    write_profile(dir.path());
    let state = dir.path().join("state");
    seed_job(&state, "1", "COMPLETED", 0);
    std::fs::write(dir.path().join("out.txt"), "ok").unwrap();
    let mut ck = base_checkpoint(&wf, dir.path());
    ck.record_output_manifest(
        "work",
        vec![OutputManifestEntry {
            path: "out.txt".to_string(),
            size: 2,
            sha256: compute_file_checksum(&dir.path().join("out.txt")).ok(),
        }],
    );
    record_job(&mut ck, "work", "1", None);
    let cp = save_checkpoint(&ck, dir.path());

    let (ok, stdout, stderr) = run_resume(
        dir.path(),
        &["resume", cp.to_str().unwrap(), "--json"],
        Some(&state),
    );
    assert!(ok, "adoption succeeds; stderr:\n{stderr}");
    let doc = json(&stdout);
    assert_eq!(doc["adopted"], 1, "the verified job was adopted: {doc}");
    assert_eq!(
        job_ids(&state),
        vec!["1".to_string()],
        "no resubmission for verified outputs"
    );
}

/// A recorded manifest that no longer matches refuses adoption: the rule is
/// re-run instead of being marked complete with a damaged artifact.
#[test]
fn resume_does_not_adopt_a_terminal_success_whose_manifest_fails() {
    let dir = tempfile::tempdir().unwrap();
    let wf = dir.path().join("w.oxoflow");
    std::fs::write(&wf, ONE_STEP).unwrap();
    write_profile(dir.path());
    let state = dir.path().join("state");
    seed_job(&state, "1", "COMPLETED", 0);
    std::fs::write(dir.path().join("out.txt"), "ok").unwrap();
    let mut ck = base_checkpoint(&wf, dir.path());
    // The recorded size no longer matches the file (truncated artifact).
    ck.record_output_manifest(
        "work",
        vec![OutputManifestEntry {
            path: "out.txt".to_string(),
            size: 99,
            sha256: compute_file_checksum(&dir.path().join("out.txt")).ok(),
        }],
    );
    record_job(&mut ck, "work", "1", None);
    let cp = save_checkpoint(&ck, dir.path());

    let (ok, stdout, stderr) = run_resume(
        dir.path(),
        &["resume", cp.to_str().unwrap(), "--json"],
        Some(&state),
    );
    assert!(ok, "the rerun succeeds; stderr:\n{stderr}");
    let doc = json(&stdout);
    assert_eq!(
        doc["adopted"], 0,
        "a failing manifest is not adopted: {doc}"
    );
    assert_eq!(
        job_ids(&state),
        vec!["1".to_string(), "2".to_string()],
        "the rule was resubmitted instead of adopted"
    );
    assert!(
        stderr.contains("strict output verification failed"),
        "the refusal must be narrated:\n{stderr}"
    );
    let after = CheckpointState::load_from_file(&cp).unwrap();
    assert!(
        after.completed_rules.contains("work"),
        "the rerun completed"
    );
}

/// MEDIUM-1: `--trust-mtime` reaches cluster adoption. An already-successful
/// scheduler job whose output manifest is unverifiable (over-cap/directory)
/// must be adopted instead of resubmitted, matching the flag's documented
/// size+mtime verdict.
#[test]
fn resume_trust_mtime_adopts_an_unverified_terminal_success() {
    let dir = tempfile::tempdir().unwrap();
    let wf = dir.path().join("w.oxoflow");
    std::fs::write(&wf, ONE_STEP).unwrap();
    write_profile(dir.path());
    let state = dir.path().join("state");
    seed_job(&state, "1", "COMPLETED", 0);
    std::fs::write(dir.path().join("out.txt"), "ok").unwrap();
    let mut ck = base_checkpoint(&wf, dir.path());
    // A size-only (unverified) entry — what an over-cap or non-regular
    // output records.
    ck.record_output_manifest(
        "work",
        vec![OutputManifestEntry {
            path: "out.txt".to_string(),
            size: 2,
            sha256: None,
        }],
    );
    record_job(&mut ck, "work", "1", None);
    let cp = save_checkpoint(&ck, dir.path());

    let (ok, stdout, stderr) = run_resume(
        dir.path(),
        &["resume", cp.to_str().unwrap(), "--trust-mtime", "--json"],
        Some(&state),
    );
    assert!(ok, "trust-mtime adoption succeeds; stderr:\n{stderr}");
    let doc = json(&stdout);
    assert_eq!(
        doc["adopted"], 1,
        "--trust-mtime must adopt the completed job: {doc}"
    );
    assert_eq!(
        job_ids(&state),
        vec!["1".to_string()],
        "no compute was resubmitted under --trust-mtime"
    );
    let after = CheckpointState::load_from_file(&cp).unwrap();
    assert!(after.completed_rules.contains("work"));
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
    // Strict resume (issue #1426) verifies the recorded output manifest: this
    // pre-completed rule's artifact is intact, so only the pending rule is
    // submitted and the route assertion below stays about routing.
    ck.record_output_manifest(
        "done",
        vec![OutputManifestEntry {
            path: "done.txt".to_string(),
            size: 4,
            sha256: compute_file_checksum(&dir.path().join("done.txt")).ok(),
        }],
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

/// Issue #1427 review I2: a finished SLURM array element is accounted for by
/// its element id (`7_1`), not its array parent (`7`).
#[test]
fn resume_adopts_array_elements_by_element_id() {
    let dir = tempfile::tempdir().unwrap();
    let wf = dir.path().join("w.oxoflow");
    std::fs::write(&wf, TWO_RULES).unwrap();
    write_profile(dir.path());
    let state = dir.path().join("state");
    seed_job(&state, "7_1", "COMPLETED", 0);
    seed_job(&state, "7_2", "COMPLETED", 0);
    std::fs::write(dir.path().join("out0.txt"), "ok").unwrap();
    std::fs::write(dir.path().join("out1.txt"), "ok").unwrap();
    let mut ck = base_checkpoint(&wf, dir.path());
    record_job(&mut ck, "r0", "7_1", Some("7"));
    record_job(&mut ck, "r1", "7_2", Some("7"));
    let cp = save_checkpoint(&ck, dir.path());

    let (ok, stdout, stderr) = run_resume(
        dir.path(),
        &["resume", cp.to_str().unwrap(), "--json"],
        Some(&state),
    );
    assert!(
        ok,
        "array elements must be adopted by element id; stderr:\n{stderr}"
    );
    let doc = json(&stdout);
    assert_eq!(doc["adopted"], 2, "{doc}");
    let after = CheckpointState::load_from_file(&cp).unwrap();
    assert!(after.completed_rules.contains("r0"));
    assert!(after.completed_rules.contains("r1"));
    assert!(after.cluster_jobs.is_empty());
    assert_eq!(
        after.rule_runs["r0"].exit_code,
        Some(0),
        "the element's accounting row supplies its own exit code"
    );
    assert_eq!(
        job_ids(&state),
        vec!["7_1".to_string(), "7_2".to_string()],
        "adoption must not resubmit the array"
    );
}

/// Issue #1427 review I1: a checkpoint whose jobs span two schedulers polls
/// and cancels each group through the backend that owns it. The PBS probe
/// must never see the SLURM id (and vice versa).
#[test]
fn resume_reconciles_a_mixed_backend_checkpoint_through_each_scheduler() {
    let dir = tempfile::tempdir().unwrap();
    let wf = dir.path().join("w.oxoflow");
    std::fs::write(&wf, TWO_RULES).unwrap();
    write_profile(dir.path());
    let state = dir.path().join("state");
    seed_job(&state, "1", "RUNNING", 0);

    // Minimal PBS mock: `qstat -x <id>` reports a running row; `qdel <id>`
    // records the id it was asked to cancel.
    let pbs_bin = dir.path().join("pbs-bin");
    std::fs::create_dir_all(&pbs_bin).unwrap();
    let pbs_state = dir.path().join("pbs-state");
    std::fs::create_dir_all(&pbs_state).unwrap();
    std::fs::write(
        pbs_bin.join("qstat"),
        "#!/bin/sh\nfor a in \"$@\"; do\n  case \"$a\" in -x) ;; *) echo \"$a rule user 00:00 R batch\" ;; esac\ndone\n",
    )
    .unwrap();
    std::fs::write(
        pbs_bin.join("qdel"),
        "#!/bin/sh\nprintf '%s\\n' \"$@\" >> \"${MOCK_PBS_DIR:?}/qdel_calls\"\n",
    )
    .unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        for program in ["qstat", "qdel"] {
            let path = pbs_bin.join(program);
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
    }

    let mut ck = base_checkpoint(&wf, dir.path());
    record_job(&mut ck, "r0", "1", None);
    record_job(&mut ck, "r1", "2", None);
    ck.cluster_jobs.get_mut("r1").unwrap().backend = "pbs".to_string();
    let cp = save_checkpoint(&ck, dir.path());

    let path = format!(
        "{}:{}:{}",
        pbs_bin.display(),
        mock_scheduler_dir().display(),
        std::env::var("PATH").unwrap_or_default()
    );
    let out = Command::new(oxo_bin())
        .args(["resume", cp.to_str().unwrap(), "--abandon-jobs", "--json"])
        .current_dir(dir.path())
        .env("PATH", path)
        .env("MOCK_SCHEDULER_DIR", &state)
        .env("MOCK_PBS_DIR", &pbs_state)
        .env("MOCK_SQUEUE_HIDE_TERMINAL", "1")
        .output()
        .unwrap();
    let ok = out.status.success();
    let stdout = String::from_utf8_lossy(&out.stdout).to_string();
    let stderr = String::from_utf8_lossy(&out.stderr).to_string();
    assert!(ok, "mixed-backend abandon succeeds; stderr:\n{stderr}");
    let doc = json(&stdout);
    assert_eq!(doc["cancelled_jobs"], 2, "{doc}");
    assert_eq!(
        std::fs::read_to_string(pbs_state.join("qdel_calls")).unwrap(),
        "2\n",
        "PBS may only be asked to cancel the PBS id"
    );
    assert_eq!(job_state(&state, "1"), "CANCELLED");
    assert!(dir.path().join("out0.txt").exists());
    assert!(dir.path().join("out1.txt").exists());
    assert!(
        stderr.contains("each group is reconciled through the scheduler that owns it"),
        "the mixed routes must be narrated:\n{stderr}"
    );
    let after = CheckpointState::load_from_file(&cp).unwrap();
    assert!(after.cluster_jobs.is_empty());
}

/// `--abandon-jobs` without a usable profile is explicit: cancel through the
/// recorded backend, then run LOCALLY (issue #1427 review, minor).
#[test]
fn resume_abandon_without_a_profile_cancels_then_runs_locally() {
    let dir = tempfile::tempdir().unwrap();
    let wf = dir.path().join("w.oxoflow");
    std::fs::write(&wf, ONE_STEP).unwrap();
    // No profiles/ directory: the recorded profile name resolves to nothing.
    let state = dir.path().join("state");
    seed_job(&state, "1", "RUNNING", 0);
    let mut ck = base_checkpoint(&wf, dir.path());
    record_job(&mut ck, "work", "1", None);
    let cp = save_checkpoint(&ck, dir.path());

    let (ok, stdout, stderr) = run_resume(
        dir.path(),
        &["resume", cp.to_str().unwrap(), "--abandon-jobs", "--json"],
        Some(&state),
    );
    assert!(ok, "abandon + local rerun succeeds; stderr:\n{stderr}");
    let doc = json(&stdout);
    assert_eq!(doc["cancelled_jobs"], 1, "{doc}");
    assert_eq!(job_state(&state, "1"), "CANCELLED");
    assert!(
        stderr.contains("run LOCALLY"),
        "the local fallback must be explicit:\n{stderr}"
    );
    assert!(dir.path().join("out.txt").exists(), "the rule ran locally");
    assert_eq!(
        job_ids(&state),
        vec!["1".to_string()],
        "a local rerun submits nothing to the scheduler"
    );
    let after = CheckpointState::load_from_file(&cp).unwrap();
    assert!(after.completed_rules.contains("work"));
}

/// Issue #1427 re-review (Important): a `run --rerun` submission keeps the
/// rule in `completed_rules` while its NEW job is live. Resume must NOT prune
/// that job — and must not submit its dependent while job 99 is still running.
#[test]
fn resume_does_not_prune_a_live_rerun_job() {
    let dir = tempfile::tempdir().unwrap();
    let wf = dir.path().join("w.oxoflow");
    std::fs::write(&wf, DEPENDENT_PAIR).unwrap();
    write_profile(dir.path());
    let state = dir.path().join("state");
    seed_job(&state, "99", "RUNNING", 0);
    std::fs::write(dir.path().join("a.txt"), "a").unwrap();
    let mut ck = base_checkpoint(&wf, dir.path());
    // The exact shape a `run --profile slurm --rerun` crash leaves: the rule
    // is completed AND running, with the rerun's job id recorded.
    ck.mark_completed_quiet("a");
    ck.mark_running("a");
    record_job(&mut ck, "a", "99", None);
    let cp = save_checkpoint(&ck, dir.path());

    let path = format!(
        "{}:{}",
        mock_scheduler_dir().display(),
        std::env::var("PATH").unwrap_or_default()
    );
    let mut child = Command::new(oxo_bin())
        .args(["resume", cp.to_str().unwrap(), "--json"])
        .current_dir(dir.path())
        .env("PATH", path)
        .env("MOCK_SCHEDULER_DIR", &state)
        .env("MOCK_SQUEUE_HIDE_TERMINAL", "1")
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .unwrap();

    // The resume must park on job 99: no dependent submission, no prune.
    std::thread::sleep(std::time::Duration::from_secs(5));
    assert_eq!(
        job_ids(&state),
        vec!["99".to_string()],
        "no rule may be resubmitted while job 99 is live"
    );
    assert!(
        !dir.path().join("b.txt").exists(),
        "the dependent must not run while its producer's job is live"
    );
    child.kill().unwrap();
    let out = child.wait_with_output().unwrap();
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        !stderr.contains("already complete"),
        "a live rerun job must never be pruned:\n{stderr}"
    );
    assert!(
        stderr.contains("waiting for 1 of 1 recorded scheduler job(s)"),
        "resume must wait on the recorded job:\n{stderr}"
    );
    let after = CheckpointState::load_from_file(&cp).unwrap();
    assert!(
        after.cluster_jobs.contains_key("a"),
        "the live job id must remain recorded for the next resume"
    );
}

/// A checkpoint with recorded job ids but no `cluster_run` identity can still
/// reconcile each job through its own backend; only queue/account are unknown.
#[test]
fn resume_abandons_without_a_cluster_run_identity() {
    let dir = tempfile::tempdir().unwrap();
    let wf = dir.path().join("w.oxoflow");
    std::fs::write(&wf, ONE_STEP).unwrap();
    let state = dir.path().join("state");
    seed_job(&state, "1", "RUNNING", 0);
    let mut ck = base_checkpoint(&wf, dir.path());
    ck.set_cluster_run(None);
    record_job(&mut ck, "work", "1", None);
    let cp = save_checkpoint(&ck, dir.path());

    let (ok, stdout, stderr) = run_resume(
        dir.path(),
        &["resume", cp.to_str().unwrap(), "--abandon-jobs", "--json"],
        Some(&state),
    );
    assert!(ok, "per-job backend recovery succeeds; stderr:\n{stderr}");
    let doc = json(&stdout);
    assert_eq!(doc["cancelled_jobs"], 1, "{doc}");
    assert_eq!(job_state(&state, "1"), "CANCELLED");
    assert!(
        stderr.contains("no cluster run identity"),
        "the missing route identity must be narrated:\n{stderr}"
    );
    assert!(dir.path().join("out.txt").exists(), "the local rerun ran");
    let after = CheckpointState::load_from_file(&cp).unwrap();
    assert!(after.completed_rules.contains("work"));
}
