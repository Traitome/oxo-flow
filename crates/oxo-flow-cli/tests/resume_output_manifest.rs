//! End-to-end strict-resume verification of recorded output manifests
//! (issue #1426).
//!
//! A rule that writes 1024 bytes is truncated to 500 after a successful run.
//! `resume` must re-execute it and restore the file (the recorded manifest's
//! size no longer matches) instead of reporting `completed` / `remaining 0`
//! from a size+mtime verdict. Intact outputs still skip; legacy checkpoints
//! (no manifest) re-run unless `--trust-mtime` accepts mtime evidence.
use std::path::{Path, PathBuf};
use std::process::Command;

const WRITER: &str = r#"[workflow]
name = "manifest-demo"

[[rules]]
name = "write"
output = ["out.bin"]
shell = "echo run >> runs.log && head -c 1024 /dev/zero > out.bin"
"#;

fn oxo() -> Command {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_oxo-flow"));
    // Ambient overrides must never decide a test's verdict.
    cmd.env_remove("OXO_FLOW_MANIFEST_HASH_MAX_BYTES");
    cmd
}

fn run_cli(dir: &Path, args: &[&str], hash_cap: Option<&str>) -> (bool, String, String) {
    let mut cmd = oxo();
    cmd.args(args).current_dir(dir);
    if let Some(cap) = hash_cap {
        cmd.env("OXO_FLOW_MANIFEST_HASH_MAX_BYTES", cap);
    }
    let out = cmd.output().unwrap();
    (
        out.status.success(),
        String::from_utf8_lossy(&out.stdout).to_string(),
        String::from_utf8_lossy(&out.stderr).to_string(),
    )
}

fn write_workflow(dir: &Path) -> PathBuf {
    let wf = dir.join("w.oxoflow");
    std::fs::write(&wf, WRITER).unwrap();
    wf
}

fn checkpoint(dir: &Path) -> PathBuf {
    dir.join(".oxo-flow/checkpoint.json")
}

fn run_count(dir: &Path) -> usize {
    std::fs::read_to_string(dir.join("runs.log"))
        .map(|log| log.lines().count())
        .unwrap_or(0)
}

fn output_len(dir: &Path) -> u64 {
    std::fs::metadata(dir.join("out.bin")).unwrap().len()
}

fn file_len(dir: &Path, name: &str) -> u64 {
    std::fs::metadata(dir.join(name)).unwrap().len()
}

/// Remove the `output_manifests` map, simulating a pre-#1426 checkpoint
/// written before output manifests existed.
fn strip_output_manifests(path: &Path) {
    let text = std::fs::read_to_string(path).unwrap();
    let mut doc: serde_json::Value = serde_json::from_str(&text).unwrap();
    doc.as_object_mut().unwrap().remove("output_manifests");
    std::fs::write(path, serde_json::to_string(&doc).unwrap()).unwrap();
}

fn first_run(dir: &Path, hash_cap: Option<&str>) {
    let wf = write_workflow(dir);
    let (ok, _stdout, stderr) = run_cli(dir, &["run", wf.to_str().unwrap(), "-j", "1"], hash_cap);
    assert!(ok, "initial run failed:\n{stderr}");
    assert_eq!(run_count(dir), 1, "the rule executed once");
    assert_eq!(output_len(dir), 1024);
}

/// The defect: a 1024-byte output truncated to 500 bytes was still reported
/// up to date. Strict `resume` re-executes and restores the artifact.
#[test]
fn resume_reruns_a_truncated_output_and_restores_it() {
    let dir = tempfile::tempdir().unwrap();
    first_run(dir.path(), None);

    // Truncate to 500 bytes. The rewrite also bumps the mtime, so the
    // historical size+mtime verdict would be "fresh" — only the recorded
    // manifest catches the damage.
    std::fs::write(dir.path().join("out.bin"), vec![0u8; 500]).unwrap();
    assert_eq!(output_len(dir.path()), 500);

    let (ok, _stdout, stderr) = run_cli(
        dir.path(),
        &["resume", checkpoint(dir.path()).to_str().unwrap()],
        None,
    );
    assert!(ok, "resume must succeed:\n{stderr}");
    assert_eq!(
        output_len(dir.path()),
        1024,
        "the truncated output must be regenerated"
    );
    assert_eq!(run_count(dir.path()), 2, "the rule re-executed:\n{stderr}");
    assert!(
        stderr.contains("strict output verification failed"),
        "the forced re-run must be warned about:\n{stderr}"
    );
    assert!(
        stderr.contains("out.bin"),
        "the warning must name the output:\n{stderr}"
    );
}

/// An intact output verifies against the manifest: resume skips it.
#[test]
fn resume_skips_a_verified_output() {
    let dir = tempfile::tempdir().unwrap();
    first_run(dir.path(), None);

    let (ok, _stdout, stderr) = run_cli(
        dir.path(),
        &["resume", checkpoint(dir.path()).to_str().unwrap()],
        None,
    );
    assert!(ok, "resume must succeed:\n{stderr}");
    assert_eq!(run_count(dir.path()), 1, "no re-execution:\n{stderr}");
    assert!(
        stderr.contains("1 completed, 0 failed, 0 remaining"),
        "the verified rule must leave nothing remaining:\n{stderr}"
    );
    assert!(
        stderr.contains("1 skipped"),
        "the verified rule must be skipped:\n{stderr}"
    );
}

/// A legacy checkpoint carries no output manifest. The safe default is to
/// re-execute; `--trust-mtime` restores the historical size+mtime verdict.
#[test]
fn resume_reruns_a_legacy_checkpoint_unless_trust_mtime() {
    let dir = tempfile::tempdir().unwrap();
    first_run(dir.path(), None);
    let cp = checkpoint(dir.path());

    strip_output_manifests(&cp);
    let (ok, _stdout, stderr) = run_cli(dir.path(), &["resume", cp.to_str().unwrap()], None);
    assert!(ok, "resume must succeed:\n{stderr}");
    assert_eq!(
        run_count(dir.path()),
        2,
        "a manifest-less completion must re-execute:\n{stderr}"
    );
    assert!(
        stderr.contains("no output manifest is recorded"),
        "the reason must be named:\n{stderr}"
    );

    // Strip the fresh manifest again, then accept mtime evidence explicitly.
    strip_output_manifests(&cp);
    let (ok, _stdout, stderr) = run_cli(
        dir.path(),
        &["resume", cp.to_str().unwrap(), "--trust-mtime"],
        None,
    );
    assert!(ok, "trust-mtime resume must succeed:\n{stderr}");
    assert_eq!(
        run_count(dir.path()),
        2,
        "--trust-mtime must accept the size+mtime verdict:\n{stderr}"
    );
    assert!(
        stderr.contains("1 skipped"),
        "the rule must be skipped:\n{stderr}"
    );
}

/// Above the recording cap the manifest stores size only. Strict resume
/// treats that entry as unverified (re-run); `--trust-mtime` skips it.
#[test]
fn resume_reruns_an_unhashed_large_output_unless_trust_mtime() {
    let dir = tempfile::tempdir().unwrap();
    // 1024-byte output, 16-byte cap → size-only manifest entry.
    first_run(dir.path(), Some("16"));
    let cp = checkpoint(dir.path());

    let entry = serde_json::from_str::<serde_json::Value>(&std::fs::read_to_string(&cp).unwrap())
        .unwrap()
        .get("output_manifests")
        .and_then(|m| m.get("write"))
        .and_then(|m| m.get(0))
        .cloned()
        .expect("manifest entry recorded");
    assert_eq!(entry["size"], 1024);
    assert!(
        entry.get("sha256").is_none(),
        "over-cap output must be size-only: {entry}"
    );

    let (ok, _stdout, stderr) = run_cli(dir.path(), &["resume", cp.to_str().unwrap()], Some("16"));
    assert!(ok, "resume must succeed:\n{stderr}");
    assert_eq!(
        run_count(dir.path()),
        2,
        "unverified entry re-runs:\n{stderr}"
    );
    assert!(
        stderr.contains("no recorded content hash"),
        "the unverified reason must be named:\n{stderr}"
    );

    let (ok, _stdout, stderr) = run_cli(
        dir.path(),
        &["resume", cp.to_str().unwrap(), "--trust-mtime"],
        Some("16"),
    );
    assert!(ok, "trust-mtime resume must succeed:\n{stderr}");
    assert_eq!(run_count(dir.path()), 2, "trust-mtime skips:\n{stderr}");
    assert!(stderr.contains("1 skipped"), "{stderr}");
}

/// With a cap at/above the output size the hash is recorded, so an intact
/// output skips — the strict default is usable, not merely a re-run hammer.
#[test]
fn resume_skips_a_hashed_large_output() {
    let dir = tempfile::tempdir().unwrap();
    first_run(dir.path(), Some("1024"));

    let cp = checkpoint(dir.path());
    let entry = serde_json::from_str::<serde_json::Value>(&std::fs::read_to_string(&cp).unwrap())
        .unwrap()
        .get("output_manifests")
        .and_then(|m| m.get("write"))
        .and_then(|m| m.get(0))
        .cloned()
        .expect("manifest entry recorded");
    assert!(
        entry["sha256"]
            .as_str()
            .is_some_and(|h| h.starts_with("sha256:")),
        "at-cap output must record a hash: {entry}"
    );

    let (ok, _stdout, stderr) =
        run_cli(dir.path(), &["resume", cp.to_str().unwrap()], Some("1024"));
    assert!(ok, "resume must succeed:\n{stderr}");
    assert_eq!(run_count(dir.path()), 1, "hashed output skips:\n{stderr}");
}

/// Transform-chunk cleanup deletes the chunk files (outputs of the map
/// rules), and strict resume must deal with that without a stale-manifest
/// false positive: the deleted chunks' manifest entries are cleared with
/// their files (LOW-4), the intact combine output still skips, and the run
/// succeeds.
#[test]
fn transform_cleanup_clears_chunk_manifests_and_resume_succeeds() {
    let dir = tempfile::tempdir().unwrap();
    let wf = dir.path().join("tf.oxoflow");
    std::fs::write(
        &wf,
        r#"[workflow]
name = "tf"

[[rules]]
name = "chunked"
output = ["parts/chunk.txt"]
shell = "echo final > {output[0]}"

[rules.transform.split]
by = "chunk"
n = "3"

[rules.transform]
map = "echo chunk_{chunk} >> chunk_runs.log && echo chunk_{chunk} > {output[0]}"
cleanup = true

[rules.transform.combine]
shell = "echo combine >> combine_runs.log && cat {chunks} > {output[0]}"
"#,
    )
    .unwrap();
    let (ok, _stdout, stderr) =
        run_cli(dir.path(), &["run", wf.to_str().unwrap(), "-j", "1"], None);
    assert!(ok, "initial transform run failed:\n{stderr}");
    assert!(
        dir.path().join("parts/chunk.txt").exists(),
        "the combine output exists"
    );
    assert!(
        !dir.path().join(".oxo-flow/chunks/chunk/0.txt").exists(),
        "cleanup removed the chunk files"
    );

    // The deleted chunks' manifest entries are cleared with them; the
    // combine's own manifest remains.
    let ck: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(checkpoint(dir.path())).unwrap()).unwrap();
    let manifests = ck["output_manifests"].as_object().unwrap();
    assert!(
        manifests.contains_key("chunked_combine"),
        "the combine output must stay recorded: {manifests:?}"
    );
    for chunk_rule in ["chunked_0", "chunked_1", "chunked_2"] {
        assert!(
            !manifests.contains_key(chunk_rule),
            "a cleaned chunk's manifest entry must not survive: {chunk_rule}"
        );
        assert!(
            ck["tombstones"]
                .as_object()
                .is_some_and(|tombstones| tombstones.contains_key(chunk_rule)),
            "a cleaned chunk producer must be tombstoned like a temporary output: {chunk_rule}"
        );
    }

    // Strict resume: the combine output verifies and stays skipped; the
    // cleaned map rules are tombstone-kept (their outputs are gone by
    // design and the combine is complete), so this run is a no-op rather
    // than a regeneration cycle.
    let (ok, _stdout, stderr) = run_cli(
        dir.path(),
        &[
            "resume",
            checkpoint(dir.path()).to_str().unwrap(),
            "-j",
            "1",
        ],
        None,
    );
    assert!(ok, "resume after transform cleanup must succeed:\n{stderr}");
    assert_eq!(
        std::fs::read_to_string(dir.path().join("combine_runs.log"))
            .unwrap()
            .lines()
            .count(),
        1,
        "the intact combine output must not re-run"
    );
    assert_eq!(
        std::fs::read_to_string(dir.path().join("chunk_runs.log"))
            .unwrap()
            .lines()
            .count(),
        3,
        "tombstoned chunk producers stay skipped while the combine is complete"
    );

    // Deleting the combine output brings the whole chain back: the combine
    // re-runs and cascade-up regenerates the tombstoned chunks, which are
    // cleaned (and re-tombstoned) again afterwards.
    std::fs::remove_file(dir.path().join("parts/chunk.txt")).unwrap();
    let (ok, _stdout, stderr) = run_cli(
        dir.path(),
        &[
            "resume",
            checkpoint(dir.path()).to_str().unwrap(),
            "-j",
            "1",
        ],
        None,
    );
    assert!(ok, "lazy regeneration resume must succeed:\n{stderr}");
    assert!(
        dir.path().join("parts/chunk.txt").exists(),
        "the combine output was rebuilt from regenerated chunks"
    );
    assert_eq!(
        std::fs::read_to_string(dir.path().join("combine_runs.log"))
            .unwrap()
            .lines()
            .count(),
        2,
        "the deleted combine output re-ran"
    );
    assert_eq!(
        std::fs::read_to_string(dir.path().join("chunk_runs.log"))
            .unwrap()
            .lines()
            .count(),
        6,
        "cascade-up regenerated the tombstoned chunk producers"
    );
    assert!(
        !dir.path().join(".oxo-flow/chunks/chunk/0.txt").exists(),
        "the regenerated chunks are cleaned again"
    );
}

/// HIGH-1: output_pattern consumer instances are re-created by the re-entry
/// replay at run start (the initial config cannot know them). A tampered
/// consumer output must still be caught by the POST-replay strict pass — the
/// pre-replay plan does not contain the instances at all.
#[test]
fn resume_verifies_replayed_output_pattern_instances() {
    let dir = tempfile::tempdir().unwrap();
    let wf = dir.path().join("replay.oxoflow");
    std::fs::write(
        &wf,
        r#"[workflow]
name = "replay-strict"

[[rules]]
name = "split"
output_pattern = "results/chunks/{i}.txt"
shell = "mkdir -p results/chunks && echo split >> split_runs.log && for i in 1 2 3; do echo part-$i > results/chunks/$i.txt; done"

[[rules]]
name = "collect"
input = ["results/chunks/{i}.txt"]
output = ["results/merged/{i}.txt"]
shell = "echo {i} >> collect_runs.log && cat {input} > {output}"
"#,
    )
    .unwrap();
    let (ok, _stdout, stderr) =
        run_cli(dir.path(), &["run", wf.to_str().unwrap(), "-j", "1"], None);
    assert!(ok, "initial replay run failed:\n{stderr}");

    // Damage one replayed consumer instance's output.
    std::fs::write(dir.path().join("results/merged/2.txt"), "XX").unwrap();

    let (ok, _stdout, stderr) = run_cli(
        dir.path(),
        &[
            "resume",
            checkpoint(dir.path()).to_str().unwrap(),
            "-j",
            "1",
        ],
        None,
    );
    assert!(ok, "resume must succeed:\n{stderr}");
    assert_eq!(
        std::fs::read_to_string(dir.path().join("results/merged/2.txt")).unwrap(),
        "part-2\n",
        "the replayed consumer instance must be re-verified and re-run:\n{stderr}"
    );
    let runs = std::fs::read_to_string(dir.path().join("collect_runs.log")).unwrap();
    assert_eq!(
        runs.lines().filter(|line| *line == "2").count(),
        2,
        "only the damaged replayed instance re-ran: {runs}"
    );
    assert_eq!(
        runs.lines().filter(|line| *line == "1").count(),
        1,
        "an intact replayed instance stays skipped: {runs}"
    );
    assert_eq!(
        std::fs::read_to_string(dir.path().join("split_runs.log"))
            .unwrap()
            .lines()
            .count(),
        1,
        "the completed producer replays without re-running: {stderr}"
    );
}

/// HIGH-2: a strict invalidation that forces a combine must be visible to
/// the tombstone cascade-up. With the combine's output above the hash cap
/// (size-only manifest), strict resume invalidates the combine; its
/// cleanup-tombstoned map producers must regenerate instead of being
/// pre-skipped, or the combine re-runs against deleted chunk inputs.
#[test]
fn resume_over_cap_combine_regenerates_tombstoned_chunks() {
    let dir = tempfile::tempdir().unwrap();
    let wf = dir.path().join("tf.oxoflow");
    std::fs::write(
        &wf,
        r#"[workflow]
name = "tf"

[[rules]]
name = "chunked"
output = ["parts/chunk.txt"]
shell = "echo final > {output[0]}"

[rules.transform.split]
by = "chunk"
n = "3"

[rules.transform]
map = "echo chunk_{chunk} >> chunk_runs.log && echo chunk_{chunk} > {output[0]}"
cleanup = true

[rules.transform.combine]
shell = "echo combine >> combine_runs.log && cat {chunks} > {output[0]}"
"#,
    )
    .unwrap();
    // Cap below the 24-byte combine output (the 8-byte chunk outputs still
    // hash at exactly the cap): the combine's entry is size-only/unverified.
    let cap = Some("8");
    let (ok, _stdout, stderr) = run_cli(dir.path(), &["run", wf.to_str().unwrap(), "-j", "1"], cap);
    assert!(ok, "initial transform run failed:\n{stderr}");

    let (ok, _stdout, stderr) = run_cli(
        dir.path(),
        &[
            "resume",
            checkpoint(dir.path()).to_str().unwrap(),
            "-j",
            "1",
        ],
        cap,
    );
    assert!(
        ok,
        "resume must not run the combine against deleted chunks:\n{stderr}"
    );
    assert!(
        dir.path().join("parts/chunk.txt").exists(),
        "the combine output was rebuilt"
    );
    assert_eq!(
        std::fs::read_to_string(dir.path().join("combine_runs.log"))
            .unwrap()
            .lines()
            .count(),
        2,
        "the size-only combine re-ran"
    );
    assert_eq!(
        std::fs::read_to_string(dir.path().join("chunk_runs.log"))
            .unwrap()
            .lines()
            .count(),
        6,
        "the tombstoned map producers regenerated before the combine"
    );
}

/// `run --verify-outputs` opts a plain run (e.g. a retry wrapper) into the
/// same strict check; without the flag `run` keeps the historical gate.
#[test]
fn run_verify_outputs_flag_rejects_a_truncated_output() {
    let dir = tempfile::tempdir().unwrap();
    first_run(dir.path(), None);
    let wf = dir.path().join("w.oxoflow");
    std::fs::write(dir.path().join("out.bin"), vec![0u8; 500]).unwrap();

    // Historical `run`: completed + fresh output → skipped, damage persists.
    let (ok, _stdout, stderr) = run_cli(dir.path(), &["run", wf.to_str().unwrap()], None);
    assert!(ok, "plain run succeeds:\n{stderr}");
    assert_eq!(run_count(dir.path()), 1, "plain run keeps the mtime gate");
    assert_eq!(output_len(dir.path()), 500);

    // Opting in re-runs the rule and restores the artifact.
    let (ok, _stdout, stderr) = run_cli(
        dir.path(),
        &["run", wf.to_str().unwrap(), "--verify-outputs"],
        None,
    );
    assert!(ok, "verify-outputs run succeeds:\n{stderr}");
    assert_eq!(
        run_count(dir.path()),
        2,
        "the damaged rule re-ran:\n{stderr}"
    );
    assert_eq!(output_len(dir.path()), 1024);
}

/// `run --verify-outputs` has no `--trust-mtime` escape hatch: a size-only
/// (over-cap) manifest entry is UNVERIFIED and must re-execute.
#[test]
fn run_verify_outputs_refuses_a_size_only_manifest() {
    let dir = tempfile::tempdir().unwrap();
    first_run(dir.path(), Some("16"));
    let wf = dir.path().join("w.oxoflow");

    let (ok, _stdout, stderr) = run_cli(
        dir.path(),
        &["run", wf.to_str().unwrap(), "--verify-outputs"],
        Some("16"),
    );
    assert!(ok, "verify-outputs run succeeds:\n{stderr}");
    assert_eq!(
        run_count(dir.path()),
        2,
        "an unhashed (size-only) manifest entry must re-run:\n{stderr}"
    );
    assert_eq!(output_len(dir.path()), 1024);
}

/// A deleted declared output re-executes (the manifest has no file to
/// verify).
#[test]
fn resume_reruns_when_a_declared_output_is_missing() {
    let dir = tempfile::tempdir().unwrap();
    first_run(dir.path(), None);
    std::fs::remove_file(dir.path().join("out.bin")).unwrap();

    let (ok, _stdout, stderr) = run_cli(
        dir.path(),
        &["resume", checkpoint(dir.path()).to_str().unwrap()],
        None,
    );
    assert!(ok, "resume must succeed:\n{stderr}");
    assert_eq!(output_len(dir.path()), 1024, "the output was regenerated");
    assert_eq!(run_count(dir.path()), 2, "the rule re-executed:\n{stderr}");
}

/// MEDIUM-3: a damaged producer's completed dependent must re-execute too.
/// Sequence: producer completes (manifest 1024 B); the output is truncated;
/// the consumer runs later and records a baseline from the truncated file;
/// resume re-runs the producer — the consumer must not be skipped on its own
/// intact output, or the run exits 0 with results derived from the damaged
/// artifact.
#[test]
fn resume_invalidates_a_completed_downstream_of_a_damaged_producer() {
    let dir = tempfile::tempdir().unwrap();
    let wf = dir.path().join("chain.oxoflow");
    std::fs::write(
        &wf,
        r#"[workflow]
name = "downstream"

[[rules]]
name = "producer"
output = ["a.bin"]
shell = "echo producer >> producer.log && head -c 1024 /dev/zero > a.bin"

[[rules]]
name = "consumer"
input = ["a.bin"]
output = ["b.size"]
shell = "echo consumer >> consumer.log && wc -c < a.bin > b.size"
"#,
    )
    .unwrap();

    // Producer alone, then damage its output, then let the consumer run
    // against the damaged file and complete (baseline = 500).
    let (ok, _stdout, stderr) = run_cli(
        dir.path(),
        &["run", wf.to_str().unwrap(), "-t", "producer"],
        None,
    );
    assert!(ok, "producer run failed:\n{stderr}");
    assert_eq!(file_len(dir.path(), "a.bin"), 1024);
    std::fs::write(dir.path().join("a.bin"), vec![0u8; 500]).unwrap();
    let (ok, _stdout, stderr) = run_cli(
        dir.path(),
        &["run", wf.to_str().unwrap(), "-t", "consumer"],
        None,
    );
    assert!(ok, "consumer run failed:\n{stderr}");
    assert_eq!(
        std::fs::read_to_string(dir.path().join("b.size"))
            .unwrap()
            .trim(),
        "500",
        "the consumer's first run consumed the damaged artifact"
    );

    let (ok, _stdout, stderr) = run_cli(
        dir.path(),
        &["resume", checkpoint(dir.path()).to_str().unwrap()],
        None,
    );
    assert!(ok, "resume must succeed:\n{stderr}");
    assert_eq!(
        file_len(dir.path(), "a.bin"),
        1024,
        "the producer regenerated"
    );
    assert!(
        stderr.contains("strict output verification invalidated"),
        "the downstream cascade must be narrated:\n{stderr}"
    );
    assert_eq!(
        std::fs::read_to_string(dir.path().join("b.size"))
            .unwrap()
            .trim(),
        "1024",
        "the completed consumer must re-execute against the restored output"
    );
    assert_eq!(
        std::fs::read_to_string(dir.path().join("producer.log"))
            .unwrap()
            .lines()
            .count(),
        2,
        "producer ran twice"
    );
    assert_eq!(
        std::fs::read_to_string(dir.path().join("consumer.log"))
            .unwrap()
            .lines()
            .count(),
        2,
        "consumer ran twice (not skipped on its intact-but-stale output)"
    );
}

/// Fan-out: manifests are per instance. Truncating one instance's output
/// re-runs only that instance; the sibling stays skipped.
#[test]
fn resume_verifies_fanout_instances_independently() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(dir.path().join("data")).unwrap();
    for sample in ["S1", "S2"] {
        std::fs::write(
            dir.path().join(format!("data/{sample}.fastq")),
            format!("{sample}-reads\n"),
        )
        .unwrap();
    }
    let wf = dir.path().join("fanout.oxoflow");
    std::fs::write(
        &wf,
        r#"[workflow]
name = "fanout"

[[sample_groups]]
name = "cohort"
samples = ["S1", "S2"]

[[rules]]
name = "qc"
input = ["data/{sample}.fastq"]
output = ["out/{sample}.txt"]
shell = "echo {sample} >> runs.log && cat {input} > {output}"
"#,
    )
    .unwrap();
    let (ok, _stdout, stderr) =
        run_cli(dir.path(), &["run", wf.to_str().unwrap(), "-j", "1"], None);
    assert!(ok, "initial fan-out run failed:\n{stderr}");

    // Truncate ONE instance's output.
    std::fs::write(dir.path().join("out/S1.txt"), "S1").unwrap();

    let (ok, _stdout, stderr) = run_cli(
        dir.path(),
        &[
            "resume",
            checkpoint(dir.path()).to_str().unwrap(),
            "-j",
            "1",
        ],
        None,
    );
    assert!(ok, "resume must succeed:\n{stderr}");
    assert_eq!(
        std::fs::read_to_string(dir.path().join("out/S1.txt")).unwrap(),
        "S1-reads\n",
        "the damaged instance regenerated"
    );
    assert_eq!(
        std::fs::read_to_string(dir.path().join("out/S2.txt")).unwrap(),
        "S2-reads\n",
        "the intact instance is untouched"
    );
    let runs = std::fs::read_to_string(dir.path().join("runs.log")).unwrap();
    assert_eq!(
        runs.lines().filter(|line| *line == "S1").count(),
        2,
        "S1 re-ran: {runs}"
    );
    assert_eq!(
        runs.lines().filter(|line| *line == "S2").count(),
        1,
        "S2 stayed skipped: {runs}"
    );
}
