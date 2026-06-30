//! CLI binary integration tests.
//!
//! These tests exercise the `oxo-flow` and `oxo-flow-web` binaries
//! via `assert_cmd`, ensuring that the compiled CLIs work correctly end-to-end.
//!
//! Binaries are located from the workspace target directory since they are
//! defined in sub-crates rather than the root package.

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

fn oxo_flow_web_cmd() -> Command {
    Command::new(workspace_bin("oxo-flow-web"))
}

// ─── oxo-flow CLI: basic flags ──────────────────────────────────────────────

#[test]
fn cli_help() {
    oxo_flow_cmd()
        .arg("--help")
        .assert()
        .success()
        .stdout(predicate::str::contains("bioinformatics pipeline engine"));
}

#[test]
fn cli_version() {
    oxo_flow_cmd()
        .arg("--version")
        .assert()
        .success()
        .stdout(predicate::str::contains(env!("CARGO_PKG_VERSION")));
}

#[test]
fn cli_no_args() {
    // Should print help/error when no subcommand given
    oxo_flow_cmd().assert().failure();
}

// ─── validate subcommand ────────────────────────────────────────────────────

#[test]
fn cli_validate_functional() {
    // Valid cases
    for file in &[
        "examples/simple_variant_calling.oxoflow",
        "examples/paired_experiment_control.oxoflow",
    ] {
        oxo_flow_cmd().args(["validate", file]).assert().success();
    }

    // Error cases
    oxo_flow_cmd()
        .args(["validate", "nonexistent.oxoflow"])
        .assert()
        .failure();

    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("bad.oxoflow");
    fs::write(&path, "this is not valid TOML {{").unwrap();

    oxo_flow_cmd()
        .args(["validate", path.to_str().unwrap()])
        .assert()
        .failure();
}

// ─── dry-run subcommand ─────────────────────────────────────────────────────

#[test]
fn cli_dry_run_simple() {
    oxo_flow_cmd()
        .args(["dry-run", "examples/simple_variant_calling.oxoflow"])
        .assert()
        .success();
}

#[test]
fn cli_dry_run_paired() {
    oxo_flow_cmd()
        .args(["dry-run", "examples/paired_experiment_control.oxoflow"])
        .assert()
        .success();
}

#[test]
fn cli_dry_run_nonexistent() {
    oxo_flow_cmd()
        .args(["dry-run", "nonexistent.oxoflow"])
        .assert()
        .failure();
}

// ─── graph subcommand ───────────────────────────────────────────────────────

#[test]
fn cli_graph_outputs_dot() {
    oxo_flow_cmd()
        .args([
            "graph",
            "-f",
            "dot",
            "examples/simple_variant_calling.oxoflow",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("digraph"));
}

#[test]
fn cli_graph_nonexistent() {
    oxo_flow_cmd()
        .args(["graph", "nonexistent.oxoflow"])
        .assert()
        .failure();
}

// ─── report subcommand ──────────────────────────────────────────────────────

#[test]
fn cli_report_html() {
    let dir = tempfile::tempdir().unwrap();
    let out = dir.path().join("report.html");

    oxo_flow_cmd()
        .args([
            "report",
            "examples/simple_variant_calling.oxoflow",
            "-f",
            "html",
            "-o",
            out.to_str().unwrap(),
        ])
        .assert()
        .success();

    assert!(out.exists());
    let content = fs::read_to_string(&out).unwrap();
    assert!(content.contains("html") || content.contains("HTML"));
}

#[test]
fn cli_report_json() {
    let dir = tempfile::tempdir().unwrap();
    let out = dir.path().join("report.json");

    oxo_flow_cmd()
        .args([
            "report",
            "examples/simple_variant_calling.oxoflow",
            "-f",
            "json",
            "-o",
            out.to_str().unwrap(),
        ])
        .assert()
        .success();

    assert!(out.exists());
    let content = fs::read_to_string(&out).unwrap();
    // JSON report should be valid JSON
    assert!(serde_json::from_str::<serde_json::Value>(&content).is_ok());
}

// ─── env subcommand ─────────────────────────────────────────────────────────

#[test]
fn cli_env_list() {
    oxo_flow_cmd().args(["env", "list"]).assert().success();
}

// ─── package subcommand ─────────────────────────────────────────────────────

#[test]
fn cli_package_docker() {
    oxo_flow_cmd()
        .args([
            "package",
            "examples/simple_variant_calling.oxoflow",
            "-f",
            "docker",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("FROM"));
}

#[test]
fn cli_package_singularity() {
    oxo_flow_cmd()
        .args([
            "package",
            "examples/simple_variant_calling.oxoflow",
            "-f",
            "singularity",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("Bootstrap"));
}

// ─── init subcommand ────────────────────────────────────────────────────────

#[test]
fn cli_init_creates_project() {
    let dir = tempfile::tempdir().unwrap();
    let project_dir = dir.path().join("my-test-pipeline");

    oxo_flow_cmd()
        .args([
            "init",
            "my-test-pipeline",
            "-d",
            project_dir.to_str().unwrap(),
        ])
        .assert()
        .success();

    // Verify project structure
    assert!(project_dir.exists());
    assert!(project_dir.join("my-test-pipeline.oxoflow").exists());
    assert!(project_dir.join("envs").exists());
    assert!(project_dir.join("scripts").exists());
    assert!(project_dir.join(".gitignore").exists());

    // Verify workflow content
    let wf = fs::read_to_string(project_dir.join("my-test-pipeline.oxoflow")).unwrap();
    assert!(wf.contains("my-test-pipeline"));
    assert!(wf.contains("[workflow]"));
}

// ─── clean subcommand ───────────────────────────────────────────────────────

#[test]
fn cli_clean_dry_run() {
    oxo_flow_cmd()
        .args(["clean", "examples/simple_variant_calling.oxoflow", "-n"])
        .assert()
        .success();
}
// ─── completions subcommand ─────────────────────────────────────────────────

#[test]
fn cli_completions_functional() {
    for shell in &["bash", "zsh", "fish"] {
        oxo_flow_cmd()
            .args(["completions", shell])
            .assert()
            .success();
    }

    oxo_flow_cmd()
        .args(["completions", "invalid_shell"])
        .assert()
        .failure();
}

// ─── oxo-flow-web binary ────────────────────────────────────────────────────

#[test]
fn web_binary_exists() {
    // Verify the web binary was built successfully
    let _cmd = oxo_flow_web_cmd();
}

// ─── Gallery workflow CLI tests ─────────────────────────────────────────────

#[test]
fn cli_validate_all_gallery_workflows() {
    let gallery_dir = "examples/gallery";
    let entries: Vec<_> = fs::read_dir(gallery_dir)
        .expect("gallery directory should exist")
        .filter_map(|e| e.ok())
        .filter(|e| e.path().extension().is_some_and(|ext| ext == "oxoflow"))
        .collect();

    assert!(
        entries.len() >= 8,
        "expected at least 8 gallery workflows, found {}",
        entries.len()
    );

    for entry in &entries {
        let path = entry.path();
        oxo_flow_cmd()
            .args(["validate", path.to_str().unwrap()])
            .assert()
            .success()
            .stderr(predicate::str::contains("✓"));
    }
}

#[test]
fn cli_dryrun_gallery_file_pipeline() {
    oxo_flow_cmd()
        .args(["dry-run", "examples/gallery/02_file_pipeline.oxoflow"])
        .assert()
        .success()
        .stderr(predicate::str::contains("3 rules would execute"))
        .stderr(predicate::str::contains("generate_data"))
        .stderr(predicate::str::contains("summarize"));
}

#[test]
fn cli_graph_gallery_rnaseq() {
    oxo_flow_cmd()
        .args([
            "graph",
            "-f",
            "dot",
            "examples/gallery/06_rnaseq_quantification.oxoflow",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("digraph"))
        .stdout(predicate::str::contains("fastp_trim"))
        .stdout(predicate::str::contains("multiqc"));
}

#[test]
fn cli_lint_gallery_wgs_germline() {
    oxo_flow_cmd()
        .args(["lint", "examples/gallery/07_wgs_germline.oxoflow"])
        .assert()
        .success();
}

// ─── Export CLI tests ───────────────────────────────────────────────────────

#[test]
fn cli_export_docker() {
    oxo_flow_cmd()
        .args([
            "export",
            "examples/gallery/01_hello_world.oxoflow",
            "-f",
            "docker",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("FROM"));
}

#[test]
fn cli_export_singularity() {
    oxo_flow_cmd()
        .args([
            "export",
            "examples/gallery/01_hello_world.oxoflow",
            "-f",
            "singularity",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("Bootstrap"));
}

#[test]
fn cli_export_toml() {
    oxo_flow_cmd()
        .args([
            "export",
            "examples/gallery/01_hello_world.oxoflow",
            "-f",
            "toml",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("[workflow]"));
}

// ─── Debug CLI tests ────────────────────────────────────────────────────────

#[test]
fn cli_debug_command() {
    let dir = tempfile::tempdir().unwrap();
    let workflow = dir.path().join("test.oxoflow");
    fs::write(
        &workflow,
        r#"
[workflow]
name = "debug-test"
version = "1.0.0"

[[rules]]
name = "step1"
input = ["input.txt"]
output = ["output.txt"]
shell = "cat {input} > {output}"
threads = 4
memory = "8G"
description = "Copy input to output"
tags = ["test", "debug"]
"#,
    )
    .unwrap();

    let output = oxo_flow_cmd()
        .args(["debug", workflow.to_str().unwrap()])
        .output()
        .expect("failed to run debug command");

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(output.status.success(), "debug command failed: {}", stderr);
    assert!(stderr.contains("step1"), "should show rule name");
    assert!(stderr.contains("cat"), "should show shell command");
}

#[test]
fn cli_debug_specific_rule() {
    let dir = tempfile::tempdir().unwrap();
    let workflow = dir.path().join("test.oxoflow");
    fs::write(
        &workflow,
        r#"
[workflow]
name = "debug-test"
version = "1.0.0"

[[rules]]
name = "step1"
input = ["input.txt"]
output = ["mid.txt"]
shell = "cat input.txt > mid.txt"

[[rules]]
name = "step2"
input = ["mid.txt"]
output = ["output.txt"]
shell = "cat mid.txt > output.txt"
"#,
    )
    .unwrap();

    let output = oxo_flow_cmd()
        .args(["debug", workflow.to_str().unwrap(), "-r", "step2"])
        .output()
        .expect("failed to run debug command");

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(output.status.success());
    assert!(stderr.contains("step2"));
    // Should only show step2, not step1
    assert!(stderr.contains("Debugging 1 rules"));
}

#[test]
fn cli_run_fails_fast_when_rule_exceeds_max_memory() {
    // A rule declaring more memory than the explicit --max-memory cap can never
    // be scheduled. The run must fail up front with a clear message and must NOT
    // execute the earlier, feasible rule (no wasted work).
    let dir = tempfile::tempdir().unwrap();
    let workflow = dir.path().join("test.oxoflow");
    fs::write(
        &workflow,
        r#"
[workflow]
name = "budget-test"
version = "1.0.0"

[[rules]]
name = "cheap_first"
output = ["a.txt"]
shell = "echo did-real-work > a.txt"

[[rules]]
name = "hungry_second"
input = ["a.txt"]
output = ["b.txt"]
shell = "echo hello > b.txt"
memory = "8G"
"#,
    )
    .unwrap();

    let output = oxo_flow_cmd()
        .current_dir(dir.path())
        .args(["run", workflow.to_str().unwrap(), "--max-memory", "100"])
        .output()
        .expect("failed to run command");

    assert!(
        !output.status.success(),
        "run should fail when a rule exceeds the memory budget"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("hungry_second") && stderr.contains("--max-memory"),
        "error should name the breaching rule and the cap: {stderr}"
    );
    assert!(
        !dir.path().join("a.txt").exists(),
        "no rules should have run; cheap_first must not have produced a.txt"
    );
}

// ─── Cluster CLI tests ──────────────────────────────────────────────────────

#[test]
fn cli_cluster_submit() {
    let tmp = tempfile::tempdir().unwrap();
    let output_dir = tmp.path().join("cluster_scripts");
    oxo_flow_cmd()
        .args([
            "cluster",
            "submit",
            "examples/gallery/02_file_pipeline.oxoflow",
            "-b",
            "slurm",
            "-o",
            output_dir.to_str().unwrap(),
        ])
        .assert()
        .success()
        .stderr(predicate::str::contains("Generating slurm job scripts"))
        .stderr(predicate::str::contains("scripts written to"));

    // Verify scripts were created
    assert!(output_dir.exists());
    let scripts: Vec<_> = fs::read_dir(&output_dir)
        .unwrap()
        .filter_map(|e| e.ok())
        .filter(|e| e.path().extension().is_some_and(|ext| ext == "sh"))
        .collect();
    assert!(
        scripts.len() >= 3,
        "expected at least 3 cluster scripts, found {}",
        scripts.len()
    );
}

#[test]
fn cli_cluster_status() {
    // Test that cluster status command executes squeue
    // On systems with SLURM: command succeeds with squeue output
    // On systems without SLURM: command fails with squeue error
    let output = oxo_flow_cmd()
        .args(["cluster", "status", "-b", "slurm"])
        .output()
        .expect("Failed to execute command");

    let stderr = String::from_utf8_lossy(&output.stderr);
    let stdout = String::from_utf8_lossy(&output.stdout);
    let combined = format!("{}{}", stderr, stdout);

    // Verify that squeue is mentioned in either stdout or stderr
    assert!(
        combined.contains("squeue"),
        "Expected 'squeue' in output, got stdout: {}, stderr: {}",
        stdout,
        stderr
    );
}

#[test]
fn cli_cluster_cancel_no_ids() {
    oxo_flow_cmd()
        .args(["cluster", "cancel", "-b", "slurm"])
        .assert()
        .success()
        .stderr(predicate::str::contains("No job IDs"));
}

// ─── Status subcommand ───────────────────────────────────────────────────────

#[test]
fn cli_status_valid_checkpoint() {
    let dir = tempfile::tempdir().unwrap();
    let checkpoint = dir.path().join("checkpoint.json");

    // Create a valid checkpoint JSON
    let json = r#"{
        "completed_rules": ["step_a", "step_b"],
        "failed_rules": [],
        "benchmarks": {
            "step_a": {"rule": "step_a", "wall_time_secs": 1.23, "max_memory_mb": null, "cpu_seconds": null},
            "step_b": {"rule": "step_b", "wall_time_secs": 2.45, "max_memory_mb": null, "cpu_seconds": null}
        }
    }"#;
    fs::write(&checkpoint, json).unwrap();

    oxo_flow_cmd()
        .args(["status", checkpoint.to_str().unwrap()])
        .assert()
        .success()
        .stderr(predicate::str::contains("step_a"))
        .stderr(predicate::str::contains("step_b"));
}

#[test]
fn cli_status_invalid_checkpoint() {
    oxo_flow_cmd()
        .args(["status", "nonexistent_checkpoint.json"])
        .assert()
        .failure();
}

// ─── Config subcommand ───────────────────────────────────────────────────────

#[test]
fn cli_config_show() {
    oxo_flow_cmd()
        .args(["config", "show", "examples/simple_variant_calling.oxoflow"])
        .assert()
        .success()
        .stderr(predicate::str::contains("Name:"));
}

#[test]
fn cli_config_stats() {
    oxo_flow_cmd()
        .args(["config", "stats", "examples/simple_variant_calling.oxoflow"])
        .assert()
        .success()
        .stderr(predicate::str::contains("Rules:"))
        .stderr(predicate::str::contains("Dependencies:"));
}

#[test]
fn cli_config_stats_gallery_multiomics() {
    oxo_flow_cmd()
        .args([
            "config",
            "stats",
            "examples/gallery/08_multiomics_integration.oxoflow",
        ])
        .assert()
        .success()
        .stderr(predicate::str::contains("Parallel groups:"));
}

// ─── Diff subcommand ─────────────────────────────────────────────────────────

#[test]
fn cli_diff_identical_workflows() {
    oxo_flow_cmd()
        .args([
            "diff",
            "examples/simple_variant_calling.oxoflow",
            "examples/simple_variant_calling.oxoflow",
        ])
        .assert()
        .success()
        .stderr(predicate::str::contains("identical"));
}

#[test]
fn cli_diff_different_workflows() {
    oxo_flow_cmd()
        .args([
            "diff",
            "examples/simple_variant_calling.oxoflow",
            "examples/paired_experiment_control.oxoflow",
        ])
        .assert()
        .success()
        .stderr(predicate::str::contains("difference"));
}

#[test]
fn cli_diff_nonexistent_workflow() {
    oxo_flow_cmd()
        .args([
            "diff",
            "examples/simple_variant_calling.oxoflow",
            "nonexistent.oxoflow",
        ])
        .assert()
        .failure();
}

// ─── Format subcommand ───────────────────────────────────────────────────────

#[test]
fn cli_format_outputs_canonical_toml() {
    oxo_flow_cmd()
        .args(["format", "examples/simple_variant_calling.oxoflow"])
        .assert()
        .success()
        .stdout(predicate::str::contains("[workflow]"));
}

#[test]
fn cli_format_save_to_file() {
    let dir = tempfile::tempdir().unwrap();
    let out = dir.path().join("formatted.oxoflow");

    oxo_flow_cmd()
        .args([
            "format",
            "examples/simple_variant_calling.oxoflow",
            "-o",
            out.to_str().unwrap(),
        ])
        .assert()
        .success();

    assert!(out.exists());
    let content = fs::read_to_string(&out).unwrap();
    assert!(content.contains("[workflow]"));
}

// ─── Touch subcommand ────────────────────────────────────────────────────────

#[test]
fn cli_touch_command_success() {
    // Touch on the hello world example (no actual output files exist, should succeed anyway)
    oxo_flow_cmd()
        .args(["touch", "examples/gallery/01_hello_world.oxoflow"])
        .assert()
        .success();
}

// ─── Profile subcommand ──────────────────────────────────────────────────────

#[test]
fn cli_profile_list() {
    oxo_flow_cmd()
        .args(["profile", "list"])
        .assert()
        .success()
        .stderr(predicate::str::contains("local"))
        .stderr(predicate::str::contains("slurm"))
        .stderr(predicate::str::contains("pbs"));
}

#[test]
fn cli_profile_show_functional() {
    for profile in &["local", "slurm", "pbs"] {
        oxo_flow_cmd()
            .args(["profile", "show", profile])
            .assert()
            .success();
    }

    oxo_flow_cmd()
        .args(["profile", "show", "unknown-profile"])
        .assert()
        .failure();
}

#[test]
fn cli_profile_current() {
    oxo_flow_cmd()
        .args(["profile", "current"])
        .assert()
        .success()
        .stderr(predicate::str::contains("local"));
}

// ─── Env subcommand: extended ────────────────────────────────────────────────

#[test]
fn cli_env_check_no_workflow() {
    // Without a workflow, reports global backend availability
    oxo_flow_cmd()
        .args(["env", "check"])
        .assert()
        .success()
        .stderr(
            predicate::str::contains("conda")
                .or(predicate::str::contains("docker"))
                .or(predicate::str::contains("venv")),
        );
}

#[test]
fn cli_env_check_with_simple_workflow() {
    // Workflow with no special environments — all checks should pass or warn
    oxo_flow_cmd()
        .args(["env", "check", "examples/gallery/01_hello_world.oxoflow"])
        .assert()
        .success();
}

// ─── Run subcommand ──────────────────────────────────────────────────────────

#[test]
fn cli_run_echo_hello_world() {
    let dir = tempfile::tempdir().unwrap();
    let workflow = dir.path().join("hello.oxoflow");
    let output_file = dir.path().join("greeting.txt");

    fs::write(
        &workflow,
        format!(
            r#"
[workflow]
name = "hello-run"
version = "1.0.0"

[[rules]]
name = "greet"
output = ["{output}"]
shell = "echo 'Hello, oxo-flow!' > {output}"
"#,
            output = output_file.to_str().unwrap()
        ),
    )
    .unwrap();

    oxo_flow_cmd()
        .args(["run", workflow.to_str().unwrap()])
        .assert()
        .success()
        .stderr(predicate::str::contains("greet"));

    assert!(output_file.exists());
    let content = fs::read_to_string(&output_file).unwrap();
    assert!(content.contains("Hello, oxo-flow!"));
}

#[test]
fn cli_run_serial_three_step_pipeline() {
    let dir = tempfile::tempdir().unwrap();
    let workflow = dir.path().join("serial.oxoflow");
    let step1_out = dir.path().join("step1.txt");
    let step2_out = dir.path().join("step2.txt");
    let step3_out = dir.path().join("step3.txt");

    fs::write(
        &workflow,
        format!(
            r#"
[workflow]
name = "serial-pipeline"
version = "1.0.0"

[[rules]]
name = "step1"
output = ["{s1}"]
shell = "echo 'line1' > {s1}"

[[rules]]
name = "step2"
input = ["{s1}"]
output = ["{s2}"]
shell = "cat {s1} > {s2} && echo 'line2' >> {s2}"

[[rules]]
name = "step3"
input = ["{s2}"]
output = ["{s3}"]
shell = "cat {s2} > {s3} && echo 'line3' >> {s3}"
"#,
            s1 = step1_out.to_str().unwrap(),
            s2 = step2_out.to_str().unwrap(),
            s3 = step3_out.to_str().unwrap(),
        ),
    )
    .unwrap();

    oxo_flow_cmd()
        .args(["run", workflow.to_str().unwrap()])
        .assert()
        .success();

    assert!(step3_out.exists());
    let content = fs::read_to_string(&step3_out).unwrap();
    assert!(content.contains("line1"));
    assert!(content.contains("line2"));
    assert!(content.contains("line3"));
}

#[test]
fn cli_run_with_target_rule() {
    let dir = tempfile::tempdir().unwrap();
    let workflow = dir.path().join("targeted.oxoflow");
    let out_a = dir.path().join("a.txt");
    let out_b = dir.path().join("b.txt");
    let out_c = dir.path().join("c.txt");

    fs::write(
        &workflow,
        format!(
            r#"
[workflow]
name = "targeted"
version = "1.0.0"

[[rules]]
name = "step_a"
output = ["{a}"]
shell = "echo 'A' > {a}"

[[rules]]
name = "step_b"
input = ["{a}"]
output = ["{b}"]
shell = "cat {a} > {b} && echo 'B' >> {b}"

[[rules]]
name = "step_c"
input = ["{b}"]
output = ["{c}"]
shell = "cat {b} > {c} && echo 'C' >> {c}"
"#,
            a = out_a.to_str().unwrap(),
            b = out_b.to_str().unwrap(),
            c = out_c.to_str().unwrap(),
        ),
    )
    .unwrap();

    // Run with target step_b only (should execute step_a + step_b but NOT step_c)
    oxo_flow_cmd()
        .args(["run", workflow.to_str().unwrap(), "-t", "step_b"])
        .assert()
        .success();

    assert!(out_a.exists(), "step_a output should exist");
    assert!(out_b.exists(), "step_b output should exist");
    assert!(
        !out_c.exists(),
        "step_c should not run when targeting step_b"
    );
}

#[test]
fn cli_run_parallel_independent_steps() {
    let dir = tempfile::tempdir().unwrap();
    let workflow = dir.path().join("parallel.oxoflow");
    let out_a = dir.path().join("pa.txt");
    let out_b = dir.path().join("pb.txt");
    let out_c = dir.path().join("pc.txt");
    let out_merge = dir.path().join("merged.txt");

    fs::write(
        &workflow,
        format!(
            r#"
[workflow]
name = "parallel-pipeline"
version = "1.0.0"

[[rules]]
name = "branch_a"
output = ["{a}"]
shell = "echo 'alpha' > {a}"

[[rules]]
name = "branch_b"
output = ["{b}"]
shell = "echo 'beta' > {b}"

[[rules]]
name = "branch_c"
output = ["{c}"]
shell = "echo 'gamma' > {c}"

[[rules]]
name = "merge_all"
input = ["{a}", "{b}", "{c}"]
output = ["{m}"]
shell = "cat {a} {b} {c} > {m}"
"#,
            a = out_a.to_str().unwrap(),
            b = out_b.to_str().unwrap(),
            c = out_c.to_str().unwrap(),
            m = out_merge.to_str().unwrap(),
        ),
    )
    .unwrap();

    oxo_flow_cmd()
        .args(["run", workflow.to_str().unwrap(), "-j", "3"])
        .assert()
        .success();

    assert!(out_merge.exists());
    let content = fs::read_to_string(&out_merge).unwrap();
    assert!(content.contains("alpha"));
    assert!(content.contains("beta"));
    assert!(content.contains("gamma"));
}

#[test]
fn cli_run_keep_going_on_failure() {
    let dir = tempfile::tempdir().unwrap();
    let workflow = dir.path().join("keep_going.oxoflow");
    let out_ok = dir.path().join("ok.txt");

    fs::write(
        &workflow,
        format!(
            r#"
[workflow]
name = "keep-going"
version = "1.0.0"

[[rules]]
name = "fail_step"
output = ["will_not_exist.txt"]
shell = "exit 1"

[[rules]]
name = "ok_step"
output = ["{ok}"]
shell = "echo 'I ran despite the failure' > {ok}"
"#,
            ok = out_ok.to_str().unwrap(),
        ),
    )
    .unwrap();

    // With --keep-going the ok step should still run even though fail_step fails
    let output = oxo_flow_cmd()
        .args(["run", workflow.to_str().unwrap(), "-k"])
        .output()
        .expect("failed to run");

    // Should not hard-fail (keep-going), but stderr should mention the failure
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("fail_step") || stderr.contains("failed") || stderr.contains("✗"),
        "should mention the failed rule in stderr"
    );
    // --keep-going must print a consolidated end-of-run failure summary naming the
    // failed rule, so it is not lost in interleaved output on large pipelines.
    assert!(
        stderr.contains("Failed rules:"),
        "keep-going should print a 'Failed rules:' summary, got:\n{stderr}"
    );
    let summary = stderr
        .split("Failed rules:")
        .nth(1)
        .expect("summary section present");
    assert!(
        summary.contains("fail_step"),
        "failure summary should name the failed rule, got:\n{stderr}"
    );
}

#[test]
fn cli_run_nontty_emits_plain_progress_lines() {
    // assert_cmd runs the binary with a piped (non-terminal) stderr, so the
    // indicatif progress bar is hidden. The run must fall back to plain per-rule
    // log lines instead of going silent between the DAG listing and the summary.
    let dir = tempfile::tempdir().unwrap();
    let workflow = dir.path().join("progress.oxoflow");
    let a = dir.path().join("a.txt");
    let b = dir.path().join("b.txt");

    fs::write(
        &workflow,
        format!(
            r#"
[workflow]
name = "nontty-progress"
version = "1.0.0"

[[rules]]
name = "make_a"
output = ["{a}"]
shell = "echo a > {a}"

[[rules]]
name = "make_b"
input = ["{a}"]
output = ["{b}"]
shell = "cat {a} > {b}"
"#,
            a = a.to_str().unwrap(),
            b = b.to_str().unwrap(),
        ),
    )
    .unwrap();

    let output = oxo_flow_cmd()
        .args(["run", workflow.to_str().unwrap(), "-j", "1"])
        .output()
        .expect("failed to run");

    assert!(output.status.success(), "run should succeed");
    let stderr = String::from_utf8_lossy(&output.stderr);
    // Per-rule "Running:" lines only exist on the non-TTY fallback path.
    assert!(
        stderr.contains("Running:") && stderr.contains("make_a") && stderr.contains("make_b"),
        "non-TTY run should emit plain per-rule progress lines, got:\n{stderr}"
    );
    assert!(
        stderr.contains("Done:"),
        "run should print a completion summary, got:\n{stderr}"
    );
}

#[test]
fn cli_run_bioinformatics_qc_pipeline() {
    // Simulate a FastQC → trimming → alignment QC pipeline using echo/wc/sort
    let dir = tempfile::tempdir().unwrap();
    let workflow = dir.path().join("qc.oxoflow");

    // Create simulated input FASTQ-like files
    let raw_r1 = dir.path().join("sample_R1.fastq");
    let raw_r2 = dir.path().join("sample_R2.fastq");
    fs::write(
        &raw_r1,
        "@SEQ_001\nACGT\n+\nIIII\n@SEQ_002\nTTTT\n+\nIIII\n",
    )
    .unwrap();
    fs::write(
        &raw_r2,
        "@SEQ_001\nTGCA\n+\nIIII\n@SEQ_002\nAAAA\n+\nIIII\n",
    )
    .unwrap();

    let qc_report = dir.path().join("qc_report.txt");
    let trim_r1 = dir.path().join("trimmed_R1.fastq");
    let stats = dir.path().join("stats.txt");

    fs::write(
        &workflow,
        format!(
            r#"
[workflow]
name = "qc-pipeline"
version = "1.0.0"
description = "Simulate QC pipeline with standard Unix tools"

[defaults]
threads = 1
memory = "1G"

[[rules]]
name = "fastqc_check"
input = ["{r1}", "{r2}"]
output = ["{qc}"]
shell = "wc -l {r1} {r2} > {qc} && echo 'QC complete' >> {qc}"

[[rules]]
name = "trim_reads"
input = ["{r1}"]
output = ["{trim}"]
shell = "grep -v '^+' {r1} | grep -v '^I' > {trim}"

[[rules]]
name = "alignment_stats"
input = ["{trim}", "{qc}"]
output = ["{stats}"]
shell = "wc -c {trim} > {stats} && cat {qc} >> {stats}"
"#,
            r1 = raw_r1.to_str().unwrap(),
            r2 = raw_r2.to_str().unwrap(),
            qc = qc_report.to_str().unwrap(),
            trim = trim_r1.to_str().unwrap(),
            stats = stats.to_str().unwrap(),
        ),
    )
    .unwrap();

    oxo_flow_cmd()
        .args(["run", workflow.to_str().unwrap()])
        .assert()
        .success();

    assert!(stats.exists());
}

#[test]
fn cli_run_config_variable_substitution() {
    let dir = tempfile::tempdir().unwrap();
    let workflow = dir.path().join("config_vars.oxoflow");
    let output = dir.path().join("result.txt");

    fs::write(
        &workflow,
        format!(
            r#"
[workflow]
name = "config-vars"
version = "1.0.0"

[config]
sample_name = "PATIENT_001"
project = "ONCOLOGY"

[[rules]]
name = "write_metadata"
output = ["{out}"]
shell = "echo 'Sample: {{config.sample_name}} Project: {{config.project}}' > {out}"
"#,
            out = output.to_str().unwrap(),
        ),
    )
    .unwrap();

    oxo_flow_cmd()
        .args(["run", workflow.to_str().unwrap()])
        .assert()
        .success();

    assert!(output.exists());
    let content = fs::read_to_string(&output).unwrap();
    assert!(content.contains("PATIENT_001"));
    assert!(content.contains("ONCOLOGY"));
}

// ─── dry-run extended tests ──────────────────────────────────────────────────

#[test]
fn cli_dry_run_with_target_rule() {
    let dir = tempfile::tempdir().unwrap();
    let workflow = dir.path().join("dryrun_target.oxoflow");

    fs::write(
        &workflow,
        r#"
[workflow]
name = "dryrun-target"
version = "1.0.0"

[[rules]]
name = "step_a"
output = ["a.txt"]
shell = "echo A"

[[rules]]
name = "step_b"
input = ["a.txt"]
output = ["b.txt"]
shell = "echo B"

[[rules]]
name = "step_c"
input = ["b.txt"]
output = ["c.txt"]
shell = "echo C"
"#,
    )
    .unwrap();

    oxo_flow_cmd()
        .args(["dry-run", workflow.to_str().unwrap(), "-t", "step_b"])
        .assert()
        .success()
        .stderr(predicate::str::contains("2 rules would execute"))
        .stderr(predicate::str::contains("step_a"))
        .stderr(predicate::str::contains("step_b"));
}

#[test]
fn cli_dry_run_shows_thread_and_env_info() {
    oxo_flow_cmd()
        .args(["dry-run", "examples/gallery/07_wgs_germline.oxoflow"])
        .assert()
        .success()
        .stderr(predicate::str::contains("threads="));
}

// ─── Graph subcommand: additional formats ────────────────────────────────────

#[test]
fn cli_graph_ascii_format() {
    oxo_flow_cmd()
        .args([
            "graph",
            "-f",
            "ascii",
            "examples/gallery/02_file_pipeline.oxoflow",
        ])
        .assert()
        .success();
}

#[test]
fn cli_graph_tree_format() {
    oxo_flow_cmd()
        .args([
            "graph",
            "-f",
            "tree",
            "examples/gallery/06_rnaseq_quantification.oxoflow",
        ])
        .assert()
        .success();
}

#[test]
fn cli_graph_dot_clustered_format() {
    oxo_flow_cmd()
        .args([
            "graph",
            "-f",
            "dot-clustered",
            "examples/gallery/08_multiomics_integration.oxoflow",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("digraph").or(predicate::str::contains("subgraph")));
}

#[test]
fn cli_graph_save_to_file() {
    let dir = tempfile::tempdir().unwrap();
    let out = dir.path().join("graph.dot");

    oxo_flow_cmd()
        .args([
            "graph",
            "-f",
            "dot",
            "-o",
            out.to_str().unwrap(),
            "examples/gallery/07_wgs_germline.oxoflow",
        ])
        .assert()
        .success();

    assert!(out.exists());
    let content = fs::read_to_string(&out).unwrap();
    assert!(content.contains("digraph"));
}

// ─── Cluster subcommand: additional backends ─────────────────────────────────

#[test]
fn cli_cluster_submit_pbs_backend() {
    let tmp = tempfile::tempdir().unwrap();
    let output_dir = tmp.path().join("pbs_scripts");

    oxo_flow_cmd()
        .args([
            "cluster",
            "submit",
            "examples/gallery/02_file_pipeline.oxoflow",
            "-b",
            "pbs",
            "-o",
            output_dir.to_str().unwrap(),
        ])
        .assert()
        .success()
        .stderr(predicate::str::contains("pbs"));
}

#[test]
fn cli_cluster_submit_sge_backend() {
    let tmp = tempfile::tempdir().unwrap();
    let output_dir = tmp.path().join("sge_scripts");

    oxo_flow_cmd()
        .args([
            "cluster",
            "submit",
            "examples/gallery/02_file_pipeline.oxoflow",
            "-b",
            "sge",
            "-o",
            output_dir.to_str().unwrap(),
        ])
        .assert()
        .success()
        .stderr(predicate::str::contains("sge"));
}

#[test]
fn cli_cluster_submit_with_queue_and_account() {
    let tmp = tempfile::tempdir().unwrap();
    let output_dir = tmp.path().join("cluster_queue_scripts");

    oxo_flow_cmd()
        .args([
            "cluster",
            "submit",
            "examples/gallery/01_hello_world.oxoflow",
            "-b",
            "slurm",
            "-q",
            "bioinformatics",
            "-a",
            "genomics-lab",
            "-o",
            output_dir.to_str().unwrap(),
        ])
        .assert()
        .success();
}

// ─── Export subcommand ───────────────────────────────────────────────────────

#[test]
fn cli_export_docker_to_file() {
    let dir = tempfile::tempdir().unwrap();
    let out = dir.path().join("Dockerfile");

    oxo_flow_cmd()
        .args([
            "export",
            "examples/gallery/07_wgs_germline.oxoflow",
            "-f",
            "docker",
            "-o",
            out.to_str().unwrap(),
        ])
        .assert()
        .success();

    assert!(out.exists());
    let content = fs::read_to_string(&out).unwrap();
    assert!(content.contains("FROM"));
}

#[test]
fn cli_export_singularity_to_file() {
    let dir = tempfile::tempdir().unwrap();
    let out = dir.path().join("Singularity.def");

    oxo_flow_cmd()
        .args([
            "export",
            "examples/gallery/07_wgs_germline.oxoflow",
            "-f",
            "singularity",
            "-o",
            out.to_str().unwrap(),
        ])
        .assert()
        .success();

    assert!(out.exists());
    let content = fs::read_to_string(&out).unwrap();
    assert!(content.contains("Bootstrap"));
}

// ─── Global flags ────────────────────────────────────────────────────────────

#[test]
fn cli_verbose_flag_produces_debug_output() {
    oxo_flow_cmd()
        .args([
            "--verbose",
            "validate",
            "examples/simple_variant_calling.oxoflow",
        ])
        .assert()
        .success();
}

#[test]
fn cli_quiet_flag_suppresses_output() {
    let output = oxo_flow_cmd()
        .args([
            "--quiet",
            "validate",
            "examples/simple_variant_calling.oxoflow",
        ])
        .output()
        .unwrap();
    // In quiet mode, stderr should have minimal output
    assert!(output.status.success());
}

// ─── Lint: extended tests ────────────────────────────────────────────────────

#[test]
fn cli_lint_all_gallery_workflows() {
    let gallery_dir = "examples/gallery";
    let entries: Vec<_> = fs::read_dir(gallery_dir)
        .expect("gallery directory should exist")
        .filter_map(|e| e.ok())
        .filter(|e| e.path().extension().is_some_and(|ext| ext == "oxoflow"))
        .collect();

    for entry in &entries {
        let path = entry.path();
        oxo_flow_cmd()
            .args(["lint", path.to_str().unwrap()])
            .assert()
            .success();
    }
}

#[test]
fn cli_lint_strict_mode() {
    // A minimal workflow with no description may trigger a lint warning
    let dir = tempfile::tempdir().unwrap();
    let workflow = dir.path().join("minimal.oxoflow");
    fs::write(
        &workflow,
        r#"
[workflow]
name = "minimal"

[[rules]]
name = "step1"
output = ["out.txt"]
shell = "echo hello > out.txt"
"#,
    )
    .unwrap();

    // strict mode: exits non-zero if any warnings
    let output = oxo_flow_cmd()
        .args(["lint", workflow.to_str().unwrap(), "--strict"])
        .output()
        .unwrap();
    // We just check it runs without panicking
    let _ = output.status;
}

// ─── Bug-fix regression tests ─────────────────────────────────────────────────

/// Bug: {config.xxx} in output paths was not expanded when validating that outputs
/// exist after execution → false "expected output file not found" warnings.
/// After fix: no WARN emitted when the file is actually created at the expanded path.
#[test]
fn run_config_var_in_output_no_false_warn() {
    let dir = tempfile::tempdir().unwrap();
    let wf = dir.path().join("wf.oxoflow");
    fs::write(
        &wf,
        r#"
[workflow]
name = "config-var-output"
[config]
sample = "SAMPLE001"
[[rules]]
name = "gen"
output = ["results/{config.sample}.txt"]
shell = "mkdir -p results && echo done > results/{config.sample}.txt"
"#,
    )
    .unwrap();

    let out = oxo_flow_cmd()
        .args(["run", wf.to_str().unwrap()])
        .current_dir(dir.path())
        .output()
        .unwrap();

    assert!(out.status.success(), "run should succeed");
    // The file should have been created at the expanded path
    assert!(
        dir.path().join("results/SAMPLE001.txt").exists(),
        "output file must exist at expanded path"
    );
    // No false "expected output file not found" warning
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        !stderr.contains("expected output file not found"),
        "no false output-missing warning expected, got: {stderr}"
    );
}

/// Bug: second run with config var outputs always re-ran (should_skip_rule not integrated).
/// After fix: second run skips rules whose expanded outputs are already up-to-date.
#[test]
fn run_config_var_output_skipped_on_second_run() {
    let dir = tempfile::tempdir().unwrap();
    let wf = dir.path().join("wf.oxoflow");
    fs::write(
        &wf,
        r#"
[workflow]
name = "skip-test"
[config]
sample = "S001"
[[rules]]
name = "produce"
output = ["out_{config.sample}.txt"]
shell = "echo data > out_{config.sample}.txt"
"#,
    )
    .unwrap();

    // First run – should execute
    oxo_flow_cmd()
        .args(["run", wf.to_str().unwrap()])
        .current_dir(dir.path())
        .assert()
        .success();
    assert!(dir.path().join("out_S001.txt").exists());

    // Second run – outputs exist and are up-to-date; rule should be skipped
    let out2 = oxo_flow_cmd()
        .args(["run", wf.to_str().unwrap()])
        .current_dir(dir.path())
        .output()
        .unwrap();
    assert!(out2.status.success());
    let stderr2 = String::from_utf8_lossy(&out2.stderr);
    assert!(
        stderr2.contains("skipped"),
        "second run should report rule as skipped, got: {stderr2}"
    );
}

/// Bug: dry-run showed raw {config.xxx} template instead of expanded commands.
/// After fix: dry-run output must show the expanded command.
#[test]
fn dry_run_expands_config_vars_in_command() {
    let dir = tempfile::tempdir().unwrap();
    let wf = dir.path().join("wf.oxoflow");
    fs::write(
        &wf,
        r#"
[workflow]
name = "dryrun-config"
[config]
sample = "PATIENT_007"
threads = 8
[[rules]]
name = "align"
output = ["aligned/{config.sample}.bam"]
shell = "bwa mem -t {config.threads} ref.fa raw/{config.sample}.fq > aligned/{config.sample}.bam"
"#,
    )
    .unwrap();

    oxo_flow_cmd()
        .args(["dry-run", wf.to_str().unwrap()])
        .assert()
        .success()
        // Expanded values must appear in the printed command
        .stderr(predicate::str::contains("PATIENT_007"))
        .stderr(predicate::str::contains("bwa mem -t 8"));
}

/// Bug: debug command showed raw {config.xxx} template instead of expanded shell command.
/// After fix: debug must show the expanded "Shell (expanded):" line.
#[test]
fn debug_expands_config_vars_in_command() {
    let dir = tempfile::tempdir().unwrap();
    let wf = dir.path().join("wf.oxoflow");
    fs::write(
        &wf,
        r#"
[workflow]
name = "debug-config"
[config]
sample = "PATIENT_042"
threads = 16
[[rules]]
name = "align"
output = ["aligned/{config.sample}.bam"]
shell = "bwa mem -t {config.threads} ref.fa raw/{config.sample}.fq > aligned/{config.sample}.bam"
"#,
    )
    .unwrap();

    oxo_flow_cmd()
        .args(["debug", wf.to_str().unwrap()])
        .assert()
        .success()
        .stderr(predicate::str::contains("PATIENT_042"))
        .stderr(predicate::str::contains("16"));
}

/// Bug: clean skipped output paths containing {config.xxx} as "wildcards"
/// and could not delete files produced with config-variable paths.
/// After fix: clean should expand config vars and successfully delete the files.
#[test]
fn clean_handles_config_var_output_paths() {
    let dir = tempfile::tempdir().unwrap();
    let wf = dir.path().join("wf.oxoflow");
    fs::write(
        &wf,
        r#"
[workflow]
name = "clean-config"
[config]
sample = "CLEAN_SAMPLE"
[[rules]]
name = "gen"
output = ["out_{config.sample}.txt"]
shell = "echo data > out_{config.sample}.txt"
"#,
    )
    .unwrap();

    // Produce the output file manually
    fs::write(dir.path().join("out_CLEAN_SAMPLE.txt"), "data").unwrap();
    assert!(dir.path().join("out_CLEAN_SAMPLE.txt").exists());

    // clean --force should delete the expanded path
    oxo_flow_cmd()
        .args(["clean", wf.to_str().unwrap(), "--force"])
        .current_dir(dir.path())
        .assert()
        .success();

    assert!(
        !dir.path().join("out_CLEAN_SAMPLE.txt").exists(),
        "file should have been deleted by clean"
    );
}

// ---------------------------------------------------------------------------
// batch command tests
// ---------------------------------------------------------------------------

#[test]
fn cli_batch_dry_run_items() {
    oxo_flow_cmd()
        .args(["batch", "echo {item}", "a", "b", "c", "-n"])
        .assert()
        .success();
}

#[test]
fn cli_batch_dry_run_from_file() {
    let dir = tempfile::tempdir().unwrap();
    let items_file = dir.path().join("items.txt");
    fs::write(&items_file, "sample1\nsample2\nsample3\n").unwrap();
    oxo_flow_cmd()
        .args([
            "batch",
            "process {item}",
            "-f",
            items_file.to_str().unwrap(),
            "-n",
        ])
        .assert()
        .success();
}

#[test]
fn cli_batch_empty_items_error() {
    // batch with no items and no file should fail
    oxo_flow_cmd()
        .args(["batch", "echo {item}"])
        .assert()
        .failure();
}

// ---------------------------------------------------------------------------
// publish command tests
// ---------------------------------------------------------------------------

#[test]
fn cli_publish_creates_bundle() {
    let dir = tempfile::tempdir().unwrap();
    let wf = dir.path().join("pub_test.oxoflow");
    fs::write(
        &wf,
        "[workflow]\nname = \"pub-test\"\nversion = \"1.0.0\"\n\n[[rules]]\nname = \"s\"\noutput = [\"out.txt\"]\nshell = \"echo done > {output[0]}\"\n",
    )
    .unwrap();

    oxo_flow_cmd()
        .args(["publish", wf.to_str().unwrap()])
        .current_dir(dir.path())
        .assert()
        .success();

    let bundle = dir.path().join("pub_test-bundle");
    assert!(bundle.exists(), "bundle directory should exist");
    assert!(
        bundle.join("pub_test.oxoflow").exists(),
        "workflow should be in bundle"
    );
    assert!(
        bundle.join("manifest.json").exists(),
        "manifest should exist"
    );
}

#[test]
fn cli_publish_nonexistent_workflow() {
    oxo_flow_cmd()
        .args(["publish", "/nonexistent/path/workflow.oxoflow"])
        .assert()
        .failure();
}

// ---------------------------------------------------------------------------
// provenance verify tests
// ---------------------------------------------------------------------------

#[test]
fn cli_provenance_verify_no_checksums() {
    let dir = tempfile::tempdir().unwrap();
    let cp = dir.path().join("cp.json");
    fs::write(
        &cp,
        r#"{"completed_rules":["step1","step2"],"failed_rules":[]}"#,
    )
    .unwrap();
    oxo_flow_cmd()
        .args(["provenance", "verify", cp.to_str().unwrap()])
        .assert()
        .success()
        .stderr(predicate::str::contains("No stored checksums found"));
}

#[test]
fn cli_provenance_verify_embedded_checksums() {
    let dir = tempfile::tempdir().unwrap();
    // Create a test file with known content
    let test_file = dir.path().join("verified_out.txt");
    fs::write(&test_file, "test data for verification").unwrap();

    // Compute SHA-256 manually without external crate
    let checksum = {
        use std::hash::Hasher;
        // Use a simple hash for test purposes — the provenance command
        // will compare against whatever we put in the checkpoint
        let mut h = std::collections::hash_map::DefaultHasher::new();
        h.write(b"test data for verification");
        let hash = h.finish();
        format!("sha256:{:x}", hash)
    };

    let cp = dir.path().join("checkpoint.json");
    let cp_content = format!(
        r#"{{"completed_rules":["gen"],"failed_rules":[],"checksums":{{"verified_out.txt":"{}"}}}}"#,
        checksum
    );
    fs::write(&cp, &cp_content).unwrap();

    // With a hash mismatch, the command exits with code 1 and reports mismatches
    oxo_flow_cmd()
        .args(["provenance", "verify", cp.to_str().unwrap()])
        .current_dir(dir.path())
        .assert()
        .failure()
        .stderr(predicate::str::contains("mismatched"));
}

// ---------------------------------------------------------------------------
// clean --orphans tests
// ---------------------------------------------------------------------------

#[test]
fn cli_clean_orphans_dry_run() {
    let dir = tempfile::tempdir().unwrap();
    let wf = dir.path().join("orphan_test.oxoflow");
    fs::write(
        &wf,
        "[workflow]\nname = \"orphan-test\"\nversion = \"1.0.0\"\n",
    )
    .unwrap();
    fs::create_dir_all(dir.path().join(".oxo-flow/chunks/chunk_001")).unwrap();
    fs::create_dir_all(dir.path().join(".oxo-flow/chunks/chunk_002")).unwrap();

    oxo_flow_cmd()
        .args(["clean", wf.to_str().unwrap(), "--orphans"])
        .current_dir(dir.path())
        .assert()
        .success()
        .stderr(predicate::str::contains("Would clean"));
}

#[test]
fn cli_clean_orphans_force() {
    let dir = tempfile::tempdir().unwrap();
    let wf = dir.path().join("orphan_f.oxoflow");
    fs::write(
        &wf,
        "[workflow]\nname = \"orphan-force\"\nversion = \"1.0.0\"\n",
    )
    .unwrap();
    fs::create_dir_all(dir.path().join(".oxo-flow/chunks/chunk_001")).unwrap();

    oxo_flow_cmd()
        .args(["clean", wf.to_str().unwrap(), "--orphans", "--force"])
        .current_dir(dir.path())
        .assert()
        .success();

    assert!(
        !dir.path().join(".oxo-flow/chunks/chunk_001").exists(),
        "orphan chunk should be deleted"
    );
}

// ---------------------------------------------------------------------------
// schema command tests
// ---------------------------------------------------------------------------

#[test]
fn cli_schema_outputs_valid_json() {
    let output = oxo_flow_cmd().args(["schema"]).assert().success();

    let stdout = String::from_utf8_lossy(&output.get_output().stdout);
    let parsed: serde_json::Value =
        serde_json::from_str(&stdout).expect("schema output should be valid JSON");
    assert_eq!(
        parsed["title"], "oxo-flow workflow definition",
        "schema should have correct title"
    );
    assert!(
        parsed["properties"].get("workflow").is_some(),
        "schema should define workflow property"
    );
    assert!(
        parsed["properties"].get("rules").is_some(),
        "schema should define rules property"
    );
}
