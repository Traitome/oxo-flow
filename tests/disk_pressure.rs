//! Issue #843: runtime disk-pressure monitoring.
//!
//! Integration tests drive the real CLI binary against a tiny A→B→C
//! pipeline with free-space thresholds expressed in MiB. The disk-level
//! paths (Reclaim, Hold, abort) are exercised with thresholds far beyond
//! any real filesystem (`999T` = "treat the disk as always below the
//! floor"), which deterministically triggers the same code paths a full
//! disk would — without needing to fill one.

mod common;
use common::workspace_bin;

use std::fs;
use std::process::Command;

fn oxo_flow_cmd() -> Command {
    Command::new(workspace_bin("oxo-flow"))
}

/// A→B→C pipeline: A produces `a.txt` (temporary), B consumes it into
/// `b.txt` (temporary), C consumes that into `c.txt` (final). Each rule
/// appends a marker so resume behavior is observable.
fn write_chain_workflow(dir: &std::path::Path, engine_section: &str) -> std::path::PathBuf {
    let wf = dir.join("chain.oxoflow");
    fs::write(
        &wf,
        format!(
            "[workflow]\nname = \"disk-chain\"\n\n\
             {engine_section}\n\
             [[rules]]\nname = \"A\"\noutput = [\"a.txt\"]\ntemporary = true\n\
             shell = \"echo A > a.txt && echo A-run >> a.log\"\n\n\
             [[rules]]\nname = \"B\"\ninput = [\"a.txt\"]\noutput = [\"b.txt\"]\ntemporary = true\n\
             shell = \"cat a.txt > b.txt && echo B-run >> b.log\"\n\n\
             [[rules]]\nname = \"C\"\ninput = [\"b.txt\"]\noutput = [\"c.txt\"]\n\
             shell = \"cat b.txt > c.txt && echo C-run >> c.log\"\n"
        ),
    )
    .unwrap();
    wf
}

/// With `reclaim_free_disk = "999T"`, every measurement is below the
/// reclaim floor, so completed temporary-rule outputs must be reclaimed
/// (tombstoned + unlinked) as soon as all dependents finish — while the
/// run itself still succeeds end to end.
#[test]
fn reclaim_tombstones_completed_temporary_outputs_midrun() {
    let dir = tempfile::tempdir().unwrap();
    let wf = write_chain_workflow(
        dir.path(),
        "[engine]\nmin_free_disk = \"1M\"\nreclaim_free_disk = \"999T\"\n",
    );

    let output = oxo_flow_cmd()
        .args(["run", wf.to_str().unwrap(), "-j", "1"])
        .current_dir(dir.path())
        .output()
        .unwrap();
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        output.status.success(),
        "run must succeed despite reclaim pressure: {stderr}"
    );

    // A's output was reclaimed after B completed; B's after C completed.
    assert!(
        !dir.path().join("a.txt").exists(),
        "a.txt must be reclaimed mid-run"
    );
    assert!(
        !dir.path().join("b.txt").exists(),
        "b.txt must be reclaimed after C completed"
    );
    // The final rule's output is never temporary — it must survive.
    assert_eq!(fs::read_to_string(dir.path().join("c.txt")).unwrap(), "A\n");
    assert!(stderr.contains("reclaimed temporary outputs of 'A'"));

    // Tombstones were persisted in the checkpoint.
    let ck = fs::read_to_string(dir.path().join(".oxo-flow").join("checkpoint.json")).unwrap();
    assert!(ck.contains("a.txt"), "checkpoint must tombstone a.txt");
    assert!(ck.contains("b.txt"), "checkpoint must tombstone b.txt");
}

/// Resume after reclaim: the tombstoned producer is regenerated via
/// cascade-up when a dependent re-runs, and the pipeline re-completes.
#[test]
fn resume_regenerates_reclaimed_outputs() {
    let dir = tempfile::tempdir().unwrap();
    let wf = write_chain_workflow(
        dir.path(),
        "[engine]\nmin_free_disk = \"1M\"\nreclaim_free_disk = \"999T\"\n",
    );

    let first = oxo_flow_cmd()
        .args(["run", wf.to_str().unwrap(), "-j", "1"])
        .current_dir(dir.path())
        .output()
        .unwrap();
    assert!(first.status.success());

    // Force C to re-run: its input (b.txt) was tombstoned, so B must be
    // regenerated via cascade-up, which in turn regenerates A.
    fs::remove_file(dir.path().join("c.txt")).unwrap();
    let resume = oxo_flow_cmd()
        .args([
            "resume",
            dir.path()
                .join(".oxo-flow")
                .join("checkpoint.json")
                .to_str()
                .unwrap(),
        ])
        .current_dir(dir.path())
        .output()
        .unwrap();
    let stderr = String::from_utf8_lossy(&resume.stderr);
    assert!(
        resume.status.success(),
        "resume must succeed after reclaim: {stderr}"
    );
    assert_eq!(
        fs::read_to_string(dir.path().join("c.txt")).unwrap(),
        "A\n",
        "C's output must be rebuilt through the regenerated chain"
    );
    // The cascade re-executed the reclaimed producers — their append-only
    // logs gained a line each. With reclaim still armed the intermediates
    // may be re-tombstoned after C completes, so assert on run evidence
    // rather than final on-disk state.
    let a_runs = fs::read_to_string(dir.path().join("a.log"))
        .unwrap()
        .lines()
        .count();
    assert!(
        a_runs >= 2,
        "A must re-execute during resume: {a_runs} run(s)"
    );
    let b_runs = fs::read_to_string(dir.path().join("b.log"))
        .unwrap()
        .lines()
        .count();
    assert!(
        b_runs >= 2,
        "B must re-execute during resume: {b_runs} run(s)"
    );
}

/// With `min_free_disk = "999T"` (no reclaim floor), nothing is in flight
/// after the final rule completes-but-below-floor... in practice the hold
/// path engages while rules are pending: dispatches are withheld and the
/// run parks, then aborts after the grace rounds (~2 parked ticks). The
/// run must fail with the disk-pressure abort message, not hang forever.
#[test]
fn hold_below_min_free_disk_aborts_after_grace() {
    let dir = tempfile::tempdir().unwrap();
    let wf = write_chain_workflow(dir.path(), "[engine]\nmin_free_disk = \"999T\"\n");

    let started = std::time::Instant::now();
    let output = oxo_flow_cmd()
        .args(["run", wf.to_str().unwrap(), "-j", "1"])
        .current_dir(dir.path())
        .output()
        .unwrap();
    let stderr = String::from_utf8_lossy(&output.stderr);

    assert!(!output.status.success(), "run must abort under Hold");
    assert!(
        stderr.contains("disk pressure"),
        "abort message must name disk pressure: {stderr}"
    );
    // Grace = 2 parked rounds × 60s; the whole run must still terminate
    // promptly (~2-3 min), proving the park loop terminates.
    assert!(
        started.elapsed() < std::time::Duration::from_secs(300),
        "abort must happen within the grace window, took {:?}",
        started.elapsed()
    );
}
