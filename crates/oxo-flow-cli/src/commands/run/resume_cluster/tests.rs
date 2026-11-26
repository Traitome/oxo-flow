use super::probe::ProbeFuture;
use super::*;
use oxo_flow_core::executor::checkpoint::{ClusterJobRecord, ClusterRunRecord};
use std::sync::Mutex;

fn route() -> SchedulerRoute {
    SchedulerRoute::new("slurm", None)
}

fn submitted_at() -> chrono::DateTime<chrono::Utc> {
    chrono::DateTime::parse_from_rfc3339("2026-01-01T00:00:00Z")
        .unwrap()
        .with_timezone(&chrono::Utc)
}

fn checkpoint_with_jobs(jobs: &[(&str, &str, Option<&str>)]) -> CheckpointState {
    let mut ck = CheckpointState::new();
    ck.set_cluster_run(Some(ClusterRunRecord {
        profile: "slurm".to_string(),
        backend: "slurm".to_string(),
        queue: None,
        account: None,
        run_dir: None,
    }));
    for (rule, job_id, base) in jobs {
        ck.mark_running(rule);
        ck.record_cluster_job(
            rule,
            ClusterJobRecord {
                job_id: (*job_id).to_string(),
                array_base: base.map(str::to_string),
                backend: "slurm".to_string(),
                submitted_at: submitted_at(),
            },
        );
    }
    ck
}

fn config_with_outputs(workdir: &Path, outputs: &[&str]) -> WorkflowConfig {
    let mut toml = String::from("[workflow]\nname = \"reattach\"\n");
    for (i, output) in outputs.iter().enumerate() {
        toml.push_str(&format!(
            "[[rules]]\nname = \"r{i}\"\nshell = \"true\"\noutput = [\"{output}\"]\n"
        ));
    }
    let path = workdir.join("wf.oxoflow");
    std::fs::write(&path, toml).unwrap();
    let mut config = WorkflowConfig::from_file(&path).unwrap();
    config.apply_defaults();
    config.expand_wildcards().unwrap();
    config
}

/// Scripted fake scheduler: poll verdicts in order (the last repeats),
/// accounting records by id, and a log of every call.
#[derive(Default)]
struct FakeProbe {
    verdicts: Mutex<Vec<BackendJobStatus>>,
    accounting: Mutex<HashMap<String, TerminalRecord>>,
    poll_error: Mutex<bool>,
    polls: Mutex<Vec<Vec<String>>>,
    cancelled: Mutex<Vec<String>>,
    cancel_error: Mutex<bool>,
    direct: bool,
}

impl FakeProbe {
    fn new(direct: bool, verdicts: Vec<BackendJobStatus>) -> Self {
        Self {
            verdicts: Mutex::new(verdicts),
            direct,
            ..Self::default()
        }
    }
    fn with_accounting(self, id: &str, status: BackendJobStatus) -> Self {
        self.accounting
            .lock()
            .unwrap()
            .insert(id.to_string(), TerminalRecord::status_only(status));
        self
    }
    fn with_accounting_record(self, id: &str, record: TerminalRecord) -> Self {
        self.accounting
            .lock()
            .unwrap()
            .insert(id.to_string(), record);
        self
    }
    fn fail_poll(&self) {
        *self.poll_error.lock().unwrap() = true;
    }
    fn fail_cancel(&self) {
        *self.cancel_error.lock().unwrap() = true;
    }
    fn next_verdict(&self) -> BackendJobStatus {
        let mut verdicts = self.verdicts.lock().unwrap();
        if verdicts.len() > 1 {
            verdicts.remove(0)
        } else {
            verdicts[0]
        }
    }
    fn cancelled(&self) -> Vec<String> {
        self.cancelled.lock().unwrap().clone()
    }
    fn poll_count(&self) -> usize {
        self.polls.lock().unwrap().len()
    }
}

impl ClusterJobProbe for FakeProbe {
    fn poll_jobs<'a>(
        &'a self,
        ids: &'a [String],
    ) -> ProbeFuture<'a, Result<HashMap<String, BackendJobStatus>>> {
        self.polls.lock().unwrap().push(ids.to_vec());
        if *self.poll_error.lock().unwrap() {
            return Box::pin(async { Err(anyhow::anyhow!("injected scheduler failure")) });
        }
        let verdict = self.next_verdict();
        let statuses = ids.iter().map(|id| (id.clone(), verdict)).collect();
        Box::pin(async move { Ok(statuses) })
    }

    fn terminal_record<'a>(&'a self, id: &'a str) -> ProbeFuture<'a, Option<TerminalRecord>> {
        let rec = self.accounting.lock().unwrap().get(id).cloned();
        Box::pin(async move { rec })
    }

    fn cancel_job<'a>(&'a self, id: &'a str) -> ProbeFuture<'a, Result<()>> {
        self.cancelled.lock().unwrap().push(id.to_string());
        if *self.cancel_error.lock().unwrap() {
            return Box::pin(async { Err(anyhow::anyhow!("injected cancel failure")) });
        }
        Box::pin(async { Ok(()) })
    }

    fn polls_elements_directly(&self) -> bool {
        self.direct
    }
}

fn zero_poll() -> Duration {
    Duration::from_millis(1)
}

#[tokio::test]
async fn terminal_success_with_outputs_is_adopted_without_resubmission() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(dir.path().join("out")).unwrap();
    std::fs::write(dir.path().join("out/result.txt"), "done").unwrap();
    let config = config_with_outputs(dir.path(), &["out/result.txt"]);
    let mut ck = checkpoint_with_jobs(&[("r0", "101", None)]);
    let probe = FakeProbe::new(true, vec![BackendJobStatus::Completed])
        .with_accounting("101", BackendJobStatus::Completed);

    let queries = recorded_queries(&ck, "slurm", probe.polls_elements_directly());
    let outcome = reconcile_recorded_jobs(
        &probe,
        &mut ck,
        queries,
        &route(),
        &config,
        dir.path(),
        false,
        false,
        zero_poll(),
    )
    .await
    .unwrap();

    assert_eq!(outcome.adopted, vec!["r0".to_string()]);
    assert!(ck.completed_rules.contains("r0"));
    assert!(!ck.running_rules().contains("r0"));
    assert!(ck.cluster_jobs.is_empty(), "the settled job id is cleared");
    assert_eq!(probe.poll_count(), 1);
    assert!(probe.cancelled().is_empty());
    // A status-only accounting record carries no wall time: the adopted rule
    // must have NO benchmark rather than a synthetic 0.0 s measurement.
    assert!(
        !ck.benchmarks.contains_key("r0"),
        "unknown wall time must stay unknown"
    );
    assert_eq!(ck.rule_runs["r0"].status.as_deref(), Some("success"));
}

#[tokio::test]
async fn adopted_benchmark_records_only_a_measured_wall_time() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(dir.path().join("out")).unwrap();
    std::fs::write(dir.path().join("out/result.txt"), "done").unwrap();
    let config = config_with_outputs(dir.path(), &["out/result.txt"]);
    let mut ck = checkpoint_with_jobs(&[("r0", "101", None)]);
    let probe = FakeProbe::new(true, vec![BackendJobStatus::Completed]).with_accounting_record(
        "101",
        TerminalRecord {
            status: BackendJobStatus::Completed,
            exit_code: Some(0),
            elapsed_secs: Some(12),
            ..TerminalRecord::default()
        },
    );

    let queries = recorded_queries(&ck, "slurm", probe.polls_elements_directly());
    let outcome = reconcile_recorded_jobs(
        &probe,
        &mut ck,
        queries,
        &route(),
        &config,
        dir.path(),
        false,
        false,
        zero_poll(),
    )
    .await
    .unwrap();

    assert_eq!(outcome.adopted, vec!["r0".to_string()]);
    let benchmark = &ck.benchmarks["r0"];
    assert_eq!(benchmark.wall_time_secs, 12.0);
    assert_eq!(
        benchmark.recorded_as.as_deref(),
        Some("adopted from slurm job 101"),
        "an adopted benchmark still names its provenance"
    );
    assert_eq!(ck.rule_runs["r0"].exit_code, Some(0));
}

#[tokio::test]
async fn terminal_success_without_outputs_is_left_for_a_rerun() {
    let dir = tempfile::tempdir().unwrap();
    let config = config_with_outputs(dir.path(), &["out/result.txt"]);
    let mut ck = checkpoint_with_jobs(&[("r0", "101", None)]);
    let probe = FakeProbe::new(true, vec![BackendJobStatus::Completed]);

    let queries = recorded_queries(&ck, "slurm", probe.polls_elements_directly());
    let outcome = reconcile_recorded_jobs(
        &probe,
        &mut ck,
        queries,
        &route(),
        &config,
        dir.path(),
        false,
        false,
        zero_poll(),
    )
    .await
    .unwrap();

    assert!(outcome.adopted.is_empty());
    assert_eq!(outcome.failed, vec!["r0".to_string()]);
    assert!(!ck.completed_rules.contains("r0"));
    assert!(ck.failed_rules.contains("r0"), "the rerun is guaranteed");
    assert!(ck.cluster_jobs.is_empty());
    assert_eq!(
        ck.rule_runs["r0"].skip_reason.as_deref(),
        Some("cluster job 101 succeeded but the rule's declared outputs are missing — re-running")
    );
}

#[tokio::test]
async fn terminal_failure_is_recorded_for_the_normal_rerun() {
    let dir = tempfile::tempdir().unwrap();
    let config = config_with_outputs(dir.path(), &["out/result.txt"]);
    std::fs::create_dir_all(dir.path().join("out")).unwrap();
    // A partial output does NOT make a failed job adoptable.
    std::fs::write(dir.path().join("out/result.txt"), "partial").unwrap();
    let mut ck = checkpoint_with_jobs(&[("r0", "101", None)]);
    let probe = FakeProbe::new(true, vec![BackendJobStatus::Failed]);

    let queries = recorded_queries(&ck, "slurm", probe.polls_elements_directly());
    let outcome = reconcile_recorded_jobs(
        &probe,
        &mut ck,
        queries,
        &route(),
        &config,
        dir.path(),
        false,
        false,
        zero_poll(),
    )
    .await
    .unwrap();

    assert_eq!(outcome.failed, vec!["r0".to_string()]);
    assert!(!ck.completed_rules.contains("r0"));
    assert!(ck.failed_rules.contains("r0"));
    assert!(ck.cluster_jobs.is_empty());
}

#[tokio::test]
async fn live_job_is_waited_for_until_it_settles() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(dir.path().join("out")).unwrap();
    std::fs::write(dir.path().join("out/result.txt"), "done").unwrap();
    let config = config_with_outputs(dir.path(), &["out/result.txt"]);
    let mut ck = checkpoint_with_jobs(&[("r0", "101", None)]);
    let probe = FakeProbe::new(
        true,
        vec![BackendJobStatus::Running, BackendJobStatus::Completed],
    );

    let queries = recorded_queries(&ck, "slurm", probe.polls_elements_directly());
    let outcome = reconcile_recorded_jobs(
        &probe,
        &mut ck,
        queries,
        &route(),
        &config,
        dir.path(),
        false,
        false,
        zero_poll(),
    )
    .await
    .unwrap();

    assert_eq!(outcome.adopted, vec!["r0".to_string()]);
    assert!(
        probe.poll_count() >= 2,
        "the live job must be polled, not resubmitted"
    );
    assert!(probe.cancelled().is_empty());
}

#[tokio::test]
async fn abandon_cancels_live_jobs_then_leaves_them_for_rerun() {
    let dir = tempfile::tempdir().unwrap();
    let config = config_with_outputs(dir.path(), &["out/result.txt"]);
    let mut ck = checkpoint_with_jobs(&[("r0", "101", None)]);
    let probe = FakeProbe::new(true, vec![BackendJobStatus::Running]);

    let queries = recorded_queries(&ck, "slurm", probe.polls_elements_directly());
    let outcome = reconcile_recorded_jobs(
        &probe,
        &mut ck,
        queries,
        &route(),
        &config,
        dir.path(),
        true,
        false,
        zero_poll(),
    )
    .await
    .unwrap();

    assert_eq!(outcome.cancelled, vec!["r0".to_string()]);
    assert_eq!(probe.cancelled(), vec!["101".to_string()]);
    assert!(ck.failed_rules.contains("r0"), "abandoned rules re-run");
    assert!(ck.cluster_jobs.is_empty());
    assert_eq!(probe.poll_count(), 1, "abandon does not wait");
}

#[tokio::test]
async fn array_elements_cancel_their_base_once() {
    let dir = tempfile::tempdir().unwrap();
    let config = config_with_outputs(dir.path(), &["a.txt", "b.txt"]);
    let mut ck = checkpoint_with_jobs(&[("r0", "44_1", Some("44")), ("r1", "44_2", Some("44"))]);
    // Base-only backend: polling targets the base.
    let probe = FakeProbe::new(false, vec![BackendJobStatus::Running]);

    let queries = recorded_queries(&ck, "slurm", probe.polls_elements_directly());
    let outcome = reconcile_recorded_jobs(
        &probe,
        &mut ck,
        queries,
        &route(),
        &config,
        dir.path(),
        true,
        false,
        zero_poll(),
    )
    .await
    .unwrap();

    assert_eq!(
        probe.cancelled(),
        vec!["44".to_string()],
        "one cancel per array base"
    );
    assert_eq!(outcome.cancelled, vec!["r0".to_string(), "r1".to_string()]);
    assert_eq!(
        probe.polls.lock().unwrap()[0],
        vec!["44".to_string()],
        "a base-only backend is polled by base id"
    );
}

#[tokio::test]
async fn scheduler_poll_error_fails_closed_without_clearing_jobs() {
    let dir = tempfile::tempdir().unwrap();
    let config = config_with_outputs(dir.path(), &["out/result.txt"]);
    let mut ck = checkpoint_with_jobs(&[("r0", "101", None)]);
    let probe = FakeProbe::new(true, vec![BackendJobStatus::Completed]);
    probe.fail_poll();

    let queries = recorded_queries(&ck, "slurm", probe.polls_elements_directly());
    let error = reconcile_recorded_jobs(
        &probe,
        &mut ck,
        queries,
        &route(),
        &config,
        dir.path(),
        false,
        false,
        zero_poll(),
    )
    .await
    .unwrap_err()
    .to_string();

    assert!(
        error.contains("cannot reach the scheduler"),
        "the scheduler error must surface: {error}"
    );
    assert!(error.contains("101"), "the job id must be named: {error}");
    // Nothing was cleared: a retry re-attaches to the same evidence.
    assert!(ck.cluster_jobs.contains_key("r0"));
}

#[tokio::test]
async fn unknown_job_state_fails_closed_unless_abandoned() {
    let dir = tempfile::tempdir().unwrap();
    let config = config_with_outputs(dir.path(), &["out/result.txt"]);
    let mut ck = checkpoint_with_jobs(&[("r0", "101", None)]);
    // Absent from the live queue and unknown to accounting.
    let probe = FakeProbe::new(true, vec![BackendJobStatus::Unknown]);

    let queries = recorded_queries(&ck, "slurm", probe.polls_elements_directly());
    let error = reconcile_recorded_jobs(
        &probe,
        &mut ck,
        queries,
        &route(),
        &config,
        dir.path(),
        false,
        false,
        zero_poll(),
    )
    .await
    .unwrap_err()
    .to_string();
    assert!(error.contains("cannot confirm the state"), "{error}");
    assert!(error.contains("--abandon-jobs"), "{error}");
    assert!(ck.cluster_jobs.contains_key("r0"));

    // abandon turns the unknown into a confirmed cancel + rerun.
    let probe = FakeProbe::new(true, vec![BackendJobStatus::Unknown]);
    let queries = recorded_queries(&ck, "slurm", probe.polls_elements_directly());
    let outcome = reconcile_recorded_jobs(
        &probe,
        &mut ck,
        queries,
        &route(),
        &config,
        dir.path(),
        true,
        false,
        zero_poll(),
    )
    .await
    .unwrap();
    assert_eq!(outcome.cancelled, vec!["r0".to_string()]);
    assert!(ck.cluster_jobs.is_empty());
}

#[tokio::test]
async fn cancel_failure_fails_closed_and_never_reruns() {
    let dir = tempfile::tempdir().unwrap();
    let config = config_with_outputs(dir.path(), &["out/result.txt"]);
    let mut ck = checkpoint_with_jobs(&[("r0", "101", None)]);
    let probe = FakeProbe::new(true, vec![BackendJobStatus::Running]);
    probe.fail_cancel();

    let queries = recorded_queries(&ck, "slurm", probe.polls_elements_directly());
    let error = reconcile_recorded_jobs(
        &probe,
        &mut ck,
        queries,
        &route(),
        &config,
        dir.path(),
        true,
        false,
        zero_poll(),
    )
    .await
    .unwrap_err()
    .to_string();
    assert!(error.contains("failed to cancel"), "{error}");
    assert!(ck.cluster_jobs.contains_key("r0"));
    assert!(!ck.failed_rules.contains("r0"));
}

#[tokio::test]
async fn accounting_settles_a_job_missing_from_the_live_queue() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(dir.path().join("out")).unwrap();
    std::fs::write(dir.path().join("out/result.txt"), "done").unwrap();
    let config = config_with_outputs(dir.path(), &["out/result.txt"]);
    let mut ck = checkpoint_with_jobs(&[("r0", "101", None)]);
    let probe = FakeProbe::new(true, vec![BackendJobStatus::Unknown])
        .with_accounting("101", BackendJobStatus::Completed);

    let queries = recorded_queries(&ck, "slurm", probe.polls_elements_directly());
    let outcome = reconcile_recorded_jobs(
        &probe,
        &mut ck,
        queries,
        &route(),
        &config,
        dir.path(),
        false,
        false,
        zero_poll(),
    )
    .await
    .unwrap();

    assert_eq!(outcome.adopted, vec!["r0".to_string()]);
    assert!(ck.cluster_jobs.is_empty());
}

#[test]
fn resolve_profile_prefers_explicit_then_recorded_then_none() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(dir.path().join("profiles")).unwrap();
    std::fs::write(
        dir.path().join("profiles/slurm.toml"),
        "[cluster]\nbackend = \"slurm\"\n",
    )
    .unwrap();
    std::fs::write(
        dir.path().join("profiles/pbs.toml"),
        "[cluster]\nbackend = \"pbs\"\n",
    )
    .unwrap();
    let mut ck = CheckpointState::new();
    ck.set_cluster_run(Some(ClusterRunRecord {
        profile: "slurm".to_string(),
        backend: "slurm".to_string(),
        queue: None,
        account: None,
        run_dir: None,
    }));

    assert_eq!(
        resolve_resume_profile(&ck, dir.path(), None).unwrap(),
        Some("slurm".to_string())
    );
    assert_eq!(
        resolve_resume_profile(&ck, dir.path(), Some("pbs")).unwrap(),
        Some("pbs".to_string())
    );
    // A recorded profile whose file is gone is not usable — the caller
    // decides between fail-closed (live jobs) and local (no live jobs).
    ck.cluster_run.as_mut().unwrap().profile = "gone".to_string();
    assert_eq!(resolve_resume_profile(&ck, dir.path(), None).unwrap(), None);
    // An EXPLICIT missing profile is always an error (same message `run`
    // gives), never silently ignored.
    let error = resolve_resume_profile(&ck, dir.path(), Some("gone"))
        .unwrap_err()
        .to_string();
    assert!(error.contains("profile 'gone' not found"), "{error}");
    // No recorded identity at all.
    let bare = CheckpointState::new();
    assert_eq!(
        resolve_resume_profile(&bare, dir.path(), None).unwrap(),
        None
    );
}

#[tokio::test]
async fn no_recorded_jobs_never_touches_the_scheduler() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(dir.path().join("profiles")).unwrap();
    std::fs::write(
        dir.path().join("profiles/slurm.toml"),
        "[cluster]\nbackend = \"slurm\"\n",
    )
    .unwrap();
    let workflow = dir.path().join("wf.oxoflow");
    std::fs::write(
        &workflow,
        "[workflow]\nname = \"x\"\n[[rules]]\nname = \"a\"\nshell = \"true\"\n",
    )
    .unwrap();
    let checkpoint_path = dir.path().join("cp.json");
    let mut ck = CheckpointState::new();
    ck.set_cluster_run(Some(ClusterRunRecord {
        profile: "slurm".to_string(),
        backend: "slurm".to_string(),
        queue: None,
        account: None,
        run_dir: None,
    }));
    ck.save_to_file(&checkpoint_path).unwrap();

    let plan = prepare_cluster_resume(
        &mut ck,
        &checkpoint_path,
        &workflow,
        None,
        false,
        false,
        dir.path(),
    )
    .await
    .unwrap();
    assert_eq!(
        plan.run_profile,
        Some("slurm".to_string()),
        "a cluster checkpoint resumes through its recorded profile"
    );
    assert!(plan.outcome.is_none());
}

#[tokio::test]
async fn recorded_jobs_without_a_profile_fail_closed_before_any_write() {
    let dir = tempfile::tempdir().unwrap();
    let workflow = dir.path().join("wf.oxoflow");
    std::fs::write(
        &workflow,
        "[workflow]\nname = \"x\"\n[[rules]]\nname = \"r0\"\nshell = \"true\"\noutput = [\"a.txt\"]\n",
    )
    .unwrap();
    let checkpoint_path = dir.path().join("cp.json");
    let mut ck = checkpoint_with_jobs(&[("r0", "101", None)]);
    ck.save_to_file(&checkpoint_path).unwrap();
    let before = std::fs::read_to_string(&checkpoint_path).unwrap();

    let error = prepare_cluster_resume(
        &mut ck,
        &checkpoint_path,
        &workflow,
        None,
        false,
        false,
        dir.path(),
    )
    .await
    .unwrap_err()
    .to_string();

    assert!(error.contains("101"), "the message names the job: {error}");
    assert!(error.contains("--profile"), "{error}");
    assert!(error.contains("--abandon-jobs"), "{error}");
    // Fail-closed happens before anything is cleared or persisted.
    assert_eq!(
        std::fs::read_to_string(&checkpoint_path).unwrap(),
        before,
        "the checkpoint must be untouched"
    );
    assert!(ck.is_running("r0"));
}

#[test]
fn direct_backends_look_up_array_elements_by_element_id() {
    // Issue #1427 review I2: SLURM polls and accounts for `44_1`, not its
    // array parent `44`; base-only backends keep the parent id.
    let ck = checkpoint_with_jobs(&[("r0", "44_1", Some("44"))]);
    let direct = recorded_queries(&ck, "slurm", true);
    assert_eq!(direct.len(), 1);
    assert_eq!(direct[0].poll_id, "44_1");
    assert_eq!(direct[0].accounting_id, "44_1");
    assert!(direct[0].per_element);

    let base_only = recorded_queries(&ck, "slurm", false);
    assert_eq!(base_only[0].poll_id, "44");
    assert_eq!(base_only[0].accounting_id, "44");
    assert!(!base_only[0].per_element);
}

#[tokio::test]
async fn mixed_backend_jobs_are_reconciled_through_their_own_scheduler() {
    // Issue #1427 review I1: a stale SLURM job id must never be handed to a
    // PBS probe (and vice versa) just because `cluster_run` now names one
    // backend.
    let dir = tempfile::tempdir().unwrap();
    let config = config_with_outputs(dir.path(), &["a.txt", "b.txt"]);
    let mut ck = checkpoint_with_jobs(&[("r0", "1", None), ("r1", "2", None)]);
    ck.cluster_jobs.get_mut("r1").unwrap().backend = "pbs".to_string();

    let slurm_queries = recorded_queries(&ck, "slurm", true);
    assert_eq!(slurm_queries.len(), 1);
    assert_eq!(slurm_queries[0].rule, "r0");
    let pbs_queries = recorded_queries(&ck, "pbs", false);
    assert_eq!(pbs_queries.len(), 1);
    assert_eq!(pbs_queries[0].rule, "r1");

    let slurm_probe = FakeProbe::new(true, vec![BackendJobStatus::Running]);
    reconcile_recorded_jobs(
        &slurm_probe,
        &mut ck,
        slurm_queries,
        &SchedulerRoute::new("slurm", None),
        &config,
        dir.path(),
        true,
        false,
        zero_poll(),
    )
    .await
    .unwrap();
    assert_eq!(slurm_probe.cancelled(), vec!["1".to_string()]);
    assert!(!ck.cluster_jobs.contains_key("r0"));
    assert!(
        ck.cluster_jobs.contains_key("r1"),
        "the PBS job must be invisible to the SLURM probe"
    );

    let pbs_probe = FakeProbe::new(false, vec![BackendJobStatus::Running]);
    let remaining = recorded_queries(&ck, "pbs", false);
    reconcile_recorded_jobs(
        &pbs_probe,
        &mut ck,
        remaining,
        &SchedulerRoute::new("pbs", None),
        &config,
        dir.path(),
        true,
        false,
        zero_poll(),
    )
    .await
    .unwrap();
    assert_eq!(pbs_probe.cancelled(), vec!["2".to_string()]);
    assert!(ck.cluster_jobs.is_empty());
}

#[tokio::test]
async fn a_completed_rule_drops_its_stale_job_without_a_profile_or_scheduler() {
    // Issue #1427 review I3: a local `run` can complete a rule while its old
    // cluster id is still recorded. That id must not wedge resume when the
    // scheduler row is later purged — a completed rule is settled.
    let dir = tempfile::tempdir().unwrap();
    let workflow = dir.path().join("wf.oxoflow");
    std::fs::write(
        &workflow,
        "[workflow]\nname = \"x\"\n[[rules]]\nname = \"r0\"\nshell = \"true\"\noutput = [\"a.txt\"]\n",
    )
    .unwrap();
    let checkpoint_path = dir.path().join("cp.json");
    let mut ck = checkpoint_with_jobs(&[("r0", "101", None)]);
    ck.mark_completed_quiet("r0");
    ck.save_to_file(&checkpoint_path).unwrap();

    // No profiles/ directory: without the prune this would fail closed
    // because unsettled jobs are recorded.
    let plan = prepare_cluster_resume(
        &mut ck,
        &checkpoint_path,
        &workflow,
        None,
        false,
        false,
        dir.path(),
    )
    .await
    .unwrap();
    assert_eq!(plan.run_profile, None);
    assert!(plan.outcome.is_none(), "nothing needed the scheduler");
    assert!(ck.cluster_jobs.is_empty());
    let persisted = CheckpointState::load_from_file(&checkpoint_path).unwrap();
    assert!(
        persisted.cluster_jobs.is_empty(),
        "the stale-id cleanup must be persisted"
    );
    assert!(persisted.completed_rules.contains("r0"));
}

#[test]
fn prune_drops_only_completed_jobs_that_are_not_in_flight() {
    // Issue #1427 re-review (Important): `run --rerun` re-submits a completed
    // rule and records its NEW job id while the rule stays in
    // `completed_rules`. That job is live — pruning it would let a resume
    // skip it and run dependents against outputs it is still rewriting.
    let mut ck = checkpoint_with_jobs(&[("done", "11", None), ("rerun", "22", None)]);
    ck.mark_completed_quiet("done");
    ck.mark_completed_quiet("rerun");
    ck.mark_running("rerun");

    let pruned = prune_stale_completed_jobs(&mut ck);

    assert_eq!(pruned.len(), 1, "only the settled completion is stale");
    assert_eq!(pruned[0].0, "done");
    assert!(!ck.cluster_jobs.contains_key("done"));
    assert!(
        ck.cluster_jobs.contains_key("rerun"),
        "a completed-but-running rerun submission must never be pruned"
    );
    assert!(ck.is_running("rerun"));
}

#[test]
fn route_queue_is_inherited_only_by_the_recorded_run_backend() {
    // Issue #1427 re-review (minor): a non-None queue must reach the group it
    // belongs to and must NOT leak to a different backend's group.
    let run = ClusterRunRecord {
        profile: "slurm".to_string(),
        backend: "slurm".to_string(),
        queue: Some("gpu".to_string()),
        account: Some("acct".to_string()),
        run_dir: None,
    };

    let (same, account) = route_for_backend("slurm", Some(&run));
    assert_eq!(same.backend, "slurm");
    assert_eq!(same.queue.as_deref(), Some("gpu"));
    assert_eq!(account.as_deref(), Some("acct"));

    let (other, account) = route_for_backend("pbs", Some(&run));
    assert_eq!(other.backend, "pbs");
    assert_eq!(
        other.queue, None,
        "a foreign backend must not inherit SLURM's queue"
    );
    assert_eq!(account, None);

    let (bare, account) = route_for_backend("pbs", None);
    assert_eq!(bare.backend, "pbs");
    assert_eq!(bare.queue, None);
    assert_eq!(account, None);
}

#[test]
fn recovery_commands_name_the_recorded_backend_and_queue() {
    // Issue #1427 review (minor): a copy-pasted recovery command must
    // address the scheduler that owns the id, not the default (SLURM).
    let routed = SchedulerRoute::new("gcp-batch", Some("us-central1".to_string()));
    assert_eq!(
        routed.status_command("7"),
        "oxo-flow cluster status -b gcp-batch --queue us-central1 7"
    );
    assert_eq!(
        routed.cancel_command("7"),
        "oxo-flow cluster cancel -b gcp-batch --queue us-central1 7"
    );
    let plain = SchedulerRoute::new("slurm", None);
    assert_eq!(
        plain.status_command("1"),
        "oxo-flow cluster status -b slurm 1"
    );
}
