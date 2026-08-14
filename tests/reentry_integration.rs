//! P3 acceptance tests for issue #78 — checkpoint re-entry.
//!
//! A `checkpoint = true` rule writes a re-entry manifest at runtime; the
//! engine merges the new samples, re-expands from templates, and executes the
//! round-2 instances in the same run. Resumes replay recorded re-entries
//! deterministically; invalidating the checkpoint rule revokes its samples.

use std::path::{Path, PathBuf};
use std::process::Command;

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

fn write_workflow(dir: &Path, manifest_body: &str, discover_input: &str) {
    let workflow = format!(
        r#"
[workflow]
name = "reentry-e2e"

[[sample_groups]]
name = "batch"
samples = ["S1"]

[[rules]]
name = "discover"
input = [{discover_input}]
output = ["discover.toml"]
shell = "printf '%s' '{manifest_body}' > discover.toml"
checkpoint = true
checkpoint_manifest = "discover.toml"

[[rules]]
name = "analyze"
input = ["discover.toml"]
output = ["out/{{sample}}.txt"]
shell = "echo {{sample}} > out/{{sample}}.txt"
"#
    );
    std::fs::write(dir.join("wf.oxoflow"), workflow).unwrap();
    std::fs::create_dir_all(dir.join("data")).unwrap();
    std::fs::write(dir.join("data/S1.fq"), ">S1\nACGT\n").unwrap();
}

fn run(dir: &Path, extra: &[&str]) -> (bool, String) {
    let out = Command::new(workspace_bin("oxo-flow"))
        .args(["run", "wf.oxoflow"])
        .args(extra)
        .current_dir(dir)
        .output()
        .unwrap();
    let stderr = String::from_utf8_lossy(&out.stderr).to_string();
    (out.status.success(), stderr)
}

fn checkpoint(dir: &Path) -> serde_json::Value {
    serde_json::from_str(&std::fs::read_to_string(dir.join(".oxo-flow/checkpoint.json")).unwrap())
        .unwrap()
}

fn completed(dir: &Path) -> Vec<String> {
    let mut v: Vec<String> = checkpoint(dir)["completed_rules"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_str().unwrap().to_string())
        .collect();
    v.sort();
    v
}

const MANIFEST_TWO: &str = "[reentry]\\nsample = [\\\"S4\\\", \\\"S5\\\"]\\n";

#[test]
fn reentry_adds_round2_instances_and_records_rounds() {
    let dir = tempfile::tempdir().unwrap();
    write_workflow(dir.path(), MANIFEST_TWO, r#""catalog.txt""#);
    std::fs::write(dir.path().join("catalog.txt"), "v1\n").unwrap();

    let (ok, stderr) = run(dir.path(), &[]);
    assert!(ok, "run failed: {stderr}");

    // Round-2 instances executed in the same run.
    assert!(dir.path().join("out/S1.txt").exists());
    assert!(dir.path().join("out/S4.txt").exists(), "{stderr}");
    assert!(dir.path().join("out/S5.txt").exists(), "{stderr}");

    // The checkpoint records the re-entry.
    let ck = checkpoint(dir.path());
    let reentries = ck["reentries"].as_array().unwrap();
    assert_eq!(reentries.len(), 1);
    assert_eq!(reentries[0]["rule"].as_str().unwrap(), "discover");
    assert_eq!(reentries[0]["round"].as_u64().unwrap(), 1);
    let samples: Vec<&str> = reentries[0]["samples"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_str().unwrap())
        .collect();
    assert_eq!(samples, vec!["S4", "S5"]);
}

#[test]
fn resume_replays_reentry_deterministically() {
    let dir = tempfile::tempdir().unwrap();
    write_workflow(dir.path(), MANIFEST_TWO, r#""catalog.txt""#);
    std::fs::write(dir.path().join("catalog.txt"), "v1\n").unwrap();

    let (ok, _) = run(dir.path(), &[]);
    assert!(ok);
    let first = completed(dir.path());

    // Second run: nothing re-runs, the same plan reconstructs.
    let (ok, stderr) = run(dir.path(), &[]);
    assert!(ok, "resume failed: {stderr}");
    assert_eq!(completed(dir.path()), first);
    assert!(
        stderr.contains("already completed"),
        "expected skips on resume: {stderr}"
    );
    assert_eq!(
        checkpoint(dir.path())["reentries"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
}

#[test]
fn invalidating_checkpoint_rule_revokes_its_samples() {
    let dir = tempfile::tempdir().unwrap();
    write_workflow(dir.path(), MANIFEST_TWO, r#""catalog.txt""#);
    std::fs::write(dir.path().join("catalog.txt"), "v1\n").unwrap();
    let (ok, _) = run(dir.path(), &[]);
    assert!(ok);
    assert!(dir.path().join("out/S4.txt").exists());

    // Change the checkpoint rule's input AND make it emit only S4 next time.
    std::fs::write(dir.path().join("catalog.txt"), "v2\n").unwrap();
    let manifest_one = "[reentry]\\nsample = [\\\"S4\\\"]\\n";
    let workflow = std::fs::read_to_string(dir.path().join("wf.oxoflow")).unwrap();
    std::fs::write(
        dir.path().join("wf.oxoflow"),
        workflow.replace(MANIFEST_TWO, manifest_one),
    )
    .unwrap();

    let (ok, stderr) = run(dir.path(), &[]);
    assert!(ok, "re-run failed: {stderr}");

    // discover re-ran and re-recorded; S5 revoked from the plan.
    let ck = checkpoint(dir.path());
    let reentries = ck["reentries"].as_array().unwrap();
    assert_eq!(reentries.len(), 1, "superseded, not appended");
    let samples: Vec<&str> = reentries[0]["samples"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_str().unwrap())
        .collect();
    assert_eq!(samples, vec!["S4"]);
    assert!(dir.path().join("out/S4.txt").exists());
}

#[test]
fn missing_manifest_fails_the_checkpoint_rule() {
    let dir = tempfile::tempdir().unwrap();
    // discover writes nothing → checkpoint_manifest never appears.
    let workflow = r#"
[workflow]
name = "reentry-missing"

[[sample_groups]]
name = "batch"
samples = ["S1"]

[[rules]]
name = "discover"
output = ["discover.done"]
shell = "touch discover.done"
checkpoint = true
checkpoint_manifest = "discover.toml"

[[rules]]
name = "analyze"
input = ["discover.done"]
output = ["out.txt"]
shell = "touch out.txt"
"#;
    std::fs::write(dir.path().join("wf.oxoflow"), workflow).unwrap();
    let (ok, stderr) = run(dir.path(), &[]);
    assert!(!ok, "run should fail");
    assert!(
        stderr.contains("manifest"),
        "expected manifest error, got: {stderr}"
    );
    assert!(
        !dir.path().join("out.txt").exists(),
        "dependent must not run"
    );
}

#[test]
fn empty_manifest_is_valid_noop() {
    let dir = tempfile::tempdir().unwrap();
    write_workflow(dir.path(), "[reentry]\\nsample = []\\n", r#""catalog.txt""#);
    std::fs::write(dir.path().join("catalog.txt"), "v1\n").unwrap();
    let (ok, stderr) = run(dir.path(), &[]);
    assert!(ok, "empty manifest run failed: {stderr}");
    assert!(dir.path().join("out/S1.txt").exists());
    // No new samples → no re-entry record (field omitted when empty).
    let reentries = checkpoint(dir.path())["reentries"]
        .as_array()
        .map(|a| a.len())
        .unwrap_or(0);
    assert_eq!(reentries, 0);
}

#[test]
fn backend_driver_executes_reentry_rounds() {
    // The same discovery workflow through BackendDriver + mock SLURM:
    // round-1 discover completes → callback merges S4/S5 → round-2
    // instances execute in the same driver run.
    use oxo_flow_core::backend::ScheduledPlan;
    use oxo_flow_core::backend::cluster::ClusterExecutor;
    use oxo_flow_core::backend::driver::{BackendDriver, DriverConfig, DriverOptions};
    use oxo_flow_core::cluster::{ClusterBackend, ClusterJobConfig};
    use oxo_flow_core::config::WorkflowConfig;
    use oxo_flow_core::dag::WorkflowDag;
    use oxo_flow_core::environment::EnvironmentResolver;
    use oxo_flow_core::executor::JobStatus;
    use std::collections::{HashMap, HashSet};
    use std::sync::Arc;

    let dir = tempfile::tempdir().unwrap();
    write_workflow(dir.path(), MANIFEST_TWO, r#""catalog.txt""#);
    std::fs::write(dir.path().join("catalog.txt"), "v1\n").unwrap();

    let fixtures = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/mock-scheduler");
    let state = tempfile::tempdir().unwrap();
    let run_dir = tempfile::tempdir().unwrap();

    let mut config = WorkflowConfig::from_file(&dir.path().join("wf.oxoflow")).unwrap();
    config.apply_defaults();
    config.expand_wildcards().unwrap();
    let dag = WorkflowDag::from_rules(&config.rules).unwrap();
    let values: HashMap<String, String> = config
        .config
        .iter()
        .map(|(k, v)| (format!("config.{k}"), v.to_string()))
        .collect();
    let mut plan = ScheduledPlan::build(
        &config,
        &dag,
        dir.path(),
        &EnvironmentResolver::new(),
        &values,
    )
    .unwrap();
    let to_run: HashSet<String> = plan.order.iter().cloned().collect();

    // The checkpoint hook mutates the config (re-expansion) while the merge
    // hook reads it — a Mutex serializes the two closures (both Send-safe).
    let config = std::sync::Mutex::new(config);

    let executor = Arc::new(
        ClusterExecutor::new(
            ClusterBackend::Slurm,
            ClusterJobConfig {
                backend: ClusterBackend::Slurm,
                queue: None,
                account: None,
                walltime: None,
                extra_args: vec![],
            },
        )
        .with_scheduler_dir(fixtures)
        .with_env("MOCK_SCHEDULER_DIR", &state.path().to_string_lossy()),
    );
    let driver = BackendDriver::new(
        executor,
        DriverConfig {
            max_submitted: 4,
            poll_interval: std::time::Duration::from_millis(50),
            poll_timeout: Some(std::time::Duration::from_secs(30)),
        },
    );

    let runtime = tokio::runtime::Runtime::new().unwrap();
    let records = runtime
        .block_on(driver.run(
            &mut plan,
            &to_run,
            DriverOptions {
                run_dir: run_dir.path(),
                on_checkpoint: Some(Box::new(|rule_name: &str| {
                    let manifest = dir.path().join("discover.toml");
                    let content = std::fs::read_to_string(&manifest).map_err(|e| {
                        oxo_flow_core::error::OxoFlowError::Config {
                            message: format!("{rule_name}: {e}"),
                        }
                    })?;
                    let (group, samples) = oxo_flow_core::reentry::parse_manifest(&content)?;
                    if samples.is_empty() {
                        return Ok(Vec::new());
                    }
                    let mut cfg = config.lock().unwrap();
                    oxo_flow_core::reentry::apply_reentry(&mut cfg, group.as_deref(), &samples)
                })),
                merge: Some(Box::new(
                    |plan: &mut ScheduledPlan, new_names: &[String]| {
                        let cfg = config.lock().unwrap();
                        plan.merge_new_instances(
                            &cfg,
                            &dag,
                            dir.path(),
                            &EnvironmentResolver::new(),
                            &values,
                            new_names,
                        )
                    },
                )),
            },
        ))
        .unwrap();

    let mut completed: Vec<&str> = records
        .iter()
        .filter(|r| r.status == JobStatus::Success)
        .map(|r| r.rule.as_str())
        .collect();
    completed.sort();
    assert_eq!(
        completed,
        vec![
            "analyze_batch_S1",
            "analyze_batch_S4",
            "analyze_batch_S5",
            "discover"
        ]
    );

    // Round-2 jobs were submitted AFTER discover completed.
    let events = std::fs::read_to_string(run_dir.path().join("events.jsonl")).unwrap();
    let lines: Vec<&str> = events.lines().collect();
    let discover_done = lines
        .iter()
        .position(|l| l.contains("\"COMPLETED\"") && l.contains("discover"))
        .unwrap();
    let s4_submitted = lines
        .iter()
        .position(|l| l.contains("\"SUBMITTED\"") && l.contains("analyze_batch_S4"))
        .unwrap();
    assert!(
        s4_submitted > discover_done,
        "round-2 must follow the checkpoint"
    );
}
