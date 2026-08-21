//! Issue #118: failed rules must not leave partial outputs behind, and
//! pre-existing user files at declared output paths must survive untouched.

use std::fs;
use std::path::PathBuf;
use std::process::Command;

fn workspace_bin(name: &str) -> PathBuf {
    let mut path = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    path.push("target/debug");
    path.push(name);
    path
}

fn oxo_flow_cmd() -> Command {
    Command::new(workspace_bin("oxo-flow"))
}

/// A failed rule's partial outputs must not make the next run treat it as
/// up-to-date. Run 1 fails after writing a partial output; run 2 (with the
/// gate enabled so the shell can succeed) must EXECUTE the rule instead of
/// skipping it, and must not keep the stale partial content.
#[test]
fn failed_rule_outputs_are_invalidated_and_rerun() {
    let dir = tempfile::tempdir().unwrap();
    let wf = dir.path().join("fail-once.oxoflow");
    fs::write(
        &wf,
        "[workflow]\nname = \"fail-once\"\nversion = \"1.0.0\"\n\n[[rules]]\nname = \"gen\"\noutput = [\"out.txt\"]\nshell = \"if [ -f ok.flag ]; then echo done > {output}; else echo partial > {output}; exit 1; fi\"\n",
    )
    .unwrap();

    let run1 = oxo_flow_cmd()
        .args(["run", wf.to_str().unwrap()])
        .current_dir(dir.path())
        .output()
        .unwrap();
    assert!(!run1.status.success(), "run 1 must fail");
    assert!(
        !dir.path().join("out.txt").exists(),
        "the failed run must not leave partial outputs behind"
    );

    // Let the same shell succeed from now on.
    fs::write(dir.path().join("ok.flag"), b"").unwrap();

    let run2 = oxo_flow_cmd()
        .args(["run", wf.to_str().unwrap()])
        .current_dir(dir.path())
        .output()
        .unwrap();
    assert!(
        run2.status.success(),
        "run 2 must succeed: {}",
        String::from_utf8_lossy(&run2.stderr)
    );
    let stderr2 = String::from_utf8_lossy(&run2.stderr);
    assert!(
        stderr2.contains("1 succeeded"),
        "the failed rule must re-execute, not skip on stale outputs: {stderr2}"
    );
    assert_eq!(
        fs::read_to_string(dir.path().join("out.txt")).unwrap(),
        "done\n",
        "the re-executed rule must produce fresh content"
    );
}

/// A failed run must not corrupt pre-existing files at declared output
/// paths that the rule never touched: an untouched pre-existing output
/// survives the failure byte-identical.
#[test]
fn failed_rule_preserves_untouched_preexisting_outputs() {
    let dir = tempfile::tempdir().unwrap();
    let wf = dir.path().join("preserve.oxoflow");
    fs::write(
        &wf,
        "[workflow]\nname = \"preserve\"\nversion = \"1.0.0\"\n\n[[rules]]\nname = \"gen\"\noutput = [\"out.txt\"]\nshell = \"exit 1\"\n",
    )
    .unwrap();
    fs::write(dir.path().join("out.txt"), b"user-data").unwrap();

    // --rerun forces execution past the freshness gate; the rule then
    // fails without touching the file.
    let run = oxo_flow_cmd()
        .args(["run", wf.to_str().unwrap(), "--rerun"])
        .current_dir(dir.path())
        .output()
        .unwrap();
    assert!(!run.status.success());
    assert_eq!(
        fs::read_to_string(dir.path().join("out.txt")).unwrap(),
        "user-data",
        "an untouched pre-existing output must survive a failed run"
    );
}

/// A pre-existing output that a failed rule MODIFIES is moved aside as
/// `<name>.oxo-failed` (recoverable) instead of being silently destroyed —
/// and the freshness gate then re-runs the rule on the next attempt.
#[test]
fn failed_rule_moves_aside_modified_preexisting_output() {
    let dir = tempfile::tempdir().unwrap();
    let wf = dir.path().join("modify.oxoflow");
    fs::write(
        &wf,
        "[workflow]\nname = \"modify\"\nversion = \"1.0.0\"\n\n[[rules]]\nname = \"gen\"\noutput = [\"out.txt\"]\nshell = \"echo corrupt > {output}; exit 1\"\n",
    )
    .unwrap();
    fs::write(dir.path().join("out.txt"), b"user-data").unwrap();

    let run = oxo_flow_cmd()
        .args(["run", wf.to_str().unwrap(), "--rerun"])
        .current_dir(dir.path())
        .output()
        .unwrap();
    assert!(!run.status.success());
    assert!(
        !dir.path().join("out.txt").exists(),
        "the modified pre-existing output must not sit at its declared path"
    );
    assert_eq!(
        fs::read_to_string(dir.path().join("out.txt.oxo-failed")).unwrap(),
        "corrupt\n",
        "the failed content must be preserved for recovery"
    );
}
