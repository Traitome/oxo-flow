//! Issue #843: runtime disk-pressure monitoring.
//!
//! Integration tests drive the real CLI binary against a tiny A→B→C
//! pipeline with free-space thresholds expressed in MiB. The disk-level
//! paths (Reclaim, Hold, abort) are exercised with thresholds far beyond
//! any real filesystem (`999T` = "treat the disk as always below the
//! floor"), which deterministically triggers the same code paths a full
//! disk would — without needing to fill one.
//!
//! Tests that park under Hold set `OXO_FLOW_DISK_TICK_SECS` /
//! `OXO_FLOW_DISK_HOLD_ROUNDS` (same env knobs the run loop honors) to
//! keep the grace window short.

mod common;
use common::workspace_bin;

use std::fs;
use std::process::Command;

fn oxo_flow_cmd() -> Command {
    Command::new(workspace_bin("oxo-flow"))
}

/// Default C shell for the chain workflow. The F1 sentinel scenario
/// overrides it with a variant that fails on a missing marker file.
const C_SHELL: &str = "cat b.txt > c.txt && echo C-run >> c.log";

/// A→B→C pipeline: A produces `a.txt` (temporary), B consumes it into
/// `b.txt` (temporary), C consumes that into `c.txt` (final). Each rule
/// appends a marker so resume behavior is observable. `c_shell` is
/// parameterized so tests can make C fail deterministically.
fn write_chain_workflow(
    dir: &std::path::Path,
    engine_section: &str,
    c_shell: &str,
) -> std::path::PathBuf {
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
             shell = \"{c_shell}\"\n"
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
        C_SHELL,
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
        C_SHELL,
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
    let wf = write_chain_workflow(dir.path(), "[engine]\nmin_free_disk = \"999T\"\n", C_SHELL);

    let started = std::time::Instant::now();
    let output = Command::new(workspace_bin("oxo-flow"))
        .args(["run", wf.to_str().unwrap(), "-j", "1"])
        .current_dir(dir.path())
        .env("OXO_FLOW_DISK_TICK_SECS", "1")
        .env("OXO_FLOW_DISK_HOLD_ROUNDS", "2")
        .output()
        .unwrap();
    let stderr = String::from_utf8_lossy(&output.stderr);

    assert!(!output.status.success(), "run must abort under Hold");
    assert!(
        stderr.contains("disk pressure"),
        "abort message must name disk pressure: {stderr}"
    );
    // Grace = 2 parked rounds × 1s tick (env-overridden); the whole run
    // must still terminate promptly, proving the park loop terminates.
    assert!(
        started.elapsed() < std::time::Duration::from_secs(60),
        "abort must happen within the grace window, took {:?}",
        started.elapsed()
    );
}

/// F1 regression (expert review): a run that starts below the floor — or
/// falls through to Hold within a single tick — must still get a reclaim
/// pass BEFORE the Hold-park decision, or reclaimable intermediates are
/// never released and the "nothing left to reclaim" abort is untrue. The
/// old structure parked first and skipped reclaim entirely on this path.
///
/// Cold start cannot reach this state (nothing is completed yet, so there
/// is nothing to reclaim by construction), so the test drives it via
/// resume: run 1 fails C on a sentinel file (A/B complete, their
/// intermediates stay on disk because the success-only sweep never runs),
/// then the sentinel is removed and `[engine]` thresholds are added to the
/// workflow (resume re-parses the workflow from disk; `[engine]` does not
/// enter per-rule fingerprints). On resume the initial observe lands in
/// Hold with reclaimable outputs present, so reclaim must fire before the
/// park aborts.
#[test]
fn hold_park_still_reclaims_before_aborting() {
    let dir = tempfile::tempdir().unwrap();
    let wf_path = dir.path().join("chain.oxoflow");

    // Run 1: no engine thresholds; C fails on the absent sentinel, so
    // A/B are completed with their temporary outputs still on disk.
    let wf = write_chain_workflow(dir.path(), "", "cat b.txt > c.txt && [ -f allow-c ]");
    let first = Command::new(workspace_bin("oxo-flow"))
        .args(["run", wf.to_str().unwrap(), "-j", "1"])
        .current_dir(dir.path())
        .output()
        .unwrap();
    assert!(
        !first.status.success(),
        "run 1 must fail on the sentinel gate"
    );
    assert!(dir.path().join("a.txt").exists(), "a.txt survives run 1");
    assert!(dir.path().join("b.txt").exists(), "b.txt survives run 1");

    // Arm the sentinel, then add unreachable thresholds to the workflow.
    fs::write(dir.path().join("allow-c"), "").unwrap();
    fs::remove_file(&wf).unwrap();
    let wf = write_chain_workflow(
        dir.path(),
        "[engine]\nmin_free_disk = \"999T\"\nreclaim_free_disk = \"999T\"\n",
        C_SHELL,
    );
    assert_eq!(wf, wf_path, "workflow path must be stable for resume");

    let output = Command::new(workspace_bin("oxo-flow"))
        .args([
            "resume",
            dir.path()
                .join(".oxo-flow")
                .join("checkpoint.json")
                .to_str()
                .unwrap(),
        ])
        .current_dir(dir.path())
        .env("OXO_FLOW_DISK_TICK_SECS", "1")
        .env("OXO_FLOW_DISK_HOLD_ROUNDS", "2")
        .output()
        .unwrap();
    let stderr = String::from_utf8_lossy(&output.stderr);

    // The resumed run still aborts (the floor is unreachable), but the
    // reclaim pass must have fired before the park — A's output tombstoned
    // and unlinked (B is completed with its output present).
    assert!(!output.status.success(), "resume must abort under Hold");
    assert!(
        stderr.contains("disk pressure"),
        "abort message must name disk pressure: {stderr}"
    );
    assert!(
        stderr.contains("reclaimed temporary outputs of 'A'"),
        "F1: reclaim must fire before the Hold park aborts: {stderr}"
    );
    let ck = fs::read_to_string(dir.path().join(".oxo-flow").join("checkpoint.json")).unwrap();
    assert!(
        ck.contains("a.txt"),
        "A's output must be tombstoned before the abort"
    );
    assert!(
        !dir.path().join("a.txt").exists(),
        "A's output must be unlinked before the abort"
    );
}

/// Issue #844: a wildcard temporary rule's outputs (`results/{sample}.bam`
/// style) were a COMPLETE post-run no-op — `wildcard_values` only carries
/// config keys, so the pattern-level path kept its `{` and the old sweep
/// skipped it silently. The fix expands every recorded instance combo from
/// `output_pattern_domains` and deletes the concrete paths. Here the
/// producer is an `output_pattern` rule whose instances discover per-value
/// domains; its temporary output pattern must be cleaned per instance after
/// the run.
#[test]
fn post_run_cleanup_expands_wildcard_temporary_outputs() {
    let dir = tempfile::tempdir().unwrap();
    let wf = dir.path().join("fanout.oxoflow");
    fs::write(
        &wf,
        r#"[workflow]
name = "wildcard-cleanup"

[[rules]]
name = "split"
output_pattern = "chunks/{i}.txt"
temporary = true
shell = "mkdir -p chunks && for i in 1 2 3; do echo part-$i > chunks/$i.txt; done"

[[rules]]
name = "collect"
input = ["chunks/{i}.txt"]
output = ["merged/{i}.txt"]
shell = "cat {input} > {output}"
"#,
    )
    .unwrap();

    let out = Command::new(workspace_bin("oxo-flow"))
        .args(["run", wf.to_str().unwrap(), "-j", "1"])
        .current_dir(dir.path())
        .output()
        .unwrap();
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(out.status.success(), "run must succeed: {stderr}");

    // Every concrete instance path of the wildcard temporary output was
    // deleted; the final merged outputs survive.
    assert!(
        !dir.path().join("chunks").exists()
            || fs::read_dir(dir.path().join("chunks"))
                .unwrap()
                .next()
                .is_none(),
        "chunks instance files must be cleaned after the run"
    );
    for i in ["1", "2", "3"] {
        assert_eq!(
            fs::read_to_string(dir.path().join(format!("merged/{i}.txt"))).unwrap(),
            format!("part-{i}\n"),
            "final merged output must survive"
        );
    }

    // Tombstones were persisted for the concrete instance paths.
    let ck = fs::read_to_string(dir.path().join(".oxo-flow").join("checkpoint.json")).unwrap();
    assert!(
        ck.contains("chunks/1.txt") && ck.contains("chunks/2.txt") && ck.contains("chunks/3.txt"),
        "checkpoint must tombstone every expanded instance path: {ck}"
    );

    // No false-positive "could not be expanded" warning (the combos fully
    // resolve the pattern).
    assert!(
        !stderr.contains("instance paths could not be expanded"),
        "fully-expandable wildcard outputs must not be reported as skipped: {stderr}"
    );
}

/// Issue #846: mid-run reclaim must also fire for `output_pattern`
/// producers. The split rule registers no DAG edges (pattern claims are
/// excluded from template matching by design), so the pre-#846 gate
/// misclassified it as a leaf and never reclaimed it. Now the consumer is
/// detected structurally and the recorded instance combos expand to concrete
/// paths: under `reclaim_free_disk = "999T"` every completed instance of
/// `chunks/{i}.txt` is tombstoned and unlinked WHILE the run is still going
/// (collect completes afterwards via cascade-up regeneration), not only by
/// the post-run sweep.
///
/// The `summarize` rule exists for TIMING, not coverage: the mid-run gate
/// fires at the top of the run loop, so reclaim can only happen while the
/// run is still in flight. In a split→collect-only workflow the last
/// consumer completes as the final rule and the loop exits before the gate
/// re-evaluates — the post-run sweep would handle it and the mid-run path
/// would be untestable. A downstream rule keeps the run alive through that
/// iteration, mirroring the A→B→C chain test's shape.
#[test]
fn midrun_reclaim_unlinks_fanout_instances_while_run_continues() {
    let dir = tempfile::tempdir().unwrap();
    let wf = dir.path().join("fanout-reclaim.oxoflow");
    fs::write(
        &wf,
        r#"[workflow]
name = "wildcard-reclaim"

[engine]
min_free_disk = "1M"
reclaim_free_disk = "999T"

[[rules]]
name = "split"
output_pattern = "chunks/{i}.txt"
temporary = true
shell = "mkdir -p chunks && for i in 1 2 3; do echo part-$i > chunks/$i.txt; done"

[[rules]]
name = "collect"
input = ["chunks/{i}.txt"]
output = ["merged/{i}.txt"]
shell = "cat {input} > {output}"

[[rules]]
name = "summarize"
input = ["merged/{i}.txt"]
output = ["summary_{i}.txt"]
shell = "cat {input} > {output}"
"#,
    )
    .unwrap();

    let out = Command::new(workspace_bin("oxo-flow"))
        .args(["run", wf.to_str().unwrap(), "-j", "1"])
        .current_dir(dir.path())
        .output()
        .unwrap();
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(out.status.success(), "run must succeed: {stderr}");

    // The mid-run path fired: its narration differs from the post-run
    // sweep's "temporary outputs deleted for '...'".
    assert!(
        stderr.contains("reclaimed temporary outputs of 'split'"),
        "mid-run reclaim must narrate the fan-out producer: {stderr}"
    );

    // Every recorded instance was unlinked mid-run — before the run ended.
    for i in ["1", "2", "3"] {
        assert!(
            !dir.path().join(format!("chunks/{i}.txt")).exists(),
            "chunks/{i}.txt must be reclaimed mid-run, not just after the run"
        );
        // The consumers completed afterwards, proving cascade-up regeneration
        // works from the reclaimed state and the run genuinely continued.
        assert_eq!(
            fs::read_to_string(dir.path().join(format!("merged/{i}.txt"))).unwrap(),
            format!("part-{i}\n"),
            "final merged output must survive"
        );
        assert_eq!(
            fs::read_to_string(dir.path().join(format!("summary_{i}.txt"))).unwrap(),
            format!("part-{i}\n"),
            "the downstream rule must complete after the reclaim"
        );
    }

    // Tombstones were persisted for the concrete instance paths.
    let ck = fs::read_to_string(dir.path().join(".oxo-flow").join("checkpoint.json")).unwrap();
    assert!(
        ck.contains("chunks/1.txt") && ck.contains("chunks/2.txt") && ck.contains("chunks/3.txt"),
        "checkpoint must tombstone every reclaimed instance path: {ck}"
    );

    // Fail-safe silence: the combos fully resolve, so the skip warning must
    // not appear.
    assert!(
        !stderr.contains("mid-run reclaim skipped temporary output_pattern rule"),
        "fully-attributable fan-out must not emit the fail-safe warning: {stderr}"
    );
}
