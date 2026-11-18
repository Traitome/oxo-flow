//! End-to-end wiring tests for the rule-shell environment allowlist
//! (issues #1108 / #1113 / #1173).
//!
//! The unit tests in `executor::process` pin the policy body; these tests pin
//! the SPAWN SITE — that a local docker rule actually receives the admitted
//! container-daemon variables, and that a bare rule does not. Without this,
//! flipping the spawn-site flag would silently reintroduce #1173 while every
//! policy unit test stayed green.

mod common;
use common::workspace_bin;

use std::path::{Path, PathBuf};
use std::process::Command as StdCommand;

fn write_docker_workflow(dir: &Path) {
    std::fs::write(
        dir.join("wf.oxoflow"),
        r#"
[workflow]
name = "docker_env"

[[rules]]
name = "probe"
shell = "true"

[rules.environment]
docker = "fake/image:latest"
"#,
    )
    .unwrap();
}

fn write_bare_workflow(dir: &Path) {
    std::fs::write(
        dir.join("wf.oxoflow"),
        r#"
[workflow]
name = "bare_env"

[[rules]]
name = "probe"
shell = "printf '%s' \"$DOCKER_HOST\" > bare_env_observed.txt"
"#,
    )
    .unwrap();
}

/// A fake `docker` that records the daemon variable it was spawned with and
/// succeeds for every setup/verify/wrap invocation, so no real daemon is
/// needed.
fn fake_docker_bin(dir: &Path) -> PathBuf {
    let bin_dir = dir.join("fakebin");
    std::fs::create_dir_all(&bin_dir).unwrap();
    let path = bin_dir.join("docker");
    std::fs::write(
        &path,
        "#!/bin/sh\nprintf '%s' \"$DOCKER_HOST\" > docker_env_observed.txt\nexit 0\n",
    )
    .unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut perms = std::fs::metadata(&path).unwrap().permissions();
        perms.set_mode(0o755);
        std::fs::set_permissions(&path, perms).unwrap();
    }
    bin_dir
}

fn run_with_env(dir: &Path, path_prefix: &Path) {
    let base_path = std::env::var("PATH").unwrap_or_default();
    let status = StdCommand::new(workspace_bin("oxo-flow"))
        .args(["run", "wf.oxoflow"])
        .current_dir(dir)
        .env("PATH", format!("{}:{base_path}", path_prefix.display()))
        .env("DOCKER_HOST", "tcp://sentinel.example:2376")
        .env("OXO_FLOW_AI_SESSION_DIR", dir.join("ai_sessions"))
        .status()
        .unwrap();
    assert!(status.success(), "run must succeed");
}

#[test]
fn docker_rule_shell_receives_the_admitted_daemon_variable() {
    let dir = tempfile::tempdir().unwrap();
    write_docker_workflow(dir.path());
    let bin_dir = fake_docker_bin(dir.path());
    run_with_env(dir.path(), &bin_dir);

    let observed =
        std::fs::read_to_string(dir.path().join("docker_env_observed.txt")).unwrap_or_default();
    assert_eq!(
        observed, "tcp://sentinel.example:2376",
        "the docker wrapper must see the admitted DOCKER_HOST (issue #1173)"
    );
}

#[test]
fn bare_rule_shell_does_not_receive_the_daemon_variable() {
    let dir = tempfile::tempdir().unwrap();
    write_bare_workflow(dir.path());
    let bin_dir = dir.path().join("unused_bin");
    std::fs::create_dir_all(&bin_dir).unwrap();
    run_with_env(dir.path(), &bin_dir);

    let observed = std::fs::read_to_string(dir.path().join("bare_env_observed.txt")).unwrap();
    assert_eq!(
        observed, "",
        "a bare rule must not observe DOCKER_HOST (issue #1113 allowlist)"
    );
}

// ─── RB-15 / #1233: CLI-side spawn sites outside the rule executor ───────
//
// The rule-shell site has its own coverage above. These tests pin the two
// CLI shell spawns that RB-15 left outside the allowlist: the reference
// build shell (`commands/run/refs.rs`) and the `batch` sugar shell
// (`commands/mod.rs`). Each asserts the SENTINEL `ANTHROPIC_AUTH_TOKEN`
// cannot leak, that `OXO_FLOW_ENV_PASSTHROUGH` still admits a named
// variable, and that the `[engine] tmp_dir`-derived `TMPDIR` reaches the
// shell.

const LEAK_CANARY: &str = "leak-canary";

fn base_env<'a>(cmd: &'a mut StdCommand, dir: &Path) -> &'a mut StdCommand {
    cmd.env("PATH", std::env::var("PATH").unwrap_or_default())
        .env("ANTHROPIC_AUTH_TOKEN", LEAK_CANARY)
        .env("OXO_FLOW_AI_SESSION_DIR", dir.join("ai_sessions"))
}

fn write_reference_workflow(dir: &Path, build: &str) {
    std::fs::write(
        dir.join("refleak.oxoflow"),
        format!(
            "[workflow]\nname = \"refleak\"\n\n[[references]]\nname = \"probe\"\noutput = \"refleak_out.txt\"\nbuild = \"{build}\"\n"
        ),
    )
    .unwrap();
}

#[test]
fn reference_build_shell_cannot_see_parent_secrets() {
    let dir = tempfile::tempdir().unwrap();
    write_reference_workflow(
        dir.path(),
        "printf '%s' \\\"$ANTHROPIC_AUTH_TOKEN\\\" > sentinel_observed.txt",
    );
    let mut cmd = StdCommand::new(workspace_bin("oxo-flow"));
    base_env(&mut cmd, dir.path());
    let status = cmd
        .args(["run", "refleak.oxoflow"])
        .current_dir(dir.path())
        .status()
        .unwrap();
    assert!(status.success(), "run must succeed");

    let observed =
        std::fs::read_to_string(dir.path().join("sentinel_observed.txt")).unwrap_or_default();
    assert_eq!(
        observed, "",
        "the reference build shell must not observe the parent's ANTHROPIC_AUTH_TOKEN (#1233)"
    );
    assert!(!observed.contains(LEAK_CANARY));
}

#[test]
fn reference_build_shell_receives_declared_tmpdir() {
    let dir = tempfile::tempdir().unwrap();
    let scratch = dir.path().join("scratch");
    write_reference_workflow(dir.path(), "printf '%s' \\\"$TMPDIR\\\" > tmp_observed.txt");
    let mut cmd = StdCommand::new(workspace_bin("oxo-flow"));
    base_env(&mut cmd, dir.path());
    cmd.env("OXO_FLOW_TMPDIR", &scratch);
    let status = cmd
        .args(["run", "refleak.oxoflow"])
        .current_dir(dir.path())
        .status()
        .unwrap();
    assert!(status.success(), "run must succeed");

    let observed = std::fs::read_to_string(dir.path().join("tmp_observed.txt")).unwrap_or_default();
    assert_eq!(
        observed,
        scratch.display().to_string(),
        "the reference build shell must receive the [engine] tmp_dir-derived TMPDIR (#1233)"
    );
}

#[test]
fn reference_build_shell_honours_env_passthrough() {
    let dir = tempfile::tempdir().unwrap();
    write_reference_workflow(
        dir.path(),
        "printf '%s' \\\"$RB15_DECLARED\\\" > passthrough.txt",
    );
    let mut cmd = StdCommand::new(workspace_bin("oxo-flow"));
    base_env(&mut cmd, dir.path());
    cmd.env("OXO_FLOW_ENV_PASSTHROUGH", "RB15_DECLARED")
        .env("RB15_DECLARED", "declared-value");
    let status = cmd
        .args(["run", "refleak.oxoflow"])
        .current_dir(dir.path())
        .status()
        .unwrap();
    assert!(status.success(), "run must succeed");

    let observed = std::fs::read_to_string(dir.path().join("passthrough.txt")).unwrap_or_default();
    assert_eq!(
        observed, "declared-value",
        "OXO_FLOW_ENV_PASSTHROUGH must re-admit a named variable to the reference build shell"
    );
}

#[test]
fn batch_shell_cannot_see_parent_secrets() {
    let dir = tempfile::tempdir().unwrap();
    let mut cmd = StdCommand::new(workspace_bin("oxo-flow"));
    base_env(&mut cmd, dir.path());
    let status = cmd
        .args([
            "batch",
            "printf '%s' \"$ANTHROPIC_AUTH_TOKEN\" > sentinel_observed.txt",
            "x",
        ])
        .current_dir(dir.path())
        .status()
        .unwrap();
    assert!(status.success(), "batch must succeed");

    let observed =
        std::fs::read_to_string(dir.path().join("sentinel_observed.txt")).unwrap_or_default();
    assert_eq!(
        observed, "",
        "the batch shell must not observe the parent's ANTHROPIC_AUTH_TOKEN (#1233)"
    );
    assert!(!observed.contains(LEAK_CANARY));
}

#[test]
fn batch_shell_receives_declared_tmpdir() {
    let dir = tempfile::tempdir().unwrap();
    let scratch = dir.path().join("scratch");
    let mut cmd = StdCommand::new(workspace_bin("oxo-flow"));
    base_env(&mut cmd, dir.path());
    cmd.env("OXO_FLOW_TMPDIR", &scratch);
    let status = cmd
        .args(["batch", "printf '%s' \"$TMPDIR\" > tmp_observed.txt", "x"])
        .current_dir(dir.path())
        .status()
        .unwrap();
    assert!(status.success(), "batch must succeed");

    let observed = std::fs::read_to_string(dir.path().join("tmp_observed.txt")).unwrap_or_default();
    assert_eq!(
        observed,
        scratch.display().to_string(),
        "the batch shell must receive the OXO_FLOW_TMPDIR-derived TMPDIR (#1233)"
    );
}

#[test]
fn batch_shell_honours_env_passthrough() {
    let dir = tempfile::tempdir().unwrap();
    let mut cmd = StdCommand::new(workspace_bin("oxo-flow"));
    base_env(&mut cmd, dir.path());
    cmd.env("OXO_FLOW_ENV_PASSTHROUGH", "RB15_DECLARED")
        .env("RB15_DECLARED", "declared-value");
    let status = cmd
        .args([
            "batch",
            "printf '%s' \"$RB15_DECLARED\" > passthrough.txt",
            "x",
        ])
        .current_dir(dir.path())
        .status()
        .unwrap();
    assert!(status.success(), "batch must succeed");

    let observed = std::fs::read_to_string(dir.path().join("passthrough.txt")).unwrap_or_default();
    assert_eq!(
        observed, "declared-value",
        "OXO_FLOW_ENV_PASSTHROUGH must re-admit a named variable to the batch shell"
    );
}

// ─── #1233 review finding: batch container items keep daemon vars ───────
//
// `run_batch_command` cannot see the `-e` spec, so the caller derives
// `container_daemon` from the spec prefix. A `docker:`/`singularity:` item
// renders a `docker run` / `singularity exec` wrapper invoked from THIS
// shell, so the daemon variables must be re-admitted (issue #1173). A
// non-container item must NOT see them.

fn write_fake_docker(dir: &Path) -> PathBuf {
    let bin_dir = dir.join("fakebin");
    std::fs::create_dir_all(&bin_dir).unwrap();
    let path = bin_dir.join("docker");
    std::fs::write(
        &path,
        "#!/bin/sh\nprintf '%s' \"$DOCKER_HOST\" > docker_env_observed.txt\nexit 0\n",
    )
    .unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut perms = std::fs::metadata(&path).unwrap().permissions();
        perms.set_mode(0o755);
        std::fs::set_permissions(&path, perms).unwrap();
    }
    bin_dir
}

#[test]
fn batch_container_item_admits_daemon_variable() {
    let dir = tempfile::tempdir().unwrap();
    let bin_dir = write_fake_docker(dir.path());

    let mut cmd = StdCommand::new(workspace_bin("oxo-flow"));
    base_env(&mut cmd, dir.path());
    cmd.env(
        "PATH",
        format!(
            "{}:{}",
            bin_dir.display(),
            std::env::var("PATH").unwrap_or_default()
        ),
    )
    .env("DOCKER_HOST", "tcp://batch-sentinel.example:2376");
    let status = cmd
        .args(["batch", "-e", "docker:fake/image:latest", "true", "x"])
        .current_dir(dir.path())
        .status()
        .unwrap();
    assert!(status.success(), "batch must succeed");

    let observed =
        std::fs::read_to_string(dir.path().join("docker_env_observed.txt")).unwrap_or_default();
    assert_eq!(
        observed, "tcp://batch-sentinel.example:2376",
        "a docker batch item must admit DOCKER_HOST (issue #1233 review finding)"
    );
}

#[test]
fn batch_non_container_item_drops_daemon_variable() {
    let dir = tempfile::tempdir().unwrap();
    let mut cmd = StdCommand::new(workspace_bin("oxo-flow"));
    base_env(&mut cmd, dir.path());
    cmd.env("DOCKER_HOST", "tcp://batch-sentinel.example:2376");
    let status = cmd
        .args([
            "batch",
            "printf '%s' \"$DOCKER_HOST\" > daemon_observed.txt",
            "x",
        ])
        .current_dir(dir.path())
        .status()
        .unwrap();
    assert!(status.success(), "batch must succeed");

    let observed = std::fs::read_to_string(dir.path().join("daemon_observed.txt")).unwrap();
    assert_eq!(
        observed, "",
        "a non-container batch shell must not observe DOCKER_HOST (bare allowlist)"
    );
}
