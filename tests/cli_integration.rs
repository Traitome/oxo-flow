//! CLI binary integration tests.
//!
//! These tests exercise the `oxo-flow` binary via `assert_cmd`, ensuring that
//! the compiled CLI works correctly end-to-end.

use assert_cmd::Command;
use predicates::prelude::*;
use std::fs;

fn oxo_flow_cmd() -> Command {
    Command::cargo_bin("oxo-flow").expect("oxo-flow binary should be built")
}

fn venus_cmd() -> Command {
    Command::cargo_bin("venus").expect("venus binary should be built")
}

fn oxo_flow_web_cmd() -> Command {
    Command::cargo_bin("oxo-flow-web").expect("oxo-flow-web binary should be built")
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
fn cli_validate_valid_workflow() {
    oxo_flow_cmd()
        .args(["validate", "examples/simple_variant_calling.oxoflow"])
        .assert()
        .success();
}

#[test]
fn cli_validate_paired_tumor_normal() {
    oxo_flow_cmd()
        .args(["validate", "examples/paired_tumor_normal.oxoflow"])
        .assert()
        .success();
}

#[test]
fn cli_validate_nonexistent_file() {
    oxo_flow_cmd()
        .args(["validate", "nonexistent.oxoflow"])
        .assert()
        .failure();
}

#[test]
fn cli_validate_invalid_toml() {
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
        .args(["dry-run", "examples/paired_tumor_normal.oxoflow"])
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
        .args(["graph", "examples/simple_variant_calling.oxoflow"])
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
fn cli_completions_bash() {
    oxo_flow_cmd()
        .args(["completions", "bash"])
        .assert()
        .success()
        .stdout(predicate::str::contains("oxo-flow"));
}

#[test]
fn cli_completions_zsh() {
    oxo_flow_cmd()
        .args(["completions", "zsh"])
        .assert()
        .success();
}

#[test]
fn cli_completions_fish() {
    oxo_flow_cmd()
        .args(["completions", "fish"])
        .assert()
        .success();
}

// ─── venus CLI ──────────────────────────────────────────────────────────────

#[test]
fn venus_help() {
    venus_cmd()
        .arg("--help")
        .assert()
        .success()
        .stdout(predicate::str::contains("venus"));
}

#[test]
fn venus_version() {
    venus_cmd()
        .arg("--version")
        .assert()
        .success()
        .stdout(predicate::str::contains("venus"));
}

#[test]
fn venus_list_steps() {
    venus_cmd()
        .arg("list-steps")
        .assert()
        .success()
        .stdout(predicate::str::contains("Fastp"))
        .stdout(predicate::str::contains("BwaMem2"))
        .stdout(predicate::str::contains("Mutect2"))
        .stdout(predicate::str::contains("Vep"))
        .stdout(predicate::str::contains("ClinicalReport"));
}

#[test]
fn venus_validate_nonexistent() {
    venus_cmd()
        .args(["validate", "nonexistent.toml"])
        .assert()
        .failure();
}

#[test]
fn venus_generate_nonexistent() {
    venus_cmd()
        .args(["generate", "nonexistent.toml"])
        .assert()
        .failure();
}

#[test]
fn venus_unknown_command() {
    venus_cmd().arg("foobar").assert().failure();
}

#[test]
fn venus_generate_and_validate() {
    let dir = tempfile::tempdir().unwrap();
    let config_path = dir.path().join("venus_config.toml");
    let output_path = dir.path().join("pipeline.oxoflow");

    let config = r#"
mode = "TumorOnly"
seq_type = "WGS"
genome_build = "GRCh38"
reference_fasta = "/ref/hg38.fa"
threads = 8
output_dir = "output"
annotate = true
report = true

[[tumor_samples]]
name = "TUMOR_01"
r1_fastq = "raw/TUMOR_01_R1.fq.gz"
r2_fastq = "raw/TUMOR_01_R2.fq.gz"
is_tumor = true
"#;

    fs::write(&config_path, config).unwrap();

    // Validate the config
    venus_cmd()
        .args(["validate", config_path.to_str().unwrap()])
        .assert()
        .success()
        .stdout(predicate::str::contains("valid"));

    // Generate the pipeline
    venus_cmd()
        .args([
            "generate",
            config_path.to_str().unwrap(),
            "-o",
            output_path.to_str().unwrap(),
        ])
        .assert()
        .success();

    // The generated file should exist and be valid oxoflow
    assert!(output_path.exists());
    let content = fs::read_to_string(&output_path).unwrap();
    assert!(content.contains("[workflow]"));

    // Validate the generated pipeline with oxo-flow
    oxo_flow_cmd()
        .args(["validate", output_path.to_str().unwrap()])
        .assert()
        .success();
}

// ─── oxo-flow-web binary ────────────────────────────────────────────────────

#[test]
fn web_binary_exists() {
    // Verify the web binary was built successfully
    let _cmd = oxo_flow_web_cmd();
}
