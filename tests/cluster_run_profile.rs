//! `run --profile <NAME>` cluster path (issue #74 phase 2).
//!
//! The load-bearing test here is `cluster_and_local_agree_on_rerun_set`: the
//! cluster path derives its submission set from `preview_run_plan` plus the
//! run's `force_rules`, and this pins that combination to what the local
//! executor actually runs from identical state. If the two ever diverge,
//! this goes red rather than a cluster run silently skipping work.

use std::path::{Path, PathBuf};
use std::process::Command as StdCommand;

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

/// Locate a workspace binary (mirrors the helper in cluster_backend.rs).
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
