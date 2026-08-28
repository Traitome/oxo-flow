//! Dispatch-fairness and run-loop accounting integration tests (issue #136).
//!
//! Guards the scheduler's submit-cap accounting end-to-end via the compiled
//! `oxo-flow` binary: `-j 0` must still execute rules, a failed rule must
//! not leak the running-count cap, and run-log surfaces (header masking,
//! `--log-file` resolution) must respect the masking/workdir contracts.
//! Kept in a dedicated crate so parallel sessions can own
//! `cli_integration.rs` independently (each integration-test crate
//! compiles and links on its own).

use assert_cmd::Command;
use predicates::prelude::*;
use std::fs;
use std::path::PathBuf;

/// Locate a workspace binary by name from the target directory.
///
/// This handles the case where binaries are defined in workspace sub-crates
/// rather than the root package, which means `CARGO_BIN_EXE_*` env vars
/// are not automatically set.
fn workspace_bin(name: &str) -> PathBuf {
    // Cargo sets OUT_DIR for build scripts and CARGO_MANIFEST_DIR for the package.
    // For integration tests, we can derive the target dir from the test binary location.
    let mut target_dir = std::env::current_exe()
        .expect("cannot find current test executable path")
        .parent()
        .expect("no parent dir for test exe")
        .parent()
        .expect("no grandparent dir for test exe")
        .to_path_buf();

    // Try the binary directly in the target/debug (or target/release) directory.
    let candidate = target_dir.join(name);
    if candidate.exists() {
        return candidate;
    }

    // On Windows, binaries have a .exe extension.
    let candidate_exe = target_dir.join(format!("{name}.exe"));
    if candidate_exe.exists() {
        return candidate_exe;
    }

    // Fall back to the deps subdirectory.
    target_dir = target_dir.join("deps");
    let candidate = target_dir.join(name);
    if candidate.exists() {
        return candidate;
    }

    panic!(
        "could not find binary '{name}' in target directory; \
         run `cargo build --workspace` first"
    );
}

fn oxo_flow_cmd() -> Command {
    Command::new(workspace_bin("oxo-flow"))
}

// ─── -j 0 (issue #136 fix 1) ────────────────────────────────────────

/// `-j 0` must clamp to one concurrent job like the semaphore does, not
/// silently run nothing: the submit-cap arithmetic used the raw `jobs`
/// value, so zero jobs meant zero submissions and a fake "0 succeeded" run.
#[test]
fn cli_jobs_zero_still_executes_rules() {
    let dir = tempfile::tempdir().unwrap();
    let wf = dir.path().join("j0.oxoflow");
    fs::write(
        &wf,
        "[workflow]\nname = \"j0\"\nversion = \"1.0.0\"\n\n[[rules]]\nname = \"gen\"\noutput = [\"out.txt\"]\nshell = \"echo hi > {output}\"\n",
    )
    .unwrap();

    oxo_flow_cmd()
        .args(["run", wf.to_str().unwrap(), "-j", "0"])
        .current_dir(dir.path())
        .assert()
        .success()
        .stderr(predicate::str::contains("Done: 1 succeeded"));

    assert!(
        dir.path().join("out.txt").exists(),
        "the rule must execute under -j 0"
    );
}

// ─── Submit-cap accounting after failures (issue #136 fix 2) ──────────

/// With `-j 1 --keep-going`, a failed rule must release its scheduler slot
/// so the remaining rules still run. Guards the running-count accounting:
/// if a failure path ever forgets `mark_completed`, the leaked slot shrinks
/// the submit cap to zero and every later rule silently never runs.
///
/// Note: the task-panic path in `run_command` (which previously leaked the
/// cap for real) cannot be triggered from the CLI — the executor returns
/// errors instead of panicking for reachable inputs — so this guard covers
/// the observable contract (cap accounting after a failed rule) end-to-end,
/// and the panic path itself is fixed + reasoned about in run.rs.
#[test]
fn cli_job1_keep_going_continues_after_rule_failure() {
    let dir = tempfile::tempdir().unwrap();
    let wf = dir.path().join("cap.oxoflow");
    fs::write(
        &wf,
        "[workflow]\nname = \"cap\"\nversion = \"1.0.0\"\n\n[[rules]]\nname = \"boom\"\noutput = [\"boom.txt\"]\nshell = \"exit 3\"\n\n[[rules]]\nname = \"gen\"\noutput = [\"out.txt\"]\nshell = \"echo hi > {output}\"\n",
    )
    .unwrap();

    // Since #135, --keep-going still exits nonzero when any rule failed —
    // the exit code reflects the failure, the run does not die early.
    oxo_flow_cmd()
        .args(["run", wf.to_str().unwrap(), "-j", "1", "--keep-going"])
        .current_dir(dir.path())
        .assert()
        .code(1);

    assert!(
        dir.path().join("out.txt").exists(),
        "with -j 1 the second rule must still run after the first fails — \
         a leaked running-count would permanently shrink the submit cap"
    );
}

// ─── run --json on abort paths (issue #142 H6) ─────────────────────────

/// `run --json` must emit the summary document even when the run aborts on
/// the first failure: the plain-failure path returned early (before the
/// summary emission) and left stdout at zero bytes, so scripts keying on
/// the JSON document got nothing while the keep-going path emitted it.
#[test]
fn cli_run_json_emits_failed_summary_on_abort() {
    let dir = tempfile::tempdir().unwrap();
    let wf = dir.path().join("failjson.oxoflow");
    fs::write(
        &wf,
        "[workflow]\nname = \"failjson\"\nversion = \"1.0.0\"\n\n[[rules]]\nname = \"boom\"\noutput = [\"boom.txt\"]\nshell = \"exit 3\"\n",
    )
    .unwrap();

    let output = oxo_flow_cmd()
        .args(["run", wf.to_str().unwrap(), "--json"])
        .current_dir(dir.path())
        .output()
        .unwrap();

    assert!(!output.status.success(), "failed run must exit nonzero");
    let stdout = String::from_utf8_lossy(&output.stdout);
    let doc: serde_json::Value =
        serde_json::from_str(&stdout).expect("abort path stdout must be JSON, got: {stdout}");
    assert_eq!(doc["command"], "run");
    assert_eq!(doc["status"], "failed");
    assert_eq!(doc["results"]["failed"], 1);
}

/// Pre-execution aborts (unknown --module) also emit the failed summary
/// instead of zero bytes.
#[test]
fn cli_run_json_emits_failed_summary_on_preflight_abort() {
    let dir = tempfile::tempdir().unwrap();
    let wf = dir.path().join("modjson.oxoflow");
    fs::write(
        &wf,
        "[workflow]\nname = \"modjson\"\nversion = \"1.0.0\"\n\n[[rules]]\nname = \"gen\"\noutput = [\"out.txt\"]\nshell = \"echo hi > {output}\"\n",
    )
    .unwrap();

    let output = oxo_flow_cmd()
        .args(["run", wf.to_str().unwrap(), "--module", "nope", "--json"])
        .current_dir(dir.path())
        .output()
        .unwrap();

    assert!(!output.status.success(), "unknown module must exit nonzero");
    let stdout = String::from_utf8_lossy(&output.stdout);
    let doc: serde_json::Value =
        serde_json::from_str(&stdout).expect("preflight abort stdout must be JSON, got: {stdout}");
    assert_eq!(doc["command"], "run");
    assert_eq!(doc["status"], "failed");
    assert_eq!(doc["results"]["succeeded"], 0);
}

// ─── provenance verify path resolution + exit codes (issue #142 H7) ──

/// The documented invocation — `run --provenance`, then
/// `provenance verify .oxo-flow/checkpoint.json` — must verify the intact
/// outputs and exit 0. Regression: output paths resolved against
/// `.oxo-flow/` (the checkpoint's parent) instead of the recorded
/// workdir, so every intact output was reported "file missing".
#[test]
fn cli_provenance_verify_documented_invocation_matches() {
    let dir = tempfile::tempdir().unwrap();
    let wf = dir.path().join("prov.oxoflow");
    fs::write(
        &wf,
        "[workflow]\nname = \"prov\"\nversion = \"1.0.0\"\n\n[[rules]]\nname = \"gen\"\noutput = [\"out.txt\"]\nshell = \"echo hi > {output}\"\n",
    )
    .unwrap();

    oxo_flow_cmd()
        .args(["run", wf.to_str().unwrap(), "--provenance"])
        .current_dir(dir.path())
        .assert()
        .success();

    let checkpoint = dir.path().join(".oxo-flow/checkpoint.json");
    assert!(
        checkpoint.exists(),
        "run --provenance must write the checkpoint"
    );

    // Invoke from OUTSIDE the workdir so CWD-relative resolution cannot
    // accidentally mask the bug: the checkpoint's recorded workdir is the
    // only correct base.
    let verify_dir = tempfile::tempdir().unwrap();
    oxo_flow_cmd()
        .args(["provenance", "verify", checkpoint.to_str().unwrap()])
        .current_dir(verify_dir.path())
        .assert()
        .success()
        .stderr(predicate::str::contains(
            "1 matched, 0 mismatched, 0 missing",
        ));

    assert!(dir.path().join("out.txt").exists());
}

/// `provenance verify --json` must emit the verify document on stdout
/// (issue #142 M8) — previously --json produced nothing at all.
#[test]
fn cli_provenance_verify_json_emits_document() {
    let dir = tempfile::tempdir().unwrap();
    let wf = dir.path().join("provjson.oxoflow");
    fs::write(
        &wf,
        "[workflow]\nname = \"provjson\"\nversion = \"1.0.0\"\n\n[[rules]]\nname = \"gen\"\noutput = [\"out.txt\"]\nshell = \"echo hi > {output}\"\n",
    )
    .unwrap();

    oxo_flow_cmd()
        .args(["run", wf.to_str().unwrap(), "--provenance"])
        .current_dir(dir.path())
        .assert()
        .success();

    let output = oxo_flow_cmd()
        .args([
            "provenance",
            "verify",
            dir.path()
                .join(".oxo-flow/checkpoint.json")
                .to_str()
                .unwrap(),
            "--json",
        ])
        .current_dir(dir.path())
        .output()
        .unwrap();

    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    let doc: serde_json::Value =
        serde_json::from_str(&stdout).expect("verify --json stdout must be JSON, got: {stdout}");
    assert_eq!(doc["command"], "provenance");
    assert_eq!(doc["verify"]["matched"], 1);
    assert_eq!(doc["verify"]["mismatched"], 0);
    assert_eq!(doc["verify"]["missing"], 0);
    assert_eq!(doc["verify"]["entries"][0]["status"], "matched");
}

/// A deleted output file is a verification failure: missing files exit 1
/// (previously they exited 0, which made the false-negative silent).
#[test]
fn cli_provenance_verify_missing_file_exits_nonzero() {
    let dir = tempfile::tempdir().unwrap();
    let wf = dir.path().join("provmiss.oxoflow");
    fs::write(
        &wf,
        "[workflow]\nname = \"provmiss\"\nversion = \"1.0.0\"\n\n[[rules]]\nname = \"gen\"\noutput = [\"out.txt\"]\nshell = \"echo hi > {output}\"\n",
    )
    .unwrap();

    oxo_flow_cmd()
        .args(["run", wf.to_str().unwrap(), "--provenance"])
        .current_dir(dir.path())
        .assert()
        .success();

    // Simulate a vanished (or tampered-away) output.
    fs::remove_file(dir.path().join("out.txt")).unwrap();

    oxo_flow_cmd()
        .args([
            "provenance",
            "verify",
            dir.path()
                .join(".oxo-flow/checkpoint.json")
                .to_str()
                .unwrap(),
        ])
        .current_dir(dir.path())
        .assert()
        .code(1)
        .stderr(predicate::str::contains(
            "0 matched, 0 mismatched, 1 missing",
        ));
}

// ─── Deleted-command migration hints (issue #142 H2) ──────────────────

/// Removed commands (issue #76) must name their replacement instead of
/// clap's bare "unrecognized subcommand" — scripts and muscle memory get
/// the migration path. Each legacy name exits nonzero with a hint naming
/// the successor command.
#[test]
fn cli_legacy_commands_print_migration_hints() {
    let expectations: &[(&str, &str)] = &[
        ("history", "status --timing"),
        ("package", "export"),
        ("profile", "run --profile"),
        ("watch", "status"),
    ];
    for (legacy, successor) in expectations {
        let dir = tempfile::tempdir().unwrap();
        let output = oxo_flow_cmd()
            .arg(legacy)
            .current_dir(dir.path())
            .output()
            .unwrap();
        assert!(
            !output.status.success(),
            "'{legacy}' must exit nonzero, not silently succeed"
        );
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(
            stderr.contains(successor),
            "'{legacy}' hint must name the successor '{successor}', got: {stderr}"
        );
        assert!(
            stderr.contains("removed"),
            "'{legacy}' hint must say the command was removed, got: {stderr}"
        );
    }
}

/// The migration hint fires in the subcommand position even when global
/// flags precede it.
#[test]
fn cli_legacy_command_hint_survives_global_flags() {
    let dir = tempfile::tempdir().unwrap();
    let output = oxo_flow_cmd()
        .args(["--json", "history"])
        .current_dir(dir.path())
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("status"));
}

// ─── ai explain degraded mode (issue #142 M10) ────────────────────────

/// A dead provider endpoint must degrade to the deterministic skeleton
/// and exit 0, not hard-fail: `OPENAI_BASE_URL` pointing at a dead local
/// port makes the provider call fail, and the explain output must still
/// carry the verified grounding (--json, no prose fields).
#[test]
fn cli_ai_explain_degrades_when_provider_unreachable() {
    let dir = tempfile::tempdir().unwrap();
    let wf = dir.path().join("explain.oxoflow");
    fs::write(
        &wf,
        "[workflow]\nname = \"explain\"\nversion = \"1.0.0\"\n\n[[rules]]\nname = \"gen\"\noutput = [\"out.txt\"]\nshell = \"echo hi > {output}\"\n",
    )
    .unwrap();

    let output = oxo_flow_cmd()
        .args(["ai", "explain", wf.to_str().unwrap(), "--json"])
        .env("OXO_FLOW_AI_PROVIDER", "openai")
        .env("OPENAI_BASE_URL", "http://127.0.0.1:1")
        .current_dir(dir.path())
        .output()
        .unwrap();

    assert!(
        output.status.success(),
        "a failed provider call must degrade, not hard-fail: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    let doc: serde_json::Value =
        serde_json::from_str(&stdout).expect("explain --json stdout must be JSON, got: {stdout}");
    // The deterministic skeleton is present without model prose.
    assert_eq!(doc["workflow_name"], "explain");
    assert_eq!(doc["steps"][0]["name"], "gen");
    assert_eq!(doc["overview_summary"], "");
    assert!(
        doc["provenance"]["bio_skills"].as_u64().unwrap() >= 500,
        "the knowledge-base grounding must survive a provider failure"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("skeleton"),
        "the degraded note must be visible on stderr: {stderr}"
    );
}

/// `OXO_FLOW_AI_PROVIDER=disabled` is an explicit opt-out: it must
/// override any saved config and still produce the skeleton with exit 0.
#[test]
fn cli_ai_explain_disabled_emits_skeleton_and_exits_zero() {
    let dir = tempfile::tempdir().unwrap();
    let wf = dir.path().join("explain2.oxoflow");
    fs::write(
        &wf,
        "[workflow]\nname = \"explain2\"\nversion = \"1.0.0\"\n\n[[rules]]\nname = \"gen\"\noutput = [\"out.txt\"]\nshell = \"echo hi > {output}\"\n",
    )
    .unwrap();

    let output = oxo_flow_cmd()
        .args(["ai", "explain", wf.to_str().unwrap(), "--json"])
        .env("OXO_FLOW_AI_PROVIDER", "disabled")
        .current_dir(dir.path())
        .output()
        .unwrap();

    assert!(
        output.status.success(),
        "explicit disable must exit 0 with the skeleton: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    let doc: serde_json::Value =
        serde_json::from_str(&stdout).expect("explain --json stdout must be JSON, got: {stdout}");
    assert_eq!(doc["workflow_name"], "explain2");
    assert_eq!(doc["overview_summary"], "");
}

// ─── lint text output carries hints (issue #142 M11) ──────────────────

/// The human lint report must include the fix suggestion, matching
/// validate's `hint:` line — previously only --json carried it.
#[test]
fn cli_lint_text_output_prints_hints() {
    let dir = tempfile::tempdir().unwrap();
    let wf = dir.path().join("lintsugg.oxoflow");
    fs::write(
        &wf,
        "[workflow]\nname = \"lintsugg\"\nversion = \"1.0.0\"\n\n[[rules]]\nname = \"gen\"\noutput = [\"out.txt\"]\nshell = \"echo hi > {output}\"\n",
    )
    .unwrap();

    let output = oxo_flow_cmd()
        .args(["lint", wf.to_str().unwrap()])
        .current_dir(dir.path())
        .output()
        .unwrap();

    let stderr = String::from_utf8_lossy(&output.stderr);
    // W001 (no workflow description) and W003 (no rule description) both
    // carry suggestions; the text renderer must print them.
    assert!(
        stderr.contains("hint:"),
        "lint text output must print suggestion hints, got: {stderr}"
    );
    assert!(
        stderr.contains("description"),
        "a W00x hint must be present, got: {stderr}"
    );
}

// ─── Run-log header masking (issue #136 fix 3) ───────────────────────

/// The run-log header embeds the raw command line; sensitive values passed
/// via `--arg KEY=secret` must be masked there exactly like every other
/// captured surface (issue #99 B1), not written in plaintext.
#[test]
fn cli_run_log_header_masks_sensitive_arg_values() {
    let dir = tempfile::tempdir().unwrap();
    let wf = dir.path().join("masklog.oxoflow");
    fs::write(
        &wf,
        "[workflow]\nname = \"masklog\"\nversion = \"1.0.0\"\n\n[config]\nTOKEN = { default = \"not-used\", sensitive = true }\n\n[[rules]]\nname = \"gen\"\noutput = [\"out.txt\"]\nshell = \"echo hi > {output}\"\n",
    )
    .unwrap();

    oxo_flow_cmd()
        .args(["run", wf.to_str().unwrap(), "--arg", "TOKEN=supersecret"])
        .current_dir(dir.path())
        .assert()
        .success();

    let log = fs::read_to_string(dir.path().join(".oxo-flow/logs/oxo-flow.log")).unwrap();
    assert!(
        !log.contains("supersecret"),
        "run log must not contain the raw sensitive value: {log}"
    );
    assert!(
        log.contains("TOKEN=***"),
        "run log must contain the masked command line: {log}"
    );
}

// ─── --log-file resolution (issue #136 fix 4) ─────────────────────────

/// A relative `--log-file` must resolve against the workdir (like the
/// default path), not against the current directory of the invocation.
#[test]
fn cli_log_file_relative_path_resolves_against_workdir() {
    let dir = tempfile::tempdir().unwrap();
    let wf_dir = dir.path().join("wf");
    fs::create_dir(&wf_dir).unwrap();
    let wf = wf_dir.join("rellog.oxoflow");
    fs::write(
        &wf,
        "[workflow]\nname = \"rellog\"\nversion = \"1.0.0\"\n\n[[rules]]\nname = \"gen\"\noutput = [\"out.txt\"]\nshell = \"echo hi > {output}\"\n",
    )
    .unwrap();

    // Invoke from OUTSIDE the workdir so CWD-relative and workdir-relative
    // resolution are distinguishable.
    oxo_flow_cmd()
        .args(["run", wf.to_str().unwrap(), "--log-file", "logs/custom.log"])
        .current_dir(dir.path())
        .assert()
        .success();

    assert!(
        wf_dir.join("logs/custom.log").exists(),
        "relative --log-file must resolve against the workdir (the workflow's directory)"
    );
    assert!(
        !dir.path().join("logs/custom.log").exists(),
        "relative --log-file must NOT resolve against the current directory"
    );
}

// ─── Undefined {config.*} gate (issue #142 H1) ─────────────────────────

/// A typo'd `{config.*}` key must fail `run` loudly — naming the key and
/// the rule — and must not write the literal-placeholder output file.
/// Previously the placeholder expanded to literal text, the run exited 0,
/// and the wrong data shipped silently.
#[test]
fn cli_run_undefined_config_key_fails_loudly() {
    let dir = tempfile::tempdir().unwrap();
    let wf = dir.path().join("typo.oxoflow");
    fs::write(
        &wf,
        "[workflow]\nname = \"typo\"\nversion = \"1.0.0\"\n\n[config]\nFOO = \"bar\"\n\n[[rules]]\nname = \"gen\"\noutput = [\"out.txt\"]\nshell = \"echo {config.FOO} {config.FO} > {output}\"\n",
    )
    .unwrap();

    let output = oxo_flow_cmd()
        .args(["run", wf.to_str().unwrap()])
        .current_dir(dir.path())
        .output()
        .unwrap();

    assert!(
        !output.status.success(),
        "an undefined config key must fail the run, not exit 0"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("FO") && stderr.contains("gen"),
        "error must name the undefined key and the rule, got: {stderr}"
    );
    assert!(
        !dir.path().join("out.txt").exists(),
        "no output file may be produced when the run fails the gate"
    );
}

/// `dry-run` applies the same gate: a typo'd key must be refused in the
/// preview too, so the user cannot get a "will run" plan for a workflow
/// that `run` rejects.
#[test]
fn cli_dry_run_undefined_config_key_fails_loudly() {
    let dir = tempfile::tempdir().unwrap();
    let wf = dir.path().join("typo2.oxoflow");
    fs::write(
        &wf,
        "[workflow]\nname = \"typo2\"\nversion = \"1.0.0\"\n\n[[rules]]\nname = \"gen\"\noutput = [\"out.txt\"]\nshell = \"echo {config.NOPE} > {output}\"\n",
    )
    .unwrap();

    let output = oxo_flow_cmd()
        .args(["dry-run", wf.to_str().unwrap()])
        .current_dir(dir.path())
        .output()
        .unwrap();

    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("NOPE") && stderr.contains("gen"),
        "dry-run must refuse the same typo, got: {stderr}"
    );
}

/// `run --json` on the H1 gate abort must still emit the failed summary
/// document (issue #142 H6 contract — every terminal path emits it).
#[test]
fn cli_run_json_emits_failed_summary_on_undefined_config_abort() {
    let dir = tempfile::tempdir().unwrap();
    let wf = dir.path().join("typo3.oxoflow");
    fs::write(
        &wf,
        "[workflow]\nname = \"typo3\"\nversion = \"1.0.0\"\n\n[[rules]]\nname = \"gen\"\noutput = [\"out.txt\"]\nshell = \"echo {config.NOPE} > {output}\"\n",
    )
    .unwrap();

    let output = oxo_flow_cmd()
        .args(["run", wf.to_str().unwrap(), "--json"])
        .current_dir(dir.path())
        .output()
        .unwrap();

    assert!(!output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    let doc: serde_json::Value =
        serde_json::from_str(&stdout).expect("gate abort stdout must be JSON, got: {stdout}");
    assert_eq!(doc["status"], "failed");
    assert_eq!(doc["results"]["succeeded"], 0);
}

// ─── Undeclared wildcard: preview/run parity (issue #142 H3a) ──────────

/// A workflow whose output contains an UNDECLARED `{sample}` wildcard must
/// be reported as WILL RUN by dry-run on a fresh dir — the brace path is
/// not an existing output, so the executor runs the rule (writing the
/// literal file name). Previously the preview counted the brace-containing
/// path as "outputs up-to-date" and said skip while `run` executed —
/// preview and run disagreed (issue #142 H3).
#[test]
fn cli_dry_run_reports_undeclared_wildcard_as_will_run() {
    let dir = tempfile::tempdir().unwrap();
    let wf = dir.path().join("h3a.oxoflow");
    fs::write(
        &wf,
        "[workflow]\nname = \"h3a\"\nversion = \"1.0.0\"\n\n[[rules]]\nname = \"gen\"\noutput = [\"out_{sample}.txt\"]\nshell = \"echo hi > {output}\"\n",
    )
    .unwrap();

    let output = oxo_flow_cmd()
        .args(["dry-run", wf.to_str().unwrap()])
        .current_dir(dir.path())
        .output()
        .unwrap();

    assert!(output.status.success(), "dry-run itself must succeed");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("[run: never completed]"),
        "fresh undeclared-wildcard rule must be reported as will run, got: {stderr}"
    );
    assert!(
        !stderr.contains("[skip: outputs up-to-date]"),
        "the brace path must not count as an existing fresh output, got: {stderr}"
    );
}

/// Parity on the run side: the executor executes the rule and writes the
/// LITERAL file `out_{sample}.txt` (the placeholder never expands).
#[test]
fn cli_run_undeclared_wildcard_writes_literal_file() {
    let dir = tempfile::tempdir().unwrap();
    let wf = dir.path().join("h3b.oxoflow");
    fs::write(
        &wf,
        "[workflow]\nname = \"h3b\"\nversion = \"1.0.0\"\n\n[[rules]]\nname = \"gen\"\noutput = [\"out_{sample}.txt\"]\nshell = \"echo hi > {output}\"\n",
    )
    .unwrap();

    // Audit C2: the literal-file behavior this test pinned was a silent
    // wrong-result bug — an unbound wildcard in an output pattern now
    // fails loudly BEFORE the shell runs, naming the wildcard and the
    // value sources.
    let out = oxo_flow_cmd()
        .args(["run", wf.to_str().unwrap()])
        .current_dir(dir.path())
        .output()
        .unwrap();
    assert!(
        !out.status.success(),
        "an unbound output wildcard must fail the run"
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("unbound wildcard {sample}"),
        "the error must name the wildcard: {stderr}"
    );
    assert!(
        !dir.path().join("out_{sample}.txt").exists(),
        "no literal placeholder-named file may be written"
    );
}

// ─── Undeclared wildcard lint (issue #142 H3b) ─────────────────────────

/// `lint` must flag an expandable-looking wildcard with no declared source
/// (W024) and name the placeholder — the fix suggestion points at the
/// declaration surfaces.
#[test]
fn cli_lint_warns_undeclared_wildcard() {
    let dir = tempfile::tempdir().unwrap();
    let wf = dir.path().join("h3c.oxoflow");
    fs::write(
        &wf,
        "[workflow]\nname = \"h3c\"\nversion = \"1.0.0\"\n\n[[rules]]\nname = \"gen\"\noutput = [\"out_{sample}.txt\"]\nshell = \"echo hi > {output}\"\n",
    )
    .unwrap();

    let output = oxo_flow_cmd()
        .args(["lint", wf.to_str().unwrap()])
        .current_dir(dir.path())
        .output()
        .unwrap();

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("W024") && stderr.contains("sample"),
        "lint must flag the undeclared wildcard with its code and name, got: {stderr}"
    );
}

// ─── Missing --profile is a hard error (issue #142 H4) ─────────────────

/// `run --profile <typo>` must exit nonzero and name the typo plus every
/// available profile. Previously it warned and ran with the workflow's own
/// config — the wrong environment for a silent production run.
#[test]
fn cli_run_missing_profile_exits_nonzero_and_lists_profiles() {
    let dir = tempfile::tempdir().unwrap();
    let wf = dir.path().join("prof.oxoflow");
    fs::write(
        &wf,
        "[workflow]\nname = \"prof\"\nversion = \"1.0.0\"\n\n[[rules]]\nname = \"gen\"\noutput = [\"out.txt\"]\nshell = \"echo hi > {output}\"\n",
    )
    .unwrap();
    fs::create_dir_all(dir.path().join("profiles")).unwrap();
    fs::write(
        dir.path().join("profiles/batch.toml"),
        "[config]\nthreads = 4\n",
    )
    .unwrap();
    fs::write(
        dir.path().join("profiles/dev.oxoflow"),
        "[config]\nthreads = 2\n",
    )
    .unwrap();

    let output = oxo_flow_cmd()
        .args(["run", wf.to_str().unwrap(), "--profile", "nope"])
        .current_dir(dir.path())
        .output()
        .unwrap();

    assert!(
        !output.status.success(),
        "a missing profile must fail the run, not warn-and-continue"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("nope"),
        "error must name the typo'd profile, got: {stderr}"
    );
    assert!(
        stderr.contains("batch") && stderr.contains("dev"),
        "error must list the available profiles, got: {stderr}"
    );
    assert!(
        !dir.path().join("out.txt").exists(),
        "a gate-aborted run must not execute rules"
    );
}

// ─── --samples subset must not overwrite gather outputs (issue #142 M1) ─

/// A cohort gather rule whose inputs are baked from `config.samples_list`
/// must NOT re-run (and overwrite the cohort table) when a later
/// `--samples S2` run changes the selection: the engine-injected list is
/// documented as non-invalidating. The run skips gather with a WARNING and
/// the counts file keeps the full-cohort content.
#[test]
fn cli_samples_subset_does_not_overwrite_gather_output() {
    let dir = tempfile::tempdir().unwrap();
    let wf = dir.path().join("m1.oxoflow");
    fs::write(
        &wf,
        "[workflow]\nname = \"m1\"\nversion = \"1.0.0\"\n\n[[sample_groups]]\nname = \"batch\"\nsamples = [\"S1\", \"S2\", \"S3\"]\n\n[[rules]]\nname = \"per_sample\"\noutput = [\"per/{sample}.txt\"]\nshell = \"echo {sample} > {output}\"\n\n[[rules]]\nname = \"gather\"\noutput = [\"counts.txt\"]\nexpand_inputs = [{ pattern = \"per/{sample}.txt\", variables = { sample = \"config.samples_list\" } }]\nshell = \"cat {input} > {output}\"\ndepends_on = [\"per_sample\"]\n",
    )
    .unwrap();

    // Full cohort run.
    oxo_flow_cmd()
        .args(["run", wf.to_str().unwrap()])
        .current_dir(dir.path())
        .assert()
        .success();
    let full_counts = fs::read_to_string(dir.path().join("counts.txt")).unwrap();
    assert_eq!(
        full_counts.lines().count(),
        3,
        "full run must gather all three per-sample files: {full_counts}"
    );

    // Subset run: gather must be skipped with a warning, not re-run.
    let output = oxo_flow_cmd()
        .args(["run", wf.to_str().unwrap(), "--samples", "S2"])
        .current_dir(dir.path())
        .output()
        .unwrap();

    assert!(
        output.status.success(),
        "subset run itself must succeed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("gather") && stderr.contains("--samples"),
        "subset run must warn that gather was skipped due to the sample selection, got: {stderr}"
    );
    let subset_counts = fs::read_to_string(dir.path().join("counts.txt")).unwrap();
    assert_eq!(
        subset_counts, full_counts,
        "gather must NOT overwrite the cohort table with the 1-sample subset: {subset_counts}"
    );
    // The dry-run preview agrees: gather shows as skipped, not re-run.
    let preview = oxo_flow_cmd()
        .args(["dry-run", wf.to_str().unwrap(), "--samples", "S2"])
        .current_dir(dir.path())
        .output()
        .unwrap();
    let preview_err = String::from_utf8_lossy(&preview.stderr);
    assert!(
        preview_err.contains("[skip: up to date]"),
        "preview must agree gather stays skipped, got: {preview_err}"
    );
    assert!(
        !preview_err.contains("[run: input changed]"),
        "preview must not report gather as input-invalidated, got: {preview_err}"
    );
}

/// `cluster status` with no job IDs must fail with a clear message instead
/// of invoking the scheduler's status command with an empty list
/// (issue #142 LOW).
#[test]
fn cli_cluster_status_requires_job_ids() {
    oxo_flow_cmd()
        .args(["cluster", "status", "-b", "slurm"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("requires at least one job ID"));
}
