//! `run --profile <NAME>` cluster path (issue #74 phase 2).
//!
//! The load-bearing test here is `cluster_and_local_agree_on_rerun_set`: the
//! cluster path derives its submission set from `preview_run_plan` plus the
//! run's `force_rules`, and this pins that combination to what the local
//! executor actually runs from identical state. If the two ever diverge,
//! this goes red rather than a cluster run silently skipping work.

mod common;
use common::workspace_bin;

use std::path::{Path, PathBuf};
use std::process::Command as StdCommand;
use std::process::Stdio;
use std::time::{Duration, Instant};

const WORKFLOW: &str = r#"
[workflow]
name = "cluster-profile"

[[sample_groups]]
name = "batch"
samples = ["S1", "S2"]

[[rules]]
name = "align"
input = ["data/{sample}.fq"]
output = ["aln/{sample}.bam"]
shell = "mkdir -p aln && cp data/{sample}.fq aln/{sample}.bam"

[[rules]]
name = "stats"
input = ["aln/{sample}.bam"]
output = ["stats/{sample}.txt"]
shell = "mkdir -p stats && wc -l aln/{sample}.bam > stats/{sample}.txt"
"#;

const CLUSTER_PROFILE: &str = r#"
[cluster]
backend = "slurm"
partition = "compute"
max_submitted = 2
poll_interval = "1s"
"#;

fn fixtures_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/mock-scheduler")
}

/// A workflow directory with two samples and an optional `profiles/slurm.toml`.
fn setup(dir: &Path, profile: Option<&str>) {
    std::fs::write(dir.join("wf.oxoflow"), WORKFLOW).unwrap();
    let data = dir.join("data");
    std::fs::create_dir_all(&data).unwrap();
    for s in ["S1", "S2"] {
        std::fs::write(data.join(format!("{s}.fq")), format!(">{s}\nACGT\n")).unwrap();
    }
    if let Some(body) = profile {
        let profiles = dir.join("profiles");
        std::fs::create_dir_all(&profiles).unwrap();
        std::fs::write(profiles.join("slurm.toml"), body).unwrap();
    }
    std::fs::create_dir_all(dir.join("sched")).unwrap();
}

/// `oxo-flow run` with the mock scheduler ahead of the real one on PATH.
/// Env goes to the child only — never `std::env::set_var`, which would race
/// the other tests in this binary.
fn run(dir: &Path, args: &[&str]) -> std::process::Output {
    let path = format!(
        "{}:{}",
        fixtures_dir().display(),
        std::env::var("PATH").unwrap_or_default()
    );
    let mut full: Vec<&str> = vec!["run", "wf.oxoflow"];
    full.extend_from_slice(args);
    StdCommand::new(workspace_bin("oxo-flow"))
        .args(&full)
        .current_dir(dir)
        .env("PATH", path)
        .env("MOCK_SCHEDULER_DIR", dir.join("sched"))
        .output()
        .unwrap()
}

/// Rules the cluster path submitted, read from the run directory's event log.
fn submitted_rules(dir: &Path) -> Vec<String> {
    let events =
        std::fs::read_to_string(dir.join(".oxo-flow/runs/latest/events.jsonl")).expect("no events");
    let mut names: Vec<String> = events
        .lines()
        .filter_map(|l| {
            let v: serde_json::Value = serde_json::from_str(l).ok()?;
            (v["t"] == "SUBMITTED").then(|| v["rule"].as_str().unwrap().to_string())
        })
        .collect();
    names.sort();
    names
}

/// Rules the local executor actually ran, read from its per-rule success lines.
fn executed_rules(output: &std::process::Output) -> Vec<String> {
    let stderr = String::from_utf8_lossy(&output.stderr);
    let mut names: Vec<String> = stderr
        .lines()
        .filter_map(|l| {
            let l = l.trim();
            let rest = l.strip_prefix("✓ ")?;
            // "✓ align_batch_S1 (0.1s)". The trailing duration is what
            // separates a per-rule line from summary lines like
            // "✓ 4 output files verified (42B total)".
            let name = rest.split_whitespace().next()?;
            rest.ends_with("s)").then(|| name.to_string())
        })
        .collect();
    names.sort();
    names
}

#[test]
fn run_profile_slurm_submits_tracks_and_records() {
    let dir = tempfile::tempdir().unwrap();
    setup(dir.path(), Some(CLUSTER_PROFILE));

    let out = run(dir.path(), &["--profile", "slurm"]);
    assert!(
        out.status.success(),
        "cluster run failed:\n{}",
        String::from_utf8_lossy(&out.stderr)
    );

    // Every rule instance was submitted, and the jobs really executed.
    assert_eq!(
        submitted_rules(dir.path()),
        vec![
            "align_batch_S1",
            "align_batch_S2",
            "stats_batch_S1",
            "stats_batch_S2"
        ]
    );
    for f in ["aln/S1.bam", "aln/S2.bam", "stats/S1.txt", "stats/S2.txt"] {
        assert!(dir.path().join(f).exists(), "missing output {f}");
    }

    // The run directory is greppable and per-rule.
    let jobs = dir.path().join(".oxo-flow/runs/latest/jobs");
    for rule in ["align_batch_S1", "stats_batch_S2"] {
        assert!(
            jobs.join(rule).join("job.sh").exists(),
            "no script for {rule}"
        );
        assert!(jobs.join(rule).join("job.id").exists(), "no id for {rule}");
    }

    // Checkpoint bookkeeping matches the local path's, so a second run is a
    // no-op rather than a resubmission.
    let ck: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(dir.path().join(".oxo-flow/checkpoint.json")).unwrap(),
    )
    .unwrap();
    let completed = ck["completed_rules"].as_array().unwrap();
    assert_eq!(completed.len(), 4, "checkpoint: {completed:?}");
    assert_eq!(ck["input_manifests"].as_object().unwrap().len(), 4);

    // Benchmarks carry what the scheduler's accounting store reported, not
    // nulls: the mock sacct answers `1234K` / `00:00:04` on the STEP row,
    // so a parser that read the allocation row alone would record neither.
    let benchmarks = ck["benchmarks"].as_object().unwrap();
    assert_eq!(benchmarks.len(), 4, "benchmarks: {benchmarks:?}");
    for (name, b) in benchmarks {
        assert_eq!(
            b["max_memory_mb"].as_u64(),
            Some(1),
            "{name} lost its peak RSS: {b}"
        );
        assert_eq!(
            b["cpu_seconds"].as_f64(),
            Some(4.0),
            "{name} lost its CPU: {b}"
        );
        // Elapsed is the RUN time sacct reported (5s), so wall time is not
        // the submit-to-settle span the driver observes.
        let wall = b["wall_time_secs"].as_f64().unwrap();
        assert!(
            (4.0..=6.0).contains(&wall),
            "{name} wall time {wall} looks like queue wait: {b}"
        );
    }

    // Report snapshot parity with the local path: the same artifacts a
    // local run leaves, so reporting pipelines cannot tell the two apart.
    let reports = dir.path().join(".oxo-flow/reports");
    let snapshots = std::fs::read_dir(&reports)
        .unwrap()
        .filter_map(|e| e.ok())
        .filter(|e| e.file_name().to_string_lossy().starts_with("report-"))
        .count();
    assert_eq!(snapshots, 1, "expected one report snapshot in {reports:?}");
    assert!(reports.join("index.json").exists());

    let second = run(dir.path(), &["--profile", "slurm"]);
    assert!(
        String::from_utf8_lossy(&second.stderr).contains("nothing to submit"),
        "second run should be a no-op:\n{}",
        String::from_utf8_lossy(&second.stderr)
    );
}

/// The parity contract: from identical state, the cluster path submits
/// exactly the set the local path executes.
#[test]
fn cluster_and_local_agree_on_rerun_set() {
    let cluster_dir = tempfile::tempdir().unwrap();
    let local_dir = tempfile::tempdir().unwrap();
    setup(cluster_dir.path(), Some(CLUSTER_PROFILE));
    setup(local_dir.path(), Some(CLUSTER_PROFILE));

    // Both start from a completed run.
    assert!(
        run(cluster_dir.path(), &["--profile", "slurm"])
            .status
            .success()
    );
    assert!(run(local_dir.path(), &[]).status.success());

    // The same input changes in both. `align_batch_S2` is invalidated
    // directly; `stats_batch_S2` only through the cascade — and its outputs
    // still look fresh at planning time, which is exactly the case a
    // preview-only submission set gets wrong.
    for d in [cluster_dir.path(), local_dir.path()] {
        std::fs::write(d.join("data/S2.fq"), ">S2 CHANGED\nACGTACGT\n").unwrap();
    }

    let cluster_out = run(cluster_dir.path(), &["--profile", "slurm"]);
    assert!(cluster_out.status.success());
    let local_out = run(local_dir.path(), &[]);
    assert!(local_out.status.success());

    let submitted = submitted_rules(cluster_dir.path());
    let executed = executed_rules(&local_out);
    assert_eq!(
        submitted, executed,
        "cluster submitted {submitted:?} but local executed {executed:?}"
    );
    assert_eq!(
        submitted,
        vec!["align_batch_S2", "stats_batch_S2"],
        "expected the changed sample's chain, cascade included"
    );
}

/// Resume after a cluster failure: fix the cause, run again. The upstream is
/// fresh and stays out of the work set, so the driver has to treat it as
/// satisfied rather than wait for it — otherwise the everyday recovery path
/// submits nothing and dies with "driver stall".
#[test]
fn resume_after_failure_submits_only_the_failed_rule() {
    let dir = tempfile::tempdir().unwrap();
    setup(dir.path(), Some(CLUSTER_PROFILE));
    // `gate` fails until the FLAG file exists — a stand-in for any fixable
    // cause (a missing module, a bad path, a full filesystem).
    std::fs::write(
        dir.path().join("wf.oxoflow"),
        r#"
[workflow]
name = "resume"

[[rules]]
name = "prep"
output = ["prep.txt"]
shell = "echo prepared > prep.txt"

[[rules]]
name = "gate"
input = ["prep.txt"]
output = ["gate.txt"]
shell = "test -f FLAG && cp prep.txt gate.txt"
"#,
    )
    .unwrap();

    let first = run(dir.path(), &["--profile", "slurm"]);
    assert!(!first.status.success(), "`gate` must fail on the first run");
    assert_eq!(submitted_rules(dir.path()), vec!["gate", "prep"]);

    std::fs::write(dir.path().join("FLAG"), "").unwrap();
    let second = run(dir.path(), &["--profile", "slurm"]);
    let stderr = String::from_utf8_lossy(&second.stderr);
    assert!(second.status.success(), "resume failed:\n{stderr}");
    assert!(!stderr.contains("driver stall"), "{stderr}");
    assert_eq!(
        submitted_rules(dir.path()),
        vec!["gate"],
        "only the failed rule re-runs; `prep` is already complete"
    );
    assert!(dir.path().join("gate.txt").exists());
}

/// Condition 4: a profile without a `[cluster]` block keeps the local path,
/// so existing config-only profiles are unaffected.
#[test]
fn profile_without_cluster_block_stays_local() {
    let dir = tempfile::tempdir().unwrap();
    setup(dir.path(), Some("[config]\nreference = \"/data/ref.fa\"\n"));

    let out = run(dir.path(), &["--profile", "slurm"]);
    assert!(out.status.success());
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        !stderr.contains("Cluster:"),
        "config-only profile must not submit to a scheduler:\n{stderr}"
    );
    assert!(!dir.path().join(".oxo-flow/runs").exists());
    assert_eq!(executed_rules(&out).len(), 4);
}

/// A workflow carrying `[cluster]` still runs locally until the user opts in
/// with `--profile` — this change must not make existing workflows submit.
#[test]
fn cluster_block_without_profile_flag_stays_local() {
    let dir = tempfile::tempdir().unwrap();
    setup(dir.path(), None);
    let with_cluster = format!("{WORKFLOW}\n[cluster]\nbackend = \"slurm\"\n");
    std::fs::write(dir.path().join("wf.oxoflow"), with_cluster).unwrap();

    let out = run(dir.path(), &[]);
    assert!(out.status.success());
    assert!(!String::from_utf8_lossy(&out.stderr).contains("Cluster:"));
    assert!(!dir.path().join(".oxo-flow/runs").exists());
}

/// `--max-submitted` is a one-off override of the profile's queue cap.
#[test]
fn max_submitted_flag_overrides_the_profile() {
    let dir = tempfile::tempdir().unwrap();
    setup(dir.path(), Some(CLUSTER_PROFILE)); // profile says 2

    let out = run(dir.path(), &["--profile", "slurm", "--max-submitted", "1"]);
    assert!(out.status.success());
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("max 1 in flight"),
        "flag must beat the profile's max_submitted = 2:\n{stderr}"
    );

    // Without the flag, the profile's value stands.
    let dir2 = tempfile::tempdir().unwrap();
    setup(dir2.path(), Some(CLUSTER_PROFILE));
    let out2 = run(dir2.path(), &["--profile", "slurm"]);
    assert!(String::from_utf8_lossy(&out2.stderr).contains("max 2 in flight"));
}

#[test]
fn cluster_profile_without_backend_is_an_actionable_error() {
    let dir = tempfile::tempdir().unwrap();
    setup(dir.path(), Some("[cluster]\npartition = \"compute\"\n"));

    let out = run(dir.path(), &["--profile", "slurm"]);
    assert!(!out.status.success(), "missing backend must fail the run");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("backend") && stderr.contains("slurm"),
        "error should name the missing key and a valid value:\n{stderr}"
    );
}

// ── Run-aware monitoring and crash recovery (issues #1085, #1083) ──────

/// Run any CLI command in the workflow dir with the mock scheduler on PATH.
fn oxo_flow(dir: &Path, args: &[&str]) -> std::process::Output {
    let path = format!(
        "{}:{}",
        fixtures_dir().display(),
        std::env::var("PATH").unwrap_or_default()
    );
    StdCommand::new(workspace_bin("oxo-flow"))
        .args(args)
        .current_dir(dir)
        .env("PATH", path)
        .env("MOCK_SCHEDULER_DIR", dir.join("sched"))
        .output()
        .unwrap()
}

/// Spawn `run --profile slurm`, keeping the child handle so a test can kill
/// the driver mid-run or steer it with extra flags. `stdout_log` captures
/// the child's stdout (the `--json` summary) when given.
fn spawn_run(dir: &Path, extra_args: &[&str], stdout_log: Option<&Path>) -> std::process::Child {
    let path = format!(
        "{}:{}",
        fixtures_dir().display(),
        std::env::var("PATH").unwrap_or_default()
    );
    let mut args = vec!["run", "wf.oxoflow", "--profile", "slurm"];
    args.extend_from_slice(extra_args);
    let mut cmd = StdCommand::new(workspace_bin("oxo-flow"));
    cmd.args(&args)
        .current_dir(dir)
        .env("PATH", path)
        .env("MOCK_SCHEDULER_DIR", dir.join("sched"))
        .stdout(match stdout_log {
            Some(path) => Stdio::from(std::fs::File::create(path).unwrap()),
            None => Stdio::null(),
        })
        .stderr(Stdio::null());
    cmd.spawn().unwrap()
}

/// Deliver SIGINT to the spawned CLI — the signal a terminal Ctrl-C sends.
fn send_sigint(child: &std::process::Child) {
    let status = StdCommand::new("kill")
        .args(["-INT", &child.id().to_string()])
        .status()
        .expect("run kill(1)");
    assert!(status.success(), "failed to deliver SIGINT");
}

fn wait_for(timeout: Duration, mut predicate: impl FnMut() -> bool) -> bool {
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        if predicate() {
            return true;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    predicate()
}

fn rule_run_status(checkpoint: &Path, rule: &str) -> Option<String> {
    let raw = std::fs::read_to_string(checkpoint).ok()?;
    let ck: serde_json::Value = serde_json::from_str(&raw).ok()?;
    ck["rule_runs"][rule]["status"].as_str().map(str::to_string)
}

/// The log path an engine-rendered script declares, with array placeholders
/// resolved for one element — an independent oracle for the CLI's resolver.
fn declared_log_path(script: &str, element_id: &str) -> PathBuf {
    let directive = script
        .lines()
        .find_map(|line| line.trim_start().strip_prefix("#SBATCH --output="))
        .expect("job.sh must declare a SLURM output path");
    let (base, index) = element_id.rsplit_once('_').unwrap_or((element_id, ""));
    PathBuf::from(directive.trim().replace("%A", base).replace("%a", index))
}

/// Simulated scheduler, real engine: a full `run --profile` writes the run
/// directory, and `cluster status|logs` must answer from it — including the
/// array `%A_%a` log path and id-to-instance mapping (issue #1085).
#[test]
fn cluster_status_and_logs_read_a_real_run_directory() {
    let dir = tempfile::tempdir().unwrap();
    setup(dir.path(), Some(CLUSTER_PROFILE));
    let out = run(dir.path(), &["--profile", "slurm"]);
    assert!(
        out.status.success(),
        "cluster run failed:\n{}",
        String::from_utf8_lossy(&out.stderr)
    );

    // The two `align` samples are grouped into one scheduler array; each
    // per-instance script declares `%A_%a`, not the instance name.
    let instance = "align_batch_S1";
    let job_dir = dir.path().join(".oxo-flow/runs/latest/jobs").join(instance);
    let script = std::fs::read_to_string(job_dir.join("job.sh")).unwrap();
    let job_id = std::fs::read_to_string(job_dir.join("job.id"))
        .unwrap()
        .trim()
        .to_string();
    assert!(
        job_id.contains('_'),
        "expected an array element id, got {job_id}"
    );
    let log_path = declared_log_path(&script, &job_id);
    assert!(
        !log_path.to_string_lossy().contains('%'),
        "array placeholders must be resolved: {}",
        log_path.display()
    );
    std::fs::create_dir_all(dir.path().join("logs")).unwrap();
    std::fs::write(dir.path().join(&log_path), "alignment marker 93%\n").unwrap();

    // status --json: the run-aware document maps every instance.
    let status = oxo_flow(dir.path(), &["--json", "cluster", "status"]);
    assert!(
        status.status.success(),
        "status failed:\n{}",
        String::from_utf8_lossy(&status.stderr)
    );
    let doc: serde_json::Value = serde_json::from_slice(&status.stdout).unwrap();
    assert!(
        doc["run_dir"]
            .as_str()
            .unwrap()
            .contains(".oxo-flow/runs/latest"),
        "{doc}"
    );
    assert_eq!(doc["counts"]["completed"], 4, "{doc}");
    let instances = doc["instances"].as_array().unwrap();
    assert_eq!(instances.len(), 4, "{doc}");
    assert!(
        instances
            .iter()
            .all(|i| i["state"] == "completed" && i["job_id"].is_string()),
        "every instance needs its scheduler id and terminal state: {doc}"
    );

    // logs by instance name reads the captured file first, even though the
    // directive carried array placeholders.
    let logs = oxo_flow(dir.path(), &["cluster", "logs", instance]);
    assert!(
        logs.status.success(),
        "logs failed:\n{}",
        String::from_utf8_lossy(&logs.stderr)
    );
    let stdout = String::from_utf8_lossy(&logs.stdout);
    assert!(stdout.contains("alignment marker 93%"), "{stdout}");
    assert!(String::from_utf8_lossy(&logs.stderr).contains("source: run-dir"));

    // ... and a scheduler id maps back to the same instance.
    let by_id = oxo_flow(dir.path(), &["--json", "cluster", "logs", &job_id]);
    assert!(
        by_id.status.success(),
        "logs by id failed:\n{}",
        String::from_utf8_lossy(&by_id.stderr)
    );
    let doc: serde_json::Value = serde_json::from_slice(&by_id.stdout).unwrap();
    assert_eq!(doc["source"], "run-dir", "{doc}");
    assert_eq!(doc["instance"], instance, "{doc}");
    assert!(
        doc["logs"].as_str().unwrap().contains("alignment marker"),
        "{doc}"
    );

    // --tail keeps only the requested trailing lines.
    std::fs::write(dir.path().join(&log_path), "first\nsecond\nthird\n").unwrap();
    let tailed = oxo_flow(dir.path(), &["cluster", "logs", instance, "--tail", "1"]);
    let stdout = String::from_utf8_lossy(&tailed.stdout);
    assert!(
        stdout.contains("third") && !stdout.contains("first"),
        "{stdout}"
    );
}

/// Simulated scheduler, real driver crash: kill the driver while its job has
/// written a partial output, then restart. The rule must be submitted again
/// (issue #1083), and the restarted run must finish it.
#[test]
fn killed_job_with_partial_output_is_requeued_end_to_end() {
    let dir = tempfile::tempdir().unwrap();
    setup(dir.path(), Some(CLUSTER_PROFILE));
    std::fs::write(
        dir.path().join("wf.oxoflow"),
        r#"
[workflow]
name = "crash"

[[rules]]
name = "stream"
output = ["out/done.txt"]
shell = "mkdir -p out && echo started > out/done.txt && { [ -f FAST ] || sleep 10; } && echo done >> out/done.txt"
"#,
    )
    .unwrap();

    let mut child = spawn_run(dir.path(), &[], None);
    let partial = dir.path().join("out/done.txt");
    let checkpoint = dir.path().join(".oxo-flow/checkpoint.json");
    let observed = wait_for(Duration::from_secs(30), || {
        partial.exists() && rule_run_status(&checkpoint, "stream").as_deref() == Some("running")
    });
    child.kill().expect("kill the driver");
    let _ = child.wait();
    assert!(
        observed,
        "driver never recorded a running submission together with the partial output"
    );

    let partial_text = std::fs::read_to_string(&partial).unwrap();
    assert!(partial_text.contains("started"), "{partial_text}");
    assert!(
        !partial_text.contains("done"),
        "the killed job must not have finished: {partial_text}"
    );

    // Re-run with the slow path disabled: the rule must be submitted again,
    // not skipped because its partial output exists.
    std::fs::write(dir.path().join("FAST"), "").unwrap();
    let restarted = run(dir.path(), &["--profile", "slurm"]);
    assert!(
        restarted.status.success(),
        "restart failed:\n{}",
        String::from_utf8_lossy(&restarted.stderr)
    );
    let events =
        std::fs::read_to_string(dir.path().join(".oxo-flow/runs/latest/events.jsonl")).unwrap();
    assert!(
        events.lines().any(|line| {
            let v: serde_json::Value = serde_json::from_str(line).unwrap();
            v["t"] == "SUBMITTED" && v["rule"] == "stream"
        }),
        "the interrupted rule must be submitted again:\n{events}"
    );
    assert!(
        std::fs::read_to_string(&partial).unwrap().contains("done"),
        "the restarted run must complete the rule"
    );
}

/// Issue #1093 F1: `--provenance` reaches the cluster path. From identical
/// workflow and inputs, the cluster checkpoint records exactly the output
/// checksums a local run records — same keys (expanded paths), same hashes.
#[test]
fn cluster_and_local_agree_on_provenance_checksums() {
    let cluster_dir = tempfile::tempdir().unwrap();
    let local_dir = tempfile::tempdir().unwrap();
    setup(cluster_dir.path(), Some(CLUSTER_PROFILE));
    setup(local_dir.path(), Some(CLUSTER_PROFILE));

    let cluster_out = run(cluster_dir.path(), &["--profile", "slurm", "--provenance"]);
    assert!(
        cluster_out.status.success(),
        "cluster run failed:\n{}",
        String::from_utf8_lossy(&cluster_out.stderr)
    );
    let local_out = run(local_dir.path(), &["--provenance"]);
    assert!(
        local_out.status.success(),
        "local run failed:\n{}",
        String::from_utf8_lossy(&local_out.stderr)
    );

    let cluster_ck = checkpoint_checksums(cluster_dir.path());
    let local_ck = checkpoint_checksums(local_dir.path());
    assert_eq!(
        cluster_ck, local_ck,
        "cluster and local checkpoints must record identical checksums"
    );
    assert_eq!(
        cluster_ck.as_object().unwrap().len(),
        4,
        "one checksum per completed output: {cluster_ck}"
    );
    for key in ["aln/S1.bam", "aln/S2.bam", "stats/S1.txt", "stats/S2.txt"] {
        let value = cluster_ck[key].as_str().unwrap_or_default();
        assert!(
            value.starts_with("sha256:"),
            "{key} must carry the default algorithm: {value}"
        );
    }
    // The zero-checksum warning is for runs that recorded nothing; this one
    // recorded four.
    let stderr = String::from_utf8_lossy(&cluster_out.stderr);
    assert!(
        !stderr.contains("recorded 0 checksums"),
        "the warning must not fire when checksums were recorded:\n{stderr}"
    );

    // Recording checksums must not disturb the skip bookkeeping: a second
    // run from the same state is still a no-op.
    let second = run(cluster_dir.path(), &["--profile", "slurm", "--provenance"]);
    assert!(
        String::from_utf8_lossy(&second.stderr).contains("nothing to submit"),
        "the run after recording checksums must still be a no-op:\n{}",
        String::from_utf8_lossy(&second.stderr)
    );
}

/// Without `--provenance` (and without per-rule `checksum`), the cluster
/// path must not pay hashing cost nor invent checksum entries.
#[test]
fn cluster_run_without_provenance_records_no_checksums() {
    let dir = tempfile::tempdir().unwrap();
    setup(dir.path(), Some(CLUSTER_PROFILE));

    let out = run(dir.path(), &["--profile", "slurm"]);
    assert!(
        out.status.success(),
        "cluster run failed:\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let checksums = checkpoint_checksums(dir.path());
    assert!(
        checksums.as_object().unwrap().is_empty(),
        "no --provenance, no per-rule checksum: {checksums}"
    );
}

/// Issue #1093 review: a run that submits nothing records nothing and must
/// say so — the local completion path warns in the same situation (issue
/// #276). Before the fix, the "nothing to submit" early return skipped the
/// warning, recreating the silent-empty failure on the most common path
/// (ask for provenance on an already up-to-date tree).
#[test]
fn up_to_date_run_with_provenance_warns_zero_checksums() {
    let dir = tempfile::tempdir().unwrap();
    setup(dir.path(), Some(CLUSTER_PROFILE));
    let first = run(dir.path(), &["--profile", "slurm"]);
    assert!(
        first.status.success(),
        "first run failed:\n{}",
        String::from_utf8_lossy(&first.stderr)
    );

    let second = run(dir.path(), &["--profile", "slurm", "--provenance"]);
    assert!(
        second.status.success(),
        "second run failed:\n{}",
        String::from_utf8_lossy(&second.stderr)
    );
    let stderr = String::from_utf8_lossy(&second.stderr);
    assert!(stderr.contains("nothing to submit"), "{stderr}");
    assert!(
        stderr.contains("recorded 0 checksums"),
        "the silent-empty case must warn:\n{stderr}"
    );
}

/// Issue #1093 F1: a rule that failed and is re-queued by the next run
/// records its checksums when it finally succeeds — dedupe and counting
/// are per-run, so an earlier process's failure never blocks a later
/// process's recording. The earlier run's checksums survive too.
#[test]
fn resumed_rule_records_provenance_checksums() {
    let dir = tempfile::tempdir().unwrap();
    setup(dir.path(), Some(CLUSTER_PROFILE));
    std::fs::write(
        dir.path().join("wf.oxoflow"),
        r#"
[workflow]
name = "resume-provenance"

[[rules]]
name = "prep"
output = ["prep.txt"]
shell = "echo prepared > prep.txt"

[[rules]]
name = "gate"
input = ["prep.txt"]
output = ["gate.txt"]
shell = "test -f FLAG && cp prep.txt gate.txt"
"#,
    )
    .unwrap();

    let first = run(dir.path(), &["--profile", "slurm", "--provenance"]);
    assert!(!first.status.success(), "`gate` must fail on the first run");
    // The rule that DID succeed recorded its checksum before the failure.
    let after_first = checkpoint_checksums(dir.path());
    assert!(
        after_first.get("prep.txt").is_some(),
        "the successful rule must record its checksum: {after_first}"
    );

    std::fs::write(dir.path().join("FLAG"), "").unwrap();
    let second = run(dir.path(), &["--profile", "slurm", "--provenance"]);
    assert!(
        second.status.success(),
        "resume failed:\n{}",
        String::from_utf8_lossy(&second.stderr)
    );
    let after_second = checkpoint_checksums(dir.path());
    assert!(
        after_second.get("gate.txt").is_some(),
        "the resumed rule must record its checksum: {after_second}"
    );
    assert!(
        after_second.get("prep.txt").is_some(),
        "the earlier run's checksum must survive the resume: {after_second}"
    );
}

// ── --keep-going failure handling (issue #1093 F2b) ───────────────────

/// `bad` fails immediately while the independent `slow` job is still in
/// flight, and `after` depends on `bad`, so it must stay blocked. `slow`
/// polls for a RELEASE file: the test decides when it is allowed to finish
/// (no timer race). The iteration bound only stops an orphaned mock job
/// from looping forever after the run ends.
const IN_FLIGHT_FAILURE_WORKFLOW: &str = r#"
[workflow]
name = "cluster-keep-going"

[[rules]]
name = "bad"
output = ["bad.txt"]
shell = "exit 1"

[[rules]]
name = "after"
input = ["bad.txt"]
output = ["after.txt"]
shell = "echo after > after.txt"

[[rules]]
name = "slow"
output = ["slow.txt"]
shell = "for _ in $(seq 1 600); do if [ -f RELEASE ]; then break; fi; sleep 0.05; done; echo done > slow.txt"
"#;

fn has_event(events: &str, t: &str, rule: &str) -> bool {
    events.lines().any(|line| {
        serde_json::from_str::<serde_json::Value>(line)
            .is_ok_and(|v| v["t"] == t && v["rule"] == rule)
    })
}

/// With `-k` on the cluster path (issue #1093 F2b), a required rule's
/// failure must not cancel independent jobs already in flight: they finish,
/// dependents of the failed rule stay blocked, and the run still exits
/// non-zero because the required rule failed.
#[test]
fn keep_going_finishes_inflight_independents_then_fails_the_run() {
    let dir = tempfile::tempdir().unwrap();
    setup(dir.path(), Some(CLUSTER_PROFILE));
    std::fs::write(dir.path().join("wf.oxoflow"), IN_FLIGHT_FAILURE_WORKFLOW).unwrap();
    let events_path = dir.path().join(".oxo-flow/runs/latest/events.jsonl");

    let mut child = spawn_run(dir.path(), &["-k"], None);
    let settled = wait_for(Duration::from_secs(30), || {
        std::fs::read_to_string(&events_path).is_ok_and(|raw| has_event(&raw, "FAILED", "bad"))
    });
    assert!(settled, "the required rule never settled as failed");

    // `bad` is terminal and `slow` is still in flight — release it.
    std::fs::write(dir.path().join("RELEASE"), "").unwrap();
    let status = child.wait().unwrap();
    assert!(
        !status.success(),
        "a required failure must still fail the run"
    );
    assert!(
        dir.path().join("slow.txt").exists(),
        "the in-flight independent must finish under -k"
    );
    assert!(
        !dir.path().join("after.txt").exists(),
        "a dependent of the failed rule must not run"
    );

    let events = std::fs::read_to_string(&events_path).unwrap();
    assert!(
        has_event(&events, "COMPLETED", "slow"),
        "slow must complete:\n{events}"
    );
    assert!(
        !has_event(&events, "CANCELLED", "slow"),
        "nothing may be cancelled under -k:\n{events}"
    );
    assert!(
        !has_event(&events, "SUBMITTED", "after"),
        "blocked dependents must not be submitted:\n{events}"
    );
}

/// Without `-k` the historical contract stands (issue #1093 F2b): a
/// required rule's failure cancels every in-flight job before the run
/// exits non-zero.
#[test]
fn without_keep_going_required_failure_cancels_inflight_independents() {
    let dir = tempfile::tempdir().unwrap();
    setup(dir.path(), Some(CLUSTER_PROFILE));
    std::fs::write(dir.path().join("wf.oxoflow"), IN_FLIGHT_FAILURE_WORKFLOW).unwrap();

    let out = run(dir.path(), &["--profile", "slurm"]);
    assert!(
        !out.status.success(),
        "a required failure must fail the run:\n{}",
        String::from_utf8_lossy(&out.stderr)
    );

    // The independent job never reached completion: it was cancelled while
    // still in flight, long before any release marker existed.
    assert!(
        !dir.path().join("slow.txt").exists(),
        "the in-flight independent must be cancelled without -k"
    );
    assert!(!dir.path().join("after.txt").exists());
    let events =
        std::fs::read_to_string(dir.path().join(".oxo-flow/runs/latest/events.jsonl")).unwrap();
    assert!(has_event(&events, "FAILED", "bad"), "{events}");
    assert!(
        !has_event(&events, "COMPLETED", "slow"),
        "slow must not complete:\n{events}"
    );
    assert!(
        !has_event(&events, "SUBMITTED", "after"),
        "blocked dependents must not be submitted:\n{events}"
    );

    // The mock scheduler's cancel is best-effort on the job process; let a
    // surviving mock job leave its loop before the tempdir disappears.
    std::fs::write(dir.path().join("RELEASE"), "").unwrap();
}

// ── Interrupt exit contract (issue #1093 follow-up) ────────────────────

/// One rule that stays in flight until the signal, plus a dependent that
/// must never be submitted once the interrupted rule settles as failed.
fn interrupt_workflow(required: bool) -> String {
    format!(
        r#"
[workflow]
name = "cluster-interrupt"

[[rules]]
name = "hang"
required = {required}
output = ["hang.txt"]
shell = "sleep 30 && echo done > hang.txt"

[[rules]]
name = "after"
input = ["hang.txt"]
output = ["after.txt"]
shell = "echo after > after.txt"
"#
    )
}

/// SIGINT while a rule is in flight: the driver cancels it, records the
/// interruption, never reports the run as completed, and exits
/// `128 + SIGINT` = 130. This must hold whether the interrupted rule is
/// `required = true` or `required = false` — an interrupt is never a
/// successful run; a `required = false` failure *without* an interrupt
/// still exits 0.
fn interrupt_exits_130(required: bool) {
    let dir = tempfile::tempdir().unwrap();
    setup(dir.path(), Some(CLUSTER_PROFILE));
    std::fs::write(dir.path().join("wf.oxoflow"), interrupt_workflow(required)).unwrap();
    let stdout_path = dir.path().join("stdout.json");
    let checkpoint = dir.path().join(".oxo-flow/checkpoint.json");
    let events_path = dir.path().join(".oxo-flow/runs/latest/events.jsonl");

    let mut child = spawn_run(dir.path(), &["--json"], Some(&stdout_path));
    let submitted = wait_for(Duration::from_secs(30), || {
        std::fs::read_to_string(&events_path).is_ok_and(|raw| has_event(&raw, "SUBMITTED", "hang"))
    });
    assert!(submitted, "the in-flight rule was never submitted");
    // The submission hook records the rule as running before the driver
    // polls again, so this proves the CLI is mid-driver (signal handlers
    // installed, job accepted) rather than racing startup.
    let running = wait_for(Duration::from_secs(30), || {
        rule_run_status(&checkpoint, "hang").as_deref() == Some("running")
    });
    assert!(running, "the rule never reached the running state");

    send_sigint(&child);
    let status = child.wait().unwrap();
    assert_eq!(
        status.code(),
        Some(130),
        "SIGINT must exit 128 + signum, got {status:?} (required = {required})"
    );

    // The interrupted verdict is persisted, and the dependent never ran.
    assert_eq!(
        rule_run_status(&checkpoint, "hang").as_deref(),
        Some("failed"),
        "the interrupted rule must stay recorded as failed (required = {required})"
    );
    let events = std::fs::read_to_string(&events_path).unwrap();
    assert!(
        events.lines().any(|line| {
            let v: serde_json::Value = serde_json::from_str(line).unwrap();
            v["t"] == "FAILED"
                && v["rule"] == "hang"
                && v["reason"]
                    .as_str()
                    .is_some_and(|r| r.contains("interrupted by SIGINT"))
        }),
        "missing the interrupted verdict:\n{events}"
    );
    assert!(
        !has_event(&events, "SUBMITTED", "after"),
        "a dependent of the interrupted rule must not be submitted:\n{events}"
    );
    assert!(!dir.path().join("after.txt").exists());

    // `--json` must never call an interrupted run completed — every
    // non-completed exit is `failed` (see docs/guide/src/commands/run.md).
    let doc: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&stdout_path).unwrap()).unwrap();
    assert_eq!(
        doc["status"], "failed",
        "an interrupt must be reported as failed: {doc}"
    );
}

#[test]
fn sigint_exits_130_when_a_required_rule_is_in_flight() {
    interrupt_exits_130(true);
}

/// The regression that motivated the fix: a `required = false` interrupt
/// used to look like a successful run (exit 0, `status: "completed"`).
#[test]
fn sigint_exits_130_when_only_non_required_rules_are_in_flight() {
    interrupt_exits_130(false);
}

/// The checkpoint's checksum map, or an empty object when the field is
/// absent.
fn checkpoint_checksums(dir: &Path) -> serde_json::Value {
    let ck: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(dir.join(".oxo-flow/checkpoint.json")).expect("no checkpoint"),
    )
    .unwrap();
    ck.get("checksums")
        .cloned()
        .unwrap_or_else(|| serde_json::json!({}))
}
