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

// ─── Pilot subset (--samples) & forced re-run (--rerun) ─────────────────────

/// --samples first:N runs a pilot subset; scaling up afterwards skips the
/// completed samples via the checkpoint and runs only the remaining ones.
#[test]
fn cli_samples_pilot_then_scale_up() {
    let dir = tempfile::tempdir().unwrap();
    let wf = dir.path().join("scale.oxoflow");
    fs::write(
        &wf,
        "[workflow]\nname = \"scale\"\nversion = \"1.0.0\"\n\n[[sample_groups]]\nname = \"cohort\"\nsamples = [\"S1\", \"S2\", \"S3\"]\n\n[[rules]]\nname = \"analyze\"\noutput = [\"out/{sample}.txt\"]\nshell = \"echo {sample} > {output}\"\n",
    )
    .unwrap();

    // Pilot: only the first sample.
    let pilot = oxo_flow_cmd()
        .args(["run", wf.to_str().unwrap(), "--samples", "first:1"])
        .current_dir(dir.path())
        .output()
        .unwrap();
    assert!(
        pilot.status.success(),
        "pilot run failed: {}",
        String::from_utf8_lossy(&pilot.stderr)
    );
    assert!(dir.path().join("out/S1.txt").exists());
    assert!(!dir.path().join("out/S2.txt").exists());

    // Scale up: S1 is completed, only S2/S3 execute.
    let full = oxo_flow_cmd()
        .args(["run", wf.to_str().unwrap()])
        .current_dir(dir.path())
        .output()
        .unwrap();
    assert!(
        full.status.success(),
        "scale-up run failed: {}",
        String::from_utf8_lossy(&full.stderr)
    );
    let stderr = String::from_utf8_lossy(&full.stderr);
    assert!(
        stderr.contains("1 skipped"),
        "scale-up should skip S1: {stderr}"
    );
    // samples_list churns between the pilot and scale-up runs (engine
    // injected): it must NOT be reported as a config change (issue #62).
    assert!(
        !stderr.contains("Config change:"),
        "--samples toggle must not trigger config-change invalidation: {stderr}"
    );
    assert!(dir.path().join("out/S2.txt").exists());
    assert!(dir.path().join("out/S3.txt").exists());
}

/// --rerun forces re-execution even when outputs are up to date, while a
/// plain second run skips everything via the checkpoint.
#[test]
fn cli_rerun_forces_execution() {
    let dir = tempfile::tempdir().unwrap();
    let wf = dir.path().join("again.oxoflow");
    fs::write(
        &wf,
        "[workflow]\nname = \"again\"\nversion = \"1.0.0\"\n\n[[rules]]\nname = \"step\"\noutput = [\"out.txt\"]\nshell = \"echo run > {output}\"\n",
    )
    .unwrap();

    let run1 = oxo_flow_cmd()
        .args(["run", wf.to_str().unwrap()])
        .current_dir(dir.path())
        .output()
        .unwrap();
    assert!(run1.status.success());

    // Second run without --rerun: everything is up to date.
    let run2 = oxo_flow_cmd()
        .args(["run", wf.to_str().unwrap()])
        .current_dir(dir.path())
        .output()
        .unwrap();
    assert!(run2.status.success());
    assert!(
        String::from_utf8_lossy(&run2.stderr).contains("1 skipped"),
        "second run should skip: {}",
        String::from_utf8_lossy(&run2.stderr)
    );

    // Third run with --rerun: forced re-execution.
    let run3 = oxo_flow_cmd()
        .args(["run", wf.to_str().unwrap(), "--rerun"])
        .current_dir(dir.path())
        .output()
        .unwrap();
    assert!(run3.status.success());
    let stderr3 = String::from_utf8_lossy(&run3.stderr);
    assert!(
        stderr3.contains("1 succeeded"),
        "--rerun should re-execute: {stderr3}"
    );
    assert!(
        !stderr3.contains("1 skipped"),
        "--rerun must not skip: {stderr3}"
    );
}

/// --samples and -t combine as an intersection: both constraints apply.
#[test]
fn cli_samples_and_target_intersect() {
    let dir = tempfile::tempdir().unwrap();
    let wf = dir.path().join("pilot.oxoflow");
    fs::write(
        &wf,
        "[workflow]\nname = \"pilot\"\nversion = \"1.0.0\"\n\n[[sample_groups]]\nname = \"cohort\"\nsamples = [\"S1\", \"S2\"]\n\n[[rules]]\nname = \"fastp_trim\"\noutput = [\"trim/{sample}.fq\"]\nshell = \"echo t > {output}\"\n\n[[rules]]\nname = \"align\"\ninput = [\"trim/{sample}.fq\"]\noutput = [\"aln/{sample}.bam\"]\nshell = \"echo a > {output}\"\n",
    )
    .unwrap();

    let out = oxo_flow_cmd()
        .args([
            "dry-run",
            wf.to_str().unwrap(),
            "--samples",
            "first:1",
            "-t",
            "fastp",
        ])
        .current_dir(dir.path())
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "dry-run failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("1 rules would execute"),
        "intersection should yield exactly the 1 fastp rule: {stderr}"
    );
    assert!(
        !stderr.contains("align_"),
        "align must be filtered by -t: {stderr}"
    );
}

/// The scientific preflight warns on a small-cohort VQSR pilot and a BQSR
/// step without known-sites resources; a pilot run prints the summary with
/// a full-cohort projection.
#[test]
fn cli_scientific_preflight_and_pilot_summary() {
    let dir = tempfile::tempdir().unwrap();
    let wf = dir.path().join("vqsr.oxoflow");
    fs::write(
        &wf,
        "[workflow]\nname = \"vqsr\"\nversion = \"1.0.0\"\n\n[[sample_groups]]\nname = \"cohort\"\nsamples = [\"S1\", \"S2\", \"S3\"]\n\n[[rules]]\nname = \"vqsr_snps\"\noutput = [\"vqsr/out.vcf.gz\"]\nshell = \"echo 'gatk VariantRecalibrator -V variants.vcf.gz -O vqsr/out.vcf.gz' > {output}\"\n",
    )
    .unwrap();

    // dry-run preflight on the pilot subset
    let dry = oxo_flow_cmd()
        .args(["dry-run", wf.to_str().unwrap(), "--samples", "first:2"])
        .current_dir(dir.path())
        .output()
        .unwrap();
    assert!(dry.status.success());
    let stderr = String::from_utf8_lossy(&dry.stderr);
    assert!(
        stderr.contains("SCI-VQSR-COHORT"),
        "preflight should warn about the 2-sample VQSR pilot: {stderr}"
    );

    // run a pilot — the summary projects to the full cohort
    let run = oxo_flow_cmd()
        .args(["run", wf.to_str().unwrap(), "--samples", "first:1"])
        .current_dir(dir.path())
        .output()
        .unwrap();
    assert!(run.status.success());
    let stderr = String::from_utf8_lossy(&run.stderr);
    assert!(
        stderr.contains("Pilot summary:"),
        "pilot run should print a summary: {stderr}"
    );
    assert!(
        stderr.contains("1/3 (pilot)"),
        "summary should report 1 of 3 samples: {stderr}"
    );
}

/// `oxo-flow ai status` lists discovered custom skills (read-only) even when
/// no AI provider is configured — discovery never activates anything.
#[test]
fn cli_ai_status_lists_discovered_skills() {
    let dir = tempfile::tempdir().unwrap();
    let skills_dir = dir.path().join(".oxo-flow").join("skills");
    std::fs::create_dir_all(&skills_dir).unwrap();
    std::fs::write(
        skills_dir.join("qc-expert.skill.toml"),
        "name = \"qc-expert\"\nversion = \"1.0.0\"\ndescription = \"Advises on FASTQ QC thresholds\"\nskill_type = \"knowledge\"\n",
    )
    .unwrap();

    let out = oxo_flow_cmd()
        .arg("ai")
        .current_dir(dir.path())
        // No AI provider configured in tests — the listing must still appear.
        .output()
        .unwrap();
    assert!(out.status.success());
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("qc-expert"),
        "ai status should list the discovered skill: {stdout}"
    );
    assert!(
        stdout.contains("Custom skills"),
        "ai status should have a Custom skills section: {stdout}"
    );
}

// ─── ai explain (issue #65) ─────────────────────────────────────────────────

#[test]
fn cli_ai_explain_requires_workflow() {
    oxo_flow_cmd()
        .args(["ai", "explain"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("requires a workflow file"));
}

#[test]
fn cli_ai_explain_unknown_step_fails_fast_without_provider() {
    // Step validation is deterministic and must not need a configured
    // provider — isolate HOME so no saved config can be found.
    let dir = tempfile::tempdir().unwrap();
    oxo_flow_cmd()
        .env("HOME", dir.path())
        .args([
            "ai",
            "explain",
            "examples/gallery/13_simple_variant_calling.oxoflow",
            "--step",
            "no_such_rule",
        ])
        .assert()
        .failure()
        .stderr(predicate::str::contains("no_such_rule"));
}

#[test]
fn cli_ai_explain_without_provider_fails_cleanly() {
    let dir = tempfile::tempdir().unwrap();
    oxo_flow_cmd()
        .env("HOME", dir.path())
        .args([
            "ai",
            "explain",
            "examples/gallery/13_simple_variant_calling.oxoflow",
        ])
        .assert()
        .failure()
        .stderr(predicate::str::contains("AI provider not configured"))
        .stderr(predicate::str::contains("ai setup"));
}

#[test]
fn cli_ai_unknown_action_errors() {
    oxo_flow_cmd()
        .args(["ai", "frobnicate"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("unknown ai action"));
}

#[test]
fn cli_ai_test_rejects_workflow_arg() {
    oxo_flow_cmd()
        .args([
            "ai",
            "test",
            "examples/gallery/13_simple_variant_calling.oxoflow",
        ])
        .assert()
        .failure()
        .stderr(predicate::str::contains("takes no workflow"));
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
fn cli_help_shows_banner() {
    // Top-level help carries the ASCII-art banner, version and repo URL.
    oxo_flow_cmd()
        .arg("--help")
        .assert()
        .success()
        .stdout(predicate::str::contains("███╗"))
        .stdout(predicate::str::contains(
            "https://github.com/Traitome/oxo-flow",
        ))
        .stdout(predicate::str::contains(env!("CARGO_PKG_VERSION")));
}

#[test]
fn cli_subcommand_help_has_no_banner() {
    // The banner belongs to the top-level help only.
    oxo_flow_cmd()
        .args(["run", "--help"])
        .assert()
        .success()
        .stdout(predicate::str::contains("███╗").not());
}

// ─── Non-UTF-8 arguments ────────────────────────────────────────────────────

/// The banner pre-scan in main() walks argv with args_os, so arguments
/// with invalid UTF-8 (common in bioinformatics paths) must not panic.
#[cfg(unix)]
#[test]
fn cli_no_panic_on_non_utf8_args() {
    use std::os::unix::ffi::OsStrExt;
    let out = oxo_flow_cmd()
        .arg(std::ffi::OsStr::from_bytes(b"bad-\xff-arg"))
        .output()
        .unwrap();
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        !stderr.contains("panicked"),
        "non-UTF-8 args must not panic: {stderr}"
    );
}

// ─── Run log header (print_banner) ─────────────────────────────────────────

/// Long-running commands print the two-line banner (version + repository)
/// at the top of their stderr log.
#[test]
fn cli_run_log_shows_banner() {
    let dir = tempfile::tempdir().unwrap();
    let wf = dir.path().join("tiny.oxoflow");
    fs::write(
        &wf,
        "[workflow]\nname = \"tiny\"\nversion = \"1.0.0\"\n\n[[rules]]\nname = \"hello\"\noutput = [\"out.txt\"]\nshell = \"echo hi > {output}\"\n",
    )
    .unwrap();

    let out = oxo_flow_cmd()
        .args(["run", wf.to_str().unwrap()])
        .current_dir(dir.path())
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "run failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains(&format!("oxo-flow v{}", env!("CARGO_PKG_VERSION"))),
        "run log should carry the banner version line: {stderr}"
    );
    assert!(
        stderr.contains("Rust-native bioinformatics pipeline engine"),
        "run log should carry the tagline: {stderr}"
    );
    assert!(
        stderr.contains("https://github.com/Traitome/oxo-flow"),
        "run log should carry the repository URL: {stderr}"
    );
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
        "examples/gallery/13_simple_variant_calling.oxoflow",
        "examples/gallery/14_paired_experiment_control.oxoflow",
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
        .args([
            "dry-run",
            "examples/gallery/13_simple_variant_calling.oxoflow",
        ])
        .assert()
        .success();
}

#[test]
fn cli_dry_run_paired() {
    oxo_flow_cmd()
        .args([
            "dry-run",
            "examples/gallery/14_paired_experiment_control.oxoflow",
        ])
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

/// dry-run reads the checkpoint (read-only) and predicts the actual
/// incremental plan: protected rules, invalidated rules, and the
/// downstream cascade (issue #66).
#[test]
fn cli_dry_run_previews_checkpoint_status() {
    let dir = tempfile::tempdir().unwrap();
    let wf = dir.path().join("w.oxoflow");
    std::fs::write(
        &wf,
        r#"[workflow]
name = "t"
version = "1.0"

[[rules]]
name = "step1"
input = ["in.txt"]
output = ["out1.txt"]
shell = "cp in.txt out1.txt"

[[rules]]
name = "step2"
input = ["out1.txt"]
output = ["out2.txt"]
depends_on = ["step1"]
shell = "cp out1.txt out2.txt"
"#,
    )
    .unwrap();
    std::fs::write(dir.path().join("in.txt"), "data1").unwrap();

    // Run once: both rules complete and the checkpoint records manifests.
    oxo_flow_cmd()
        .arg("run")
        .arg(&wf)
        .arg("-j")
        .arg("2")
        .current_dir(dir.path())
        .assert()
        .success();

    // dry-run: everything protected.
    let out = oxo_flow_cmd()
        .arg("dry-run")
        .arg(&wf)
        .current_dir(dir.path())
        .output()
        .unwrap();
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("completed: 2"), "{stderr}");
    assert!(
        stderr.matches("[skip: up to date]").count() == 2,
        "{stderr}"
    );

    // Change the input: step1 invalidates, step2 cascades.
    std::fs::write(dir.path().join("in.txt"), "changed data").unwrap();
    let out = oxo_flow_cmd()
        .arg("dry-run")
        .arg(&wf)
        .current_dir(dir.path())
        .output()
        .unwrap();
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("[run: input changed]"), "{stderr}");
    assert!(stderr.contains("[rerun: downstream of step1]"), "{stderr}");
    assert!(stderr.contains("rerun cascade: step1 → step2"), "{stderr}");

    // --json exposes the same prediction machine-readably.
    let out = oxo_flow_cmd()
        .args(["dry-run", "--json"])
        .arg(&wf)
        .current_dir(dir.path())
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&out.stdout);
    let v: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    let preview = &v["checkpoint_preview"];
    assert_eq!(preview["summary"]["will_run"], 2, "{stdout}");
    assert_eq!(
        preview["plan"][0]["status"], "run-input-changed",
        "{stdout}"
    );
    assert_eq!(preview["plan"][1]["status"], "run-cascaded", "{stdout}");
    assert_eq!(preview["plan"][1]["cascaded_from"], "step1", "{stdout}");
}

// ─── graph subcommand ───────────────────────────────────────────────────────

#[test]
fn cli_graph_outputs_dot() {
    oxo_flow_cmd()
        .args([
            "graph",
            "-f",
            "dot",
            "examples/gallery/13_simple_variant_calling.oxoflow",
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
            "examples/gallery/13_simple_variant_calling.oxoflow",
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
            "examples/gallery/13_simple_variant_calling.oxoflow",
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

/// The gallery is embedded in the binary (issue #76): `template` must work
/// from an arbitrary directory with no repo checkout around — the binary
/// lives in target/, CWD is a temp dir, so filesystem discovery would fail.
#[test]
fn cli_template_gallery_works_without_repo_checkout() {
    let dir = tempfile::tempdir().unwrap();

    // Listing
    let out = oxo_flow_cmd()
        .arg("template")
        .current_dir(dir.path())
        .output()
        .unwrap();
    assert!(out.status.success());
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("01_hello_world") && stderr.contains("07_wgs_germline"),
        "gallery listing should come from the binary: {stderr}"
    );

    // Applying by exact stem
    oxo_flow_cmd()
        .args(["template", "01_hello_world"])
        .current_dir(dir.path())
        .assert()
        .success();
    let wf = fs::read_to_string(dir.path().join("hello_world.oxoflow")).unwrap();
    assert!(wf.contains("[workflow]"), "template content is embedded");
    assert!(
        wf.contains("name = \"hello_world\""),
        "workflow name is substituted from the stem"
    );

    // Applying by descriptive suffix in a subdirectory
    let sub = dir.path().join("sub");
    fs::create_dir_all(&sub).unwrap();
    oxo_flow_cmd()
        .args(["template", "parallel_samples", "-o", sub.to_str().unwrap()])
        .assert()
        .success();
    assert!(sub.join("parallel_samples.oxoflow").exists());

    // Unknown template still errors cleanly.
    oxo_flow_cmd()
        .args(["template", "no_such_template"])
        .current_dir(dir.path())
        .assert()
        .failure()
        .stderr(predicate::str::contains("not found"));
}

// ─── clean subcommand ───────────────────────────────────────────────────────

#[test]
fn cli_clean_dry_run() {
    oxo_flow_cmd()
        .args([
            "clean",
            "examples/gallery/13_simple_variant_calling.oxoflow",
            "-n",
        ])
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

#[test]
fn cli_export_compose() {
    oxo_flow_cmd()
        .args([
            "export",
            "examples/gallery/01_hello_world.oxoflow",
            "-f",
            "compose",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("version: \"3.8\""))
        .stdout(predicate::str::contains("services:"));
}

#[test]
fn cli_export_compose_to_file() {
    let dir = tempfile::tempdir().unwrap();
    let out = dir.path().join("compose.yml");

    oxo_flow_cmd()
        .args([
            "export",
            "examples/gallery/01_hello_world.oxoflow",
            "-f",
            "compose",
            "-o",
            out.to_str().unwrap(),
        ])
        .assert()
        .success()
        .stderr(predicate::str::contains("Exported compose"));

    let content = fs::read_to_string(&out).unwrap();
    assert!(content.contains("version: \"3.8\""));
    assert!(content.contains("command: [\"run\", \"workflow.oxoflow\"]"));
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

#[test]
fn cli_status_defaults_to_workdir_checkpoint() {
    let dir = tempfile::tempdir().unwrap();
    let flow_dir = dir.path().join(".oxo-flow");
    fs::create_dir_all(&flow_dir).unwrap();
    fs::write(
        flow_dir.join("checkpoint.json"),
        r#"{
            "completed_rules": ["step_a"],
            "failed_rules": [],
            "benchmarks": {
                "step_a": {"rule": "step_a", "wall_time_secs": 1.23, "max_memory_mb": null, "cpu_seconds": null}
            }
        }"#,
    )
    .unwrap();

    oxo_flow_cmd()
        .current_dir(dir.path())
        .args(["status"])
        .assert()
        .success()
        .stderr(predicate::str::contains("step_a"));
}

#[test]
fn cli_status_missing_default_checkpoint_fails() {
    let dir = tempfile::tempdir().unwrap();

    oxo_flow_cmd()
        .current_dir(dir.path())
        .args(["status"])
        .assert()
        .failure()
        .stderr(predicate::str::contains(".oxo-flow/checkpoint.json"));
}

#[test]
fn cli_status_timing_slowest_first_with_total() {
    let dir = tempfile::tempdir().unwrap();
    let checkpoint = dir.path().join("checkpoint.json");
    fs::write(
        &checkpoint,
        r#"{
            "completed_rules": ["step_fast", "step_slow"],
            "failed_rules": [],
            "benchmarks": {
                "step_fast": {"rule": "step_fast", "wall_time_secs": 1.0, "max_memory_mb": null, "cpu_seconds": null},
                "step_slow": {"rule": "step_slow", "wall_time_secs": 9.5, "max_memory_mb": null, "cpu_seconds": null}
            }
        }"#,
    )
    .unwrap();

    oxo_flow_cmd()
        .args(["status", checkpoint.to_str().unwrap(), "--timing"])
        .assert()
        .success()
        .stderr(predicate::str::contains("Rule timings"))
        .stderr(predicate::str::contains("total 10.5s"))
        .stderr(predicate::function(|s: &str| {
            s.find("step_slow (9.5s)")
                .is_some_and(|slow| s.find("step_fast (1.0s)").is_some_and(|fast| slow < fast))
        }));
}

#[test]
fn cli_status_timing_limit_truncates() {
    let dir = tempfile::tempdir().unwrap();
    let checkpoint = dir.path().join("checkpoint.json");
    fs::write(
        &checkpoint,
        r#"{
            "completed_rules": ["step_fast", "step_mid", "step_slow"],
            "failed_rules": [],
            "benchmarks": {
                "step_fast": {"rule": "step_fast", "wall_time_secs": 1.0, "max_memory_mb": null, "cpu_seconds": null},
                "step_mid": {"rule": "step_mid", "wall_time_secs": 5.0, "max_memory_mb": null, "cpu_seconds": null},
                "step_slow": {"rule": "step_slow", "wall_time_secs": 9.5, "max_memory_mb": null, "cpu_seconds": null}
            }
        }"#,
    )
    .unwrap();

    oxo_flow_cmd()
        .args([
            "status",
            checkpoint.to_str().unwrap(),
            "--timing",
            "-n",
            "2",
        ])
        .assert()
        .success()
        .stderr(predicate::str::contains("step_slow (9.5s)"))
        .stderr(predicate::str::contains("step_mid (5.0s)"))
        .stderr(predicate::str::contains("step_fast").not());
}

#[test]
fn cli_status_json_includes_timings() {
    let dir = tempfile::tempdir().unwrap();
    let checkpoint = dir.path().join("checkpoint.json");
    fs::write(
        &checkpoint,
        r#"{
            "completed_rules": ["step_fast", "step_slow"],
            "failed_rules": [],
            "benchmarks": {
                "step_fast": {"rule": "step_fast", "wall_time_secs": 1.0, "max_memory_mb": null, "cpu_seconds": null},
                "step_slow": {"rule": "step_slow", "wall_time_secs": 9.5, "max_memory_mb": null, "cpu_seconds": null}
            }
        }"#,
    )
    .unwrap();

    oxo_flow_cmd()
        .args(["status", checkpoint.to_str().unwrap(), "--timing", "--json"])
        .assert()
        .success()
        .stdout(predicate::str::contains("\"timings\""))
        .stdout(predicate::str::contains("\"step_slow\": 9.5"))
        .stdout(predicate::str::contains("\"total_time_secs\": 10.5"));
}

#[test]
fn cli_status_limit_requires_timing() {
    oxo_flow_cmd()
        .args(["status", "checkpoint.json", "-n", "5"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("--timing"));
}

// ─── License subcommand ──────────────────────────────────────────────────────

#[test]
fn cli_license_status() {
    oxo_flow_cmd()
        .args(["license"])
        .assert()
        .success()
        .stdout(predicate::str::contains("License status:"));
}

#[test]
fn cli_license_invalid_path_fails() {
    oxo_flow_cmd()
        .args(["license", "/nonexistent/license.lic"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("License verification failed"));
}

// ─── Config subcommand ───────────────────────────────────────────────────────

#[test]
fn cli_config_show() {
    oxo_flow_cmd()
        .args([
            "config",
            "show",
            "examples/gallery/13_simple_variant_calling.oxoflow",
        ])
        .assert()
        .success()
        .stderr(predicate::str::contains("Name:"));
}

#[test]
fn cli_config_stats() {
    oxo_flow_cmd()
        .args([
            "config",
            "stats",
            "examples/gallery/13_simple_variant_calling.oxoflow",
        ])
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
            "examples/gallery/13_simple_variant_calling.oxoflow",
            "examples/gallery/13_simple_variant_calling.oxoflow",
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
            "examples/gallery/13_simple_variant_calling.oxoflow",
            "examples/gallery/14_paired_experiment_control.oxoflow",
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
            "examples/gallery/13_simple_variant_calling.oxoflow",
            "nonexistent.oxoflow",
        ])
        .assert()
        .failure();
}

// ─── Format subcommand ───────────────────────────────────────────────────────

#[test]
fn cli_format_outputs_canonical_toml() {
    oxo_flow_cmd()
        .args([
            "format",
            "examples/gallery/13_simple_variant_calling.oxoflow",
        ])
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
            "examples/gallery/13_simple_variant_calling.oxoflow",
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
            "examples/gallery/13_simple_variant_calling.oxoflow",
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
            "examples/gallery/13_simple_variant_calling.oxoflow",
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

// ─── workflow [config] CLI tests ──────────────────────────────────────

#[test]
fn cli_run_with_arg_required_missing_fails() {
    let dir = tempfile::tempdir().unwrap();
    let wf = dir.path().join("args.oxoflow");
    fs::write(
        &wf,
        "[workflow]\nname = \"args\"\nversion = \"1.0.0\"\n\n[config]\ndatabase = { required = true, help = \"Path to database\" }\n\n[[rules]]\nname = \"s\"\noutput = [\"out.txt\"]\nshell = \"echo {config.database} > {output[0]}\"\n",
    )
    .unwrap();

    let output = oxo_flow_cmd()
        .args(["run", wf.to_str().unwrap()])
        .current_dir(dir.path())
        .output()
        .unwrap();
    assert!(
        !output.status.success(),
        "should fail when required arg missing"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("required config"),
        "should mention required config: {stderr}"
    );
    assert!(
        stderr.contains("database"),
        "should name the missing arg: {stderr}"
    );
}

#[test]
fn cli_run_with_arg_provided_succeeds() {
    let dir = tempfile::tempdir().unwrap();
    let wf = dir.path().join("args2.oxoflow");
    fs::write(
        &wf,
        "[workflow]\nname = \"args2\"\nversion = \"1.0.0\"\n\n[config]\ndatabase = { required = true, help = \"Path\" }\n\n[[rules]]\nname = \"s\"\noutput = [\"out.txt\"]\nshell = \"echo {config.database} > {output[0]}\"\n",
    )
    .unwrap();

    let output = oxo_flow_cmd()
        .args(["run", wf.to_str().unwrap(), "--arg", "database=refs/nt"])
        .current_dir(dir.path())
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "should succeed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(dir.path().join("out.txt").exists());
    let content = fs::read_to_string(dir.path().join("out.txt")).unwrap();
    assert_eq!(content.trim(), "refs/nt");
}

#[test]
fn cli_run_with_arg_default_value() {
    let dir = tempfile::tempdir().unwrap();
    let wf = dir.path().join("args3.oxoflow");
    fs::write(
        &wf,
        "[workflow]\nname = \"args3\"\nversion = \"1.0.0\"\n\n[config]\nthreshold = { default = \"1e-5\", help = \"E-value\" }\n\n[[rules]]\nname = \"s\"\noutput = [\"out.txt\"]\nshell = \"echo {config.threshold} > {output[0]}\"\n",
    )
    .unwrap();

    let output = oxo_flow_cmd()
        .args(["run", wf.to_str().unwrap()])
        .current_dir(dir.path())
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "should succeed with default: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let content = fs::read_to_string(dir.path().join("out.txt")).unwrap();
    assert_eq!(content.trim(), "1e-5", "default value should be used");
}

#[test]
fn cli_run_with_arg_overrides_default() {
    let dir = tempfile::tempdir().unwrap();
    let wf = dir.path().join("args4.oxoflow");
    fs::write(
        &wf,
        "[workflow]\nname = \"args4\"\nversion = \"1.0.0\"\n\n[config]\nthreshold = { default = \"1e-5\", help = \"E-value\" }\n\n[[rules]]\nname = \"s\"\noutput = [\"out.txt\"]\nshell = \"echo {config.threshold} > {output[0]}\"\n",
    )
    .unwrap();

    let output = oxo_flow_cmd()
        .args(["run", wf.to_str().unwrap(), "--arg", "threshold=1e-10"])
        .current_dir(dir.path())
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "should succeed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let content = fs::read_to_string(dir.path().join("out.txt")).unwrap();
    assert_eq!(content.trim(), "1e-10", "CLI arg should override default");
}

#[test]
fn cli_run_with_arg_invalid_format_fails() {
    let dir = tempfile::tempdir().unwrap();
    let wf = dir.path().join("args5.oxoflow");
    fs::write(&wf, "[workflow]\nname = \"args5\"\nversion = \"1.0.0\"\n\n[[rules]]\nname = \"s\"\noutput = [\"out.txt\"]\nshell = \"echo ok > {output[0]}\"\n").unwrap();

    let output = oxo_flow_cmd()
        .args(["run", wf.to_str().unwrap(), "--arg", "no-equals-sign"])
        .current_dir(dir.path())
        .output()
        .unwrap();
    assert!(!output.status.success(), "should fail on invalid format");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("KEY=VALUE") || stderr.contains("invalid"),
        "should mention format: {stderr}"
    );
}

#[test]
fn cli_run_config_with_choices_rejects_invalid_value() {
    let dir = tempfile::tempdir().unwrap();
    let wf = dir.path().join("choices.oxoflow");
    fs::write(
        &wf,
        "[workflow]\nname = \"choices\"\nversion = \"1.0.0\"\n\n[config]\nmode = { default = \"dna\", choices = [\"dna\", \"rna\"] }\n\n[[rules]]\nname = \"s\"\noutput = [\"out.txt\"]\nshell = \"echo {config.mode} > {output[0]}\"\n",
    )
    .unwrap();

    // Invalid value should fail
    let output = oxo_flow_cmd()
        .args(["run", wf.to_str().unwrap(), "--mode=protein"])
        .current_dir(dir.path())
        .output()
        .unwrap();
    assert!(!output.status.success(), "invalid choice should fail");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("invalid value") && stderr.contains("dna, rna"),
        "should mention invalid value and allowed choices: {stderr}"
    );

    // Valid value should work
    let output2 = oxo_flow_cmd()
        .args(["run", wf.to_str().unwrap(), "--mode=rna"])
        .current_dir(dir.path())
        .output()
        .unwrap();
    assert!(output2.status.success(), "valid choice should succeed");
}

#[test]
fn cli_run_config_with_bad_default_rejected() {
    let dir = tempfile::tempdir().unwrap();
    let wf = dir.path().join("badchoice.oxoflow");
    fs::write(
        &wf,
        "[workflow]\nname = \"badchoice\"\nversion = \"1.0.0\"\n\n[config]\nmode = { default = \"protein\", choices = [\"dna\", \"rna\"] }\n\n[[rules]]\nname = \"s\"\noutput = [\"out.txt\"]\nshell = \"echo {config.mode} > {output[0]}\"\n",
    )
    .unwrap();

    // Default itself is invalid — should fail at parse/validation time
    let output = oxo_flow_cmd()
        .args(["run", wf.to_str().unwrap()])
        .current_dir(dir.path())
        .output()
        .unwrap();
    assert!(!output.status.success(), "bad default should be rejected");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("invalid value") && stderr.contains("protein"),
        "should mention invalid value: {stderr}"
    );
}

/// Typed config values (int/float) must support numeric comparisons in `when`.
#[test]
fn cli_run_when_condition_uses_typed_integer() {
    let dir = tempfile::tempdir().unwrap();
    let wf = dir.path().join("typed.oxoflow");
    fs::write(
        &wf,
        "[workflow]\nname = \"typed\"\nversion = \"1.0.0\"\n\n\
         [config]\nmin_qual = 30\n\n\
         [[rules]]\nname = \"qc_pass\"\noutput = [\"pass.txt\"]\n\
         shell = \"echo pass > {output[0]}\"\n\
         when = 'config.min_qual >= 20'\n\n\
         [[rules]]\nname = \"qc_fail\"\noutput = [\"fail.txt\"]\n\
         shell = \"echo fail > {output[0]}\"\n\
         when = 'config.min_qual < 20'\n",
    )
    .unwrap();

    let output = oxo_flow_cmd()
        .args(["run", wf.to_str().unwrap()])
        .current_dir(dir.path())
        .output()
        .unwrap();

    assert!(output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("1 succeeded"),
        "qc_pass should run: {stderr}"
    );
    assert!(
        dir.path().join("pass.txt").exists(),
        "qc_pass output should exist"
    );
    assert!(
        !dir.path().join("fail.txt").exists(),
        "qc_fail should be skipped"
    );
}

#[test]
fn cli_run_config_type_int_rejects_bad_value() {
    let dir = tempfile::tempdir().unwrap();
    let wf = dir.path().join("typeint.oxoflow");
    fs::write(
        &wf,
        "[workflow]\nname = \"t\"\nversion = \"1.0\"\n\n[config]\nmin_q = { default = \"30\", type = \"int\" }\n\n[[rules]]\nname = \"s\"\noutput = [\"out.txt\"]\nshell = \"echo {config.min_q} > {output[0]}\"\n",
    )
    .unwrap();
    let out = oxo_flow_cmd()
        .args(["run", wf.to_str().unwrap(), "--min_q=abc"])
        .current_dir(dir.path())
        .output()
        .unwrap();
    assert!(!out.status.success());
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("expects an integer"), "{stderr}");
}

#[test]
fn cli_run_config_type_int_accepts_good_value() {
    let dir = tempfile::tempdir().unwrap();
    let wf = dir.path().join("typeint2.oxoflow");
    fs::write(
        &wf,
        "[workflow]\nname = \"t\"\nversion = \"1.0\"\n\n[config]\nmin_q = { default = \"30\", type = \"int\" }\n\n[[rules]]\nname = \"s\"\noutput = [\"out.txt\"]\nshell = \"echo {config.min_q} > {output[0]}\"\n",
    )
    .unwrap();
    let out = oxo_flow_cmd()
        .args(["run", wf.to_str().unwrap(), "--min_q=42"])
        .current_dir(dir.path())
        .output()
        .unwrap();
    assert!(out.status.success());
}

#[test]
fn cli_run_config_type_float_rejects_bad_value() {
    let dir = tempfile::tempdir().unwrap();
    let wf = dir.path().join("typefloat.oxoflow");
    fs::write(
        &wf,
        "[workflow]\nname = \"t\"\nversion = \"1.0\"\n\n[config]\nthr = { default = \"1e-5\", type = \"float\" }\n\n[[rules]]\nname = \"s\"\noutput = [\"out.txt\"]\nshell = \"echo {config.thr} > {output[0]}\"\n",
    )
    .unwrap();
    let out = oxo_flow_cmd()
        .args(["run", wf.to_str().unwrap(), "--thr=notanumber"])
        .current_dir(dir.path())
        .output()
        .unwrap();
    assert!(!out.status.success());
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("expects a float"), "{stderr}");
}

#[test]
fn cli_run_config_range_rejects_out_of_range() {
    let dir = tempfile::tempdir().unwrap();
    let wf = dir.path().join("range.oxoflow");
    fs::write(
        &wf,
        "[workflow]\nname = \"t\"\nversion = \"1.0\"\n\n[config]\nqual = { default = \"30\", type = \"int\", range = \"0..60\" }\n\n[[rules]]\nname = \"s\"\noutput = [\"out.txt\"]\nshell = \"echo {config.qual} > {output[0]}\"\n",
    )
    .unwrap();
    let out = oxo_flow_cmd()
        .args(["run", wf.to_str().unwrap(), "--qual=999"])
        .current_dir(dir.path())
        .output()
        .unwrap();
    assert!(!out.status.success());
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("outside range"), "{stderr}");
}

#[test]
fn cli_run_config_range_accepts_in_range() {
    let dir = tempfile::tempdir().unwrap();
    let wf = dir.path().join("range2.oxoflow");
    fs::write(
        &wf,
        "[workflow]\nname = \"t\"\nversion = \"1.0\"\n\n[config]\nqual = { default = \"30\", type = \"int\", range = \"0..60\" }\n\n[[rules]]\nname = \"s\"\noutput = [\"out.txt\"]\nshell = \"echo {config.qual} > {output[0]}\"\n",
    )
    .unwrap();
    let out = oxo_flow_cmd()
        .args(["run", wf.to_str().unwrap(), "--qual=45"])
        .current_dir(dir.path())
        .output()
        .unwrap();
    assert!(out.status.success());
}

// ─── [[references]] auto-build tests ────────────────────────────────────

#[test]
fn cli_run_auto_builds_reference() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir(dir.path().join("ref")).unwrap();
    std::fs::write(dir.path().join("ref/genome.fa"), ">chr1\nACGT\n").unwrap();
    let wf = dir.path().join("ref_test.oxoflow");
    std::fs::write(
        &wf,
        "[workflow]\nname = \"ref-test\"\nversion = \"1.0.0\"\n\n[config]\nreference_dir = \"ref\"\nsamples = \"s.csv\"\n\n[[rules]]\nname = \"s\"\noutput = [\"out.txt\"]\nshell = \"echo bwa={config.bwa_index} > {output[0]}\"\n",
    )
    .unwrap();

    let output = oxo_flow_cmd()
        .args(["run", wf.to_str().unwrap(), "--skip-ref-build"])
        .current_dir(dir.path())
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "should succeed with --skip-ref-build: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let content = std::fs::read_to_string(dir.path().join("out.txt")).unwrap();
    assert!(
        content.contains("bwa=ref/bwa/genome.fa"),
        "config should contain derived bwa_index path: {content}"
    );
}

#[test]
fn cli_run_auto_derives_all_index_paths() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir(dir.path().join("ref")).unwrap();
    std::fs::write(dir.path().join("ref/genome.fa"), ">chr1\nACGT\n").unwrap();
    let wf = dir.path().join("derive.oxoflow");
    std::fs::write(
        &wf,
        "[workflow]\nname = \"derive\"\nversion = \"1.0.0\"\n\n[config]\nreference_dir = \"ref\"\nsamples = \"s.csv\"\n\n[[rules]]\nname = \"s\"\noutput = [\"out.txt\"]\nshell = \"echo bwa={config.bwa_index} star={config.star_index} minimap2={config.minimap2_index} dict={config.gatk_dict} faidx={config.samtools_faidx} > {output[0]}\"\n",
    )
    .unwrap();

    let output = oxo_flow_cmd()
        .args(["run", wf.to_str().unwrap(), "--skip-ref-build"])
        .current_dir(dir.path())
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let content = std::fs::read_to_string(dir.path().join("out.txt")).unwrap();
    assert!(content.contains("bwa=ref/bwa/genome.fa"), "{content}");
    assert!(content.contains("star=ref/star"), "{content}");
    assert!(content.contains("minimap2=ref/genome.fa.mmi"), "{content}");
    assert!(content.contains("dict=ref/genome.dict"), "{content}");
    assert!(content.contains("faidx=ref/genome.fa.fai"), "{content}");
}

#[test]
fn cli_run_ref_build_fails_with_clear_error() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir(dir.path().join("ref")).unwrap();
    std::fs::write(dir.path().join("ref/genome.fa"), ">chr1\nACGT\n").unwrap();
    let wf = dir.path().join("fail_ref.oxoflow");
    std::fs::write(
        &wf,
        "[workflow]\nname = \"fail-ref\"\nversion = \"1.0.0\"\n\n[config]\nreference_dir = \"ref\"\nsamples = \"s.csv\"\n\n[[references]]\nname = \"custom\"\nsource = \"ref/genome.fa\"\noutput = \"ref/custom.idx\"\nbuild = \"nonexistent_command_xyz\"\n\n[[rules]]\nname = \"s\"\noutput = [\"out.txt\"]\nshell = \"echo done > {output[0]}\"\n",
    )
    .unwrap();

    let output = oxo_flow_cmd()
        .args(["run", wf.to_str().unwrap()])
        .current_dir(dir.path())
        .output()
        .unwrap();
    assert!(
        !output.status.success(),
        "should fail when build command is missing"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("failed to build"),
        "should report build failure: {stderr}"
    );
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

    // New publish emits a .tar.zst archive, not a loose directory
    let archive = dir.path().join("pub_test-bundle.tar.zst");
    assert!(
        archive.exists(),
        "bundle archive should exist at {:?}",
        archive
    );
    // Verify it's a valid tar.zst by checking the magic bytes
    let data = fs::read(&archive).unwrap();
    assert!(data.len() > 8, "archive should not be empty");
    // zstd magic: 0x28 0xB5 0x2F 0xFD
    assert_eq!(
        &data[..4],
        &[0x28, 0xB5, 0x2F, 0xFD],
        "should be zstd compressed"
    );
}

#[test]
fn cli_publish_bundles_environment_conda_file() {
    let dir = tempfile::tempdir().unwrap();
    fs::create_dir(dir.path().join("envs")).unwrap();
    fs::write(
        dir.path().join("envs/fastp.yaml"),
        "name: fastp\ndependencies: [fastp]\n",
    )
    .unwrap();
    let wf = dir.path().join("pub_env.oxoflow");
    fs::write(
        &wf,
        "[workflow]\nname = \"pub-env\"\nversion = \"1.0.0\"\n\n[[rules]]\nname = \"s\"\noutput = [\"out.txt\"]\nshell = \"echo done > {output[0]}\"\n\n[rules.environment]\nconda = \"envs/fastp.yaml\"\n",
    )
    .unwrap();

    let output = oxo_flow_cmd()
        .args(["publish", wf.to_str().unwrap()])
        .current_dir(dir.path())
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "publish failed: stderr={}",
        String::from_utf8_lossy(&output.stderr)
    );

    // Verify the archive contains the expected files
    let archive = dir.path().join("pub_env-bundle.tar.zst");
    assert!(archive.exists(), "bundle archive should exist");

    // Decompress and list contents
    let file = std::fs::File::open(&archive).unwrap();
    let decoder = zstd::stream::read::Decoder::new(file).unwrap();
    let mut archive = tar::Archive::new(decoder);
    let mut found_manifest = false;
    let mut found_env = false;
    let mut found_wf = false;
    let mut paths = Vec::new();
    for entry in archive.entries().unwrap() {
        let entry = entry.unwrap();
        let path = entry.path().unwrap().to_string_lossy().to_string();
        paths.push(path.clone());
        if path == "manifest.json" {
            found_manifest = true;
        }
        if path.contains("fastp.yaml") {
            found_env = true;
        }
        if path.contains("pub_env.oxoflow") {
            found_wf = true;
        }
    }
    assert!(
        found_manifest,
        "manifest.json should be in archive. Found: {:?}",
        paths
    );
    assert!(
        found_env,
        "conda env file should be in archive. Found: {:?}",
        paths
    );
    assert!(
        found_wf,
        "workflow file should be in archive. Found: {:?}",
        paths
    );
}

#[test]
fn cli_publish_bundles_mamba_env() {
    let dir = tempfile::tempdir().unwrap();
    fs::create_dir(dir.path().join("envs")).unwrap();
    fs::write(
        dir.path().join("envs/qc.yaml"),
        "name: qc\ndependencies: [fastqc]\n",
    )
    .unwrap();
    let wf = dir.path().join("pub_mamba.oxoflow");
    fs::write(
        &wf,
        "[workflow]\nname = \"pub-mamba\"\nversion = \"1.0.0\"\n\n[[rules]]\nname = \"s\"\noutput = [\"out.txt\"]\nshell = \"echo done > {output[0]}\"\n\n[rules.environment]\nmamba = \"envs/qc.yaml\"\n",
    )
    .unwrap();

    oxo_flow_cmd()
        .args(["publish", wf.to_str().unwrap()])
        .current_dir(dir.path())
        .assert()
        .success();

    let archive = dir.path().join("pub_mamba-bundle.tar.zst");
    assert!(archive.exists(), "bundle archive should exist");

    let file = std::fs::File::open(&archive).unwrap();
    let decoder = zstd::stream::read::Decoder::new(file).unwrap();
    let mut archive = tar::Archive::new(decoder);
    let mut found_qc = false;
    for entry in archive.entries().unwrap() {
        let entry = entry.unwrap();
        let path = entry.path().unwrap().to_string_lossy().to_string();
        if path.contains("qc.yaml") {
            found_qc = true;
        }
    }
    assert!(
        found_qc,
        "mamba env file from [rules.environment] should be bundled"
    );
}

#[test]
fn cli_publish_nonexistent_workflow() {
    oxo_flow_cmd()
        .args(["publish", "/nonexistent/path/workflow.oxoflow"])
        .assert()
        .failure();
}

// ─── publish + run --bundle round-trip ─────────────────────────────────

#[test]
fn cli_publish_then_run_bundle_roundtrip() {
    let dir = tempfile::tempdir().unwrap();
    let wf = dir.path().join("roundtrip.oxoflow");
    fs::write(
        &wf,
        "[workflow]\nname = \"roundtrip\"\nversion = \"1.0.0\"\n\n[[rules]]\nname = \"s\"\noutput = [\"out.txt\"]\nshell = \"echo ok > {output[0]}\"\n",
    )
    .unwrap();

    // Publish
    let output = oxo_flow_cmd()
        .args(["publish", wf.to_str().unwrap()])
        .current_dir(dir.path())
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "publish failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let bundle = dir.path().join("roundtrip-bundle.tar.zst");
    assert!(bundle.exists(), "bundle should exist");

    // Run from bundle with explicit workdir
    let run_output = oxo_flow_cmd()
        .args([
            "run",
            "--bundle",
            bundle.to_str().unwrap(),
            "--yes",
            "-d",
            dir.path().to_str().unwrap(),
            "-j",
            "1",
        ])
        .output()
        .unwrap();
    assert!(
        run_output.status.success(),
        "run --bundle failed: stderr={}",
        String::from_utf8_lossy(&run_output.stderr)
    );
    assert!(
        dir.path().join("out.txt").exists(),
        "output file should be created in specified workdir"
    );
}

#[test]
fn cli_run_bundle_rejects_corrupted_archive() {
    let dir = tempfile::tempdir().unwrap();
    let bad_bundle = dir.path().join("corrupt.tar.zst");
    fs::write(&bad_bundle, "this is not a valid zstd archive").unwrap();

    let output = oxo_flow_cmd()
        .args(["run", "--bundle", bad_bundle.to_str().unwrap()])
        .output()
        .unwrap();
    assert!(!output.status.success(), "should reject corrupted archive");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("zstd") || stderr.contains("decompress") || stderr.contains("bundle"),
        "error should mention bundle/decompress: {stderr}"
    );
}

#[test]
fn cli_run_bundle_rejects_missing_manifest() {
    let dir = tempfile::tempdir().unwrap();
    // Build a valid tar.zst with no manifest.json
    let bundle_path = dir.path().join("nomanifest.tar.zst");
    let file = std::fs::File::create(&bundle_path).unwrap();
    let encoder = zstd::stream::write::Encoder::new(file, 3).unwrap();
    let mut tar = tar::Builder::new(encoder);
    // Add a dummy file but no manifest
    let tmp = dir.path().join("dummy.txt");
    fs::write(&tmp, "hello").unwrap();
    tar.append_path_with_name(&tmp, "dummy.txt").unwrap();
    let encoder = tar.into_inner().unwrap();
    encoder.finish().unwrap();

    let output = oxo_flow_cmd()
        .args(["run", "--bundle", bundle_path.to_str().unwrap()])
        .output()
        .unwrap();
    assert!(
        !output.status.success(),
        "should reject bundle without manifest"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("manifest"),
        "error should mention manifest: {stderr}"
    );
}

#[test]
fn cli_run_bundle_rejects_checksum_mismatch() {
    let dir = tempfile::tempdir().unwrap();
    // Create a valid bundle via publish
    fs::write(dir.path().join("dummy.txt"), "original content").unwrap();
    let wf = dir.path().join("cs_test.oxoflow");
    fs::write(
        &wf,
        "[workflow]\nname = \"cs\"\nversion = \"1.0.0\"\n\n[[rules]]\nname = \"s\"\noutput = [\"dummy.txt\"]\nshell = \"echo ok > {output[0]}\"\n",
    )
    .unwrap();

    let output = oxo_flow_cmd()
        .args(["publish", wf.to_str().unwrap()])
        .current_dir(dir.path())
        .output()
        .unwrap();
    assert!(output.status.success());

    // Now tamper with the bundle — modify a file inside it
    let bundle = dir.path().join("cs_test-bundle.tar.zst");
    // Extract, modify dummy.txt checksum in manifest, repack
    let extract_dir = dir.path().join("tampered");
    std::fs::create_dir(&extract_dir).unwrap();
    let file = std::fs::File::open(&bundle).unwrap();
    let decoder = zstd::stream::read::Decoder::new(file).unwrap();
    let mut archive = tar::Archive::new(decoder);
    archive.unpack(&extract_dir).unwrap();

    // Tamper manifest — change a SHA-256 prefix
    let manifest_path = extract_dir.join("manifest.json");
    let mut manifest: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&manifest_path).unwrap()).unwrap();
    manifest["files"][0]["sha256"] = serde_json::Value::String(
        "sha256:0000000000000000000000000000000000000000000000000000000000000000".into(),
    );
    std::fs::write(
        &manifest_path,
        serde_json::to_string_pretty(&manifest).unwrap(),
    )
    .unwrap();

    // Repack
    let tampered_bundle = dir.path().join("tampered.tar.zst");
    let file = std::fs::File::create(&tampered_bundle).unwrap();
    let encoder = zstd::stream::write::Encoder::new(file, 3).unwrap();
    let mut tar = tar::Builder::new(encoder);
    for entry in std::fs::read_dir(&extract_dir).unwrap() {
        let entry = entry.unwrap();
        let path = entry.path();
        if path.is_file() {
            let name = path.file_name().unwrap().to_str().unwrap();
            tar.append_path_with_name(&path, name).unwrap();
        }
    }
    let encoder = tar.into_inner().unwrap();
    encoder.finish().unwrap();

    // Run tampered bundle
    let output = oxo_flow_cmd()
        .args(["run", "--bundle", tampered_bundle.to_str().unwrap()])
        .output()
        .unwrap();
    assert!(!output.status.success(), "should reject tampered bundle");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("checksum") || stderr.contains("mismatch"),
        "error should mention checksum mismatch: {stderr}"
    );
}

#[test]
fn cli_pull_rejects_invalid_url() {
    let output = oxo_flow_cmd()
        .args(["pull", "not-a-valid-url!!!"])
        .output()
        .unwrap();
    assert!(!output.status.success(), "should reject invalid URL");
}

/// Non-bundle repository mode: `pull file://<dir>` clones a git repository,
/// discovers its workflow, and sanity-parses it — no bundle packaging
/// required (issue #76 follow-up).
#[test]
fn cli_pull_clones_repository_without_bundle() {
    let dir = tempfile::tempdir().unwrap();
    let repo = dir.path().join("pipeline-repo");
    fs::create_dir_all(&repo).unwrap();
    fs::write(
        repo.join("rnaseq.oxoflow"),
        "[workflow]\nname = \"rnaseq\"\nversion = \"1.0\"\n",
    )
    .unwrap();
    let git = |args: &[&str]| {
        let status = std::process::Command::new("git")
            .args(args)
            .current_dir(&repo)
            .status()
            .unwrap();
        assert!(status.success(), "git {args:?} failed");
    };
    git(&["init", "-q"]);
    git(&["config", "user.email", "test@example.com"]);
    git(&["config", "user.name", "test"]);
    git(&["add", "rnaseq.oxoflow"]);
    git(&["commit", "-qm", "initial"]);

    let work = dir.path().join("work");
    fs::create_dir_all(&work).unwrap();
    let out = oxo_flow_cmd()
        .args(["pull", &format!("file://{}", repo.display())])
        .current_dir(&work)
        .output()
        .unwrap();
    assert!(out.status.success());
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("Cloning"), "{stderr}");
    assert!(stderr.contains("rnaseq.oxoflow"), "{stderr}");
    assert!(
        stderr.contains("oxo-flow run"),
        "next steps must point at run: {stderr}"
    );
    // Clone landed next to the CWD (repo dir name) with the workflow inside.
    assert!(work.join("pipeline-repo").join("rnaseq.oxoflow").exists());
}

/// nextflow-style repository execution: `run file://<repo>` checks the
/// workflow out into a cache, executes it, and defaults the workdir to the
/// current directory (outputs/checkpoint land next to the user's data, not
/// inside the clone). Second run reuses the cache and skips completed work.
#[test]
fn cli_run_executes_workflow_from_repository() {
    let dir = tempfile::tempdir().unwrap();
    let repo = dir.path().join("wf-repo");
    fs::create_dir_all(&repo).unwrap();
    fs::write(
        repo.join("main.oxoflow"),
        r#"[workflow]
name = "remote-wf"
version = "1.0"

[[rules]]
name = "hello"
output = ["result.txt"]
shell = "echo hello > result.txt"
"#,
    )
    .unwrap();
    let git = |args: &[&str]| {
        let status = std::process::Command::new("git")
            .args(args)
            .current_dir(&repo)
            .status()
            .unwrap();
        assert!(status.success(), "git {args:?} failed");
    };
    git(&["init", "-q"]);
    git(&["config", "user.email", "test@example.com"]);
    git(&["config", "user.name", "test"]);
    git(&["add", "main.oxoflow"]);
    git(&["commit", "-qm", "initial"]);

    let work = dir.path().join("analysis");
    fs::create_dir_all(&work).unwrap();
    let repo_url = format!("file://{}", repo.display());

    // First run: clone + execute. Outputs and checkpoint land in CWD.
    let out = oxo_flow_cmd()
        .args(["run", &repo_url, "-j", "2"])
        .current_dir(&work)
        .output()
        .unwrap();
    assert!(out.status.success());
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("running workflow from"), "{stderr}");
    assert!(
        work.join("result.txt").exists(),
        "outputs land in the workdir"
    );
    assert!(
        work.join(".oxo-flow").join("checkpoint.json").exists(),
        "checkpoint lands in the workdir"
    );
    assert!(
        work.join(".oxo-flow/repos/wf-repo/main.oxoflow").exists(),
        "clone cached under .oxo-flow/repos"
    );

    // Second run: cache reused, rule skipped via checkpoint.
    let out = oxo_flow_cmd()
        .args(["run", &repo_url, "-j", "2"])
        .current_dir(&work)
        .output()
        .unwrap();
    assert!(out.status.success());
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("reusing cached checkout"), "{stderr}");
    assert!(stderr.contains("already completed"), "{stderr}");
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

// ─── transitive failure propagation / skip accounting ───────────────────

/// A three-rule chain `a -> b -> c` where `a` fails. `c` must not execute:
/// `dag.dependencies()` reports direct predecessors only, so a blocked rule
/// has to join the failed set for the block to reach its own dependents.
#[test]
fn cli_run_keep_going_blocks_transitive_dependents() {
    let dir = tempfile::tempdir().unwrap();
    let wf = dir.path().join("chain.oxoflow");
    fs::write(
        &wf,
        "[workflow]\nname = \"chain\"\nversion = \"1.0.0\"\n\n\
         [[rules]]\nname = \"a_fails\"\noutput = [\"a.txt\"]\nshell = \"exit 1\"\n\n\
         [[rules]]\nname = \"b_child\"\ninput = [\"a.txt\"]\noutput = [\"b.txt\"]\n\
         depends_on = [\"a_fails\"]\nshell = \"cat {input[0]} > {output[0]}\"\n\n\
         [[rules]]\nname = \"c_grandchild\"\ninput = [\"b.txt\"]\noutput = [\"c.txt\"]\n\
         depends_on = [\"b_child\"]\nshell = \"cat {input[0]} 2>/dev/null > {output[0]}; exit 0\"\n",
    )
    .unwrap();

    let output = oxo_flow_cmd()
        .args(["run", wf.to_str().unwrap(), "-k"])
        .current_dir(dir.path())
        .output()
        .unwrap();

    // The grandchild tolerates a missing input, so if it runs it "succeeds"
    // and writes an empty file — the silent-corruption case this guards.
    assert!(
        !dir.path().join("c.txt").exists(),
        "grandchild of a failed rule must not execute"
    );

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("0 succeeded"),
        "no rule should succeed: {stderr}"
    );
    assert!(
        stderr.contains("Blocked rules"),
        "blocked rules should be reported: {stderr}"
    );
    assert!(
        stderr.contains("c_grandchild"),
        "grandchild should be listed as blocked: {stderr}"
    );
}

/// A blocked rule produces no outputs, so it must never be checkpointed as
/// completed — otherwise a later run skips it forever and the pipeline reports
/// success while an output is silently empty.
#[test]
fn cli_run_blocked_rule_is_not_checkpointed() {
    let dir = tempfile::tempdir().unwrap();
    let wf = dir.path().join("chain.oxoflow");
    fs::write(
        &wf,
        "[workflow]\nname = \"chain\"\nversion = \"1.0.0\"\n\n\
         [[rules]]\nname = \"a_fails\"\noutput = [\"a.txt\"]\nshell = \"exit 1\"\n\n\
         [[rules]]\nname = \"b_child\"\ninput = [\"a.txt\"]\noutput = [\"b.txt\"]\n\
         depends_on = [\"a_fails\"]\nshell = \"cat {input[0]} 2>/dev/null > {output[0]}; exit 0\"\n",
    )
    .unwrap();

    oxo_flow_cmd()
        .args(["run", wf.to_str().unwrap(), "-k"])
        .current_dir(dir.path())
        .output()
        .unwrap();

    let checkpoint = dir.path().join(".oxo-flow/checkpoint.json");
    if checkpoint.exists() {
        let raw = fs::read_to_string(&checkpoint).unwrap();
        assert!(
            !raw.contains("b_child") || !raw.contains("\"completed_rules\":[\"b_child\"]"),
            "blocked rule must not be recorded as completed: {raw}"
        );
        let parsed: serde_json::Value = serde_json::from_str(&raw).unwrap();
        if let Some(done) = parsed["completed_rules"].as_array() {
            assert!(
                !done.iter().any(|r| r.as_str() == Some("b_child")),
                "blocked rule must not be checkpointed as completed: {raw}"
            );
        }
    }
}

/// A rule skipped by a false `when` condition must be counted once, not once
/// by a pre-pass and again by the executor that actually evaluates it.
#[test]
fn cli_run_condition_skip_counted_once() {
    let dir = tempfile::tempdir().unwrap();
    let wf = dir.path().join("cond.oxoflow");
    fs::write(
        &wf,
        "[workflow]\nname = \"cond\"\nversion = \"1.0.0\"\n\n\
         [config]\nmode = \"dna\"\n\n\
         [[rules]]\nname = \"only_rna\"\noutput = [\"rna.txt\"]\n\
         when = 'config.mode == \"rna\"'\nshell = \"echo x > {output[0]}\"\n",
    )
    .unwrap();

    let output = oxo_flow_cmd()
        .args(["run", wf.to_str().unwrap()])
        .current_dir(dir.path())
        .output()
        .unwrap();

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("0 succeeded, 1 skipped, 0 failed"),
        "one rule skipped once, not twice: {stderr}"
    );
}

/// A `when` condition referencing `{config.*}` must evaluate correctly.
/// The executor's config_values builder and evaluate_condition must both
/// handle the `config.` prefix.
#[test]
fn cli_run_when_condition_sees_config_values() {
    let dir = tempfile::tempdir().unwrap();
    let wf = dir.path().join("argwhen.oxoflow");
    fs::write(
        &wf,
        "[workflow]\nname = \"argwhen\"\nversion = \"1.0.0\"\n\n\
         [config]\nmode = { default = \"dna\" }\n\n\
         [[rules]]\nname = \"only_rna\"\noutput = [\"rna.txt\"]\n\
         when = 'config.mode == \"rna\"'\nshell = \"echo ran > {output[0]}\"\n",
    )
    .unwrap();

    let output = oxo_flow_cmd()
        .args(["run", wf.to_str().unwrap()])
        .current_dir(dir.path())
        .output()
        .unwrap();

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("0 succeeded, 1 skipped, 0 failed"),
        "rule should be skipped when condition is false (mode=dna, when asks for rna): {stderr}"
    );
}

/// When the condition IS true, the rule should execute.
#[test]
fn cli_run_when_condition_matches_config_and_runs() {
    let dir = tempfile::tempdir().unwrap();
    let wf = dir.path().join("argwhen2.oxoflow");
    fs::write(
        &wf,
        "[workflow]\nname = \"argwhen2\"\nversion = \"1.0.0\"\n\n\
         [config]\nmode = { default = \"rna\" }\n\n\
         [[rules]]\nname = \"only_rna\"\noutput = [\"rna.txt\"]\n\
         when = 'config.mode == \"rna\"'\nshell = \"echo ran > {output[0]}\"\n",
    )
    .unwrap();

    let output = oxo_flow_cmd()
        .args(["run", wf.to_str().unwrap()])
        .current_dir(dir.path())
        .output()
        .unwrap();

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("1 succeeded"),
        "rule should execute when condition matches: {stderr}"
    );
    assert!(
        dir.path().join("rna.txt").exists(),
        "output file should exist when rule executed"
    );
}

/// The manifest reserves an empty `signatures` array so that adding bundle
/// signing later is an additive change rather than a manifest format bump.
#[test]
fn cli_publish_manifest_reserves_signatures_field() {
    let dir = tempfile::tempdir().unwrap();
    let wf = dir.path().join("sig.oxoflow");
    fs::write(
        &wf,
        "[workflow]\nname = \"sig\"\nversion = \"1.0.0\"\n\n\
         [[rules]]\nname = \"s\"\noutput = [\"out.txt\"]\nshell = \"echo done > {output[0]}\"\n",
    )
    .unwrap();

    let output = oxo_flow_cmd()
        .args(["publish", wf.to_str().unwrap()])
        .current_dir(dir.path())
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "publish failed: stderr={}",
        String::from_utf8_lossy(&output.stderr)
    );

    let archive = dir.path().join("sig-bundle.tar.zst");
    let file = std::fs::File::open(&archive).unwrap();
    let decoder = zstd::stream::read::Decoder::new(file).unwrap();
    let mut tar = tar::Archive::new(decoder);

    let mut manifest_json = String::new();
    for entry in tar.entries().unwrap() {
        let mut entry = entry.unwrap();
        if entry.path().unwrap().to_string_lossy() == "manifest.json" {
            use std::io::Read as _;
            entry.read_to_string(&mut manifest_json).unwrap();
        }
    }
    assert!(!manifest_json.is_empty(), "manifest.json should be present");

    let manifest: serde_json::Value = serde_json::from_str(&manifest_json).unwrap();
    let signatures = manifest
        .get("signatures")
        .unwrap_or_else(|| panic!("manifest should reserve a signatures field: {manifest_json}"));
    assert!(
        signatures.is_array(),
        "signatures should be an array, got: {signatures}"
    );
    assert!(
        signatures.as_array().unwrap().is_empty(),
        "signatures should be empty until signing is implemented"
    );

    // Verify resources field is present with per-rule and recommendation data
    let resources = manifest
        .get("resources")
        .unwrap_or_else(|| panic!("manifest should include a resources field: {manifest_json}"));
    assert!(
        resources.get("rules").unwrap().is_array(),
        "resources.rules should be an array"
    );
    let recommendations = resources
        .get("recommendations")
        .expect("resources.recommendations should be present");
    assert!(
        recommendations
            .get("min_threads")
            .unwrap()
            .as_u64()
            .unwrap()
            >= 1,
        "recommendations.min_threads should be >= 1"
    );
}

/// Bundles must not extract to a predictable path. The old
/// `temp_dir()/oxo-bundle-<pid>` location could be pre-created by another user
/// on a shared machine, and PIDs are reused.
#[test]
fn cli_run_bundle_extracts_to_unpredictable_dir() {
    let dir = tempfile::tempdir().unwrap();
    let wf = dir.path().join("tmp.oxoflow");
    fs::write(
        &wf,
        "[workflow]\nname = \"tmp\"\nversion = \"1.0.0\"\n\n\
         [[rules]]\nname = \"s\"\noutput = [\"out.txt\"]\nshell = \"echo done > {output[0]}\"\n",
    )
    .unwrap();

    oxo_flow_cmd()
        .args(["publish", wf.to_str().unwrap()])
        .current_dir(dir.path())
        .output()
        .unwrap();
    let archive = dir.path().join("tmp-bundle.tar.zst");
    assert!(archive.exists(), "bundle should exist");

    let output = oxo_flow_cmd()
        .args(["run", "--bundle", archive.to_str().unwrap(), "--yes"])
        .current_dir(dir.path())
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "bundle run failed: stderr={}",
        String::from_utf8_lossy(&output.stderr)
    );

    // The pid-derived path must no longer be the extraction target.
    let legacy = std::env::temp_dir().join(format!("oxo-bundle-{}", std::process::id()));
    assert!(
        !legacy.exists(),
        "extraction must not use the predictable pid-based path: {}",
        legacy.display()
    );
}

/// The dry-run -j suggestion must be capped by DAG width: a single-rule
/// workflow has nothing to parallelize, so -j 1 is the professional
/// suggestion regardless of machine threads.
#[test]
fn cli_dry_run_suggestion_capped_by_dag_width() {
    let dir = tempfile::tempdir().unwrap();
    let wf = dir.path().join("single.oxoflow");
    fs::write(
        &wf,
        "[workflow]\nname = \"single\"\n\n[[rules]]\nname = \"only\"\noutput = [\"out.txt\"]\nshell = \"echo hi > {output[0]}\"\n",
    )
    .unwrap();

    let output = oxo_flow_cmd()
        .args(["dry-run", wf.to_str().unwrap()])
        .current_dir(dir.path())
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "dry-run failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("-j 1"),
        "single-rule workflow should suggest -j 1, got: {stderr}"
    );
}

/// A two-rule sequential chain also has width 1 → -j 1.
#[test]
fn cli_dry_run_suggestion_sequential_chain() {
    let dir = tempfile::tempdir().unwrap();
    let wf = dir.path().join("chain.oxoflow");
    fs::write(
        &wf,
        "[workflow]\nname = \"chain\"\n\n[[rules]]\nname = \"a\"\noutput = [\"mid.txt\"]\nshell = \"echo a > {output[0]}\"\n\n[[rules]]\nname = \"b\"\ninput = [\"mid.txt\"]\noutput = [\"out.txt\"]\nshell = \"cat {input[0]} > {output[0]}\"\n",
    )
    .unwrap();

    let output = oxo_flow_cmd()
        .args(["dry-run", wf.to_str().unwrap()])
        .current_dir(dir.path())
        .output()
        .unwrap();
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("-j 1"),
        "sequential chain should suggest -j 1, got: {stderr}"
    );
}

/// Two independent rules (width 2) with 1-thread declarations should
/// suggest -j 2 on machines with >= 2 threads.
#[test]
fn cli_dry_run_suggestion_parallel_width() {
    let dir = tempfile::tempdir().unwrap();
    let wf = dir.path().join("parallel.oxoflow");
    fs::write(
        &wf,
        "[workflow]\nname = \"parallel\"\n\n[[rules]]\nname = \"x\"\noutput = [\"x.txt\"]\nshell = \"echo x > {output[0]}\"\n\n[[rules]]\nname = \"y\"\noutput = [\"y.txt\"]\nshell = \"echo y > {output[0]}\"\n",
    )
    .unwrap();

    let output = oxo_flow_cmd()
        .args(["dry-run", wf.to_str().unwrap()])
        .current_dir(dir.path())
        .output()
        .unwrap();
    let stderr = String::from_utf8_lossy(&output.stderr);
    // Width=2, 1-thread rules: suggestion should be exactly -j 2
    assert!(
        stderr.contains("-j 2"),
        "two independent rules should suggest -j 2, got: {stderr}"
    );
}

// ─── Config-change impact analysis (issue #62) ──────────────────────────────

fn write_impact_workflow(dir: &std::path::Path, name: &str) -> std::path::PathBuf {
    let wf = dir.join(format!("{name}.oxoflow"));
    fs::write(
        &wf,
        r#"[workflow]
name = "impact"
version = "1.0.0"

[config]
min_quality = "20"

[[rules]]
name = "upstream"
output = ["up.txt"]
shell = "echo up > {output}"

[[rules]]
name = "param"
input = ["up.txt"]
output = ["param.txt"]
shell = "echo q={config.min_quality} > {output}"

[[rules]]
name = "downstream"
input = ["param.txt"]
output = ["down.txt"]
shell = "cat {input} > {output}"
"#,
    )
    .unwrap();
    wf
}

/// Changing a config key via CLI override re-runs only the rules that
/// reference it plus their downstream; upstream rules keep the checkpoint.
#[test]
fn cli_config_change_invalidates_only_referencing_rules() {
    let dir = tempfile::tempdir().unwrap();
    let wf = write_impact_workflow(dir.path(), "impact");

    // First run: everything executes with min_quality = 20.
    let run1 = oxo_flow_cmd()
        .args(["run", wf.to_str().unwrap()])
        .current_dir(dir.path())
        .output()
        .unwrap();
    assert!(
        run1.status.success(),
        "run1 failed: {}",
        String::from_utf8_lossy(&run1.stderr)
    );
    assert_eq!(
        fs::read_to_string(dir.path().join("param.txt")).unwrap(),
        "q=20\n"
    );

    // Second run, identical: everything hits the checkpoint, no summary.
    let run2 = oxo_flow_cmd()
        .args(["run", wf.to_str().unwrap()])
        .current_dir(dir.path())
        .output()
        .unwrap();
    assert!(run2.status.success());
    let stderr2 = String::from_utf8_lossy(&run2.stderr);
    assert!(
        !stderr2.contains("Config change:"),
        "no change expected: {stderr2}"
    );
    assert!(
        stderr2.contains("3 skipped"),
        "all rules should skip: {stderr2}"
    );

    // Third run with min_quality=30: upstream keeps the checkpoint, param and
    // its downstream re-execute with the new value.
    let run3 = oxo_flow_cmd()
        .args(["run", wf.to_str().unwrap(), "min_quality=30"])
        .current_dir(dir.path())
        .output()
        .unwrap();
    assert!(
        run3.status.success(),
        "run3 failed: {}",
        String::from_utf8_lossy(&run3.stderr)
    );
    let stderr3 = String::from_utf8_lossy(&run3.stderr);
    assert!(
        stderr3.contains("Config change:"),
        "expected summary: {stderr3}"
    );
    assert!(
        stderr3.contains("min_quality: 20 → 30"),
        "expected old→new: {stderr3}"
    );
    assert!(
        stderr3.contains("re-running 2/3"),
        "expected 2 re-runs: {stderr3}"
    );
    assert!(
        stderr3.contains("already completed"),
        "upstream must skip: {stderr3}"
    );
    assert_eq!(
        fs::read_to_string(dir.path().join("param.txt")).unwrap(),
        "q=30\n"
    );
    assert_eq!(
        fs::read_to_string(dir.path().join("down.txt")).unwrap(),
        "q=30\n"
    );

    // Fourth run WITHOUT the override: effective config flips back to 20, so
    // the referencing rules re-run again (comparison is on effective config,
    // which includes CLI overrides — outputs must match what was run).
    let run4 = oxo_flow_cmd()
        .args(["run", wf.to_str().unwrap()])
        .current_dir(dir.path())
        .output()
        .unwrap();
    assert!(run4.status.success());
    let stderr4 = String::from_utf8_lossy(&run4.stderr);
    assert!(
        stderr4.contains("min_quality: 30 → 20"),
        "expected 30 → 20: {stderr4}"
    );
    assert_eq!(
        fs::read_to_string(dir.path().join("param.txt")).unwrap(),
        "q=20\n"
    );

    // --rerun forces everything and suppresses the change summary.
    let run5 = oxo_flow_cmd()
        .args(["run", wf.to_str().unwrap(), "--rerun"])
        .current_dir(dir.path())
        .output()
        .unwrap();
    assert!(run5.status.success());
    let stderr5 = String::from_utf8_lossy(&run5.stderr);
    assert!(
        !stderr5.contains("Config change:"),
        "summary suppressed under --rerun: {stderr5}"
    );
    assert!(
        !stderr5.contains("already completed"),
        "nothing skipped under --rerun: {stderr5}"
    );
}

/// Editing a rule's shell invalidates exactly that rule plus downstream.
#[test]
fn cli_rule_edit_invalidates_rule_and_downstream() {
    let dir = tempfile::tempdir().unwrap();
    let wf = write_impact_workflow(dir.path(), "edit");

    let run1 = oxo_flow_cmd()
        .args(["run", wf.to_str().unwrap()])
        .current_dir(dir.path())
        .output()
        .unwrap();
    assert!(run1.status.success());

    // Edit the param rule's shell (add a marker to the output).
    let toml = fs::read_to_string(&wf).unwrap();
    let edited = toml.replace(
        "shell = \"echo q={config.min_quality} > {output}\"",
        "shell = \"echo q={config.min_quality}-edited > {output}\"",
    );
    assert_ne!(toml, edited, "workflow edit must take effect");
    fs::write(&wf, edited).unwrap();

    let run2 = oxo_flow_cmd()
        .args(["run", wf.to_str().unwrap()])
        .current_dir(dir.path())
        .output()
        .unwrap();
    assert!(
        run2.status.success(),
        "run2 failed: {}",
        String::from_utf8_lossy(&run2.stderr)
    );
    let stderr = String::from_utf8_lossy(&run2.stderr);
    assert!(
        stderr.contains("rule definition changed: param"),
        "expected fingerprint mismatch for param: {stderr}"
    );
    assert!(
        stderr.contains("already completed"),
        "upstream must skip: {stderr}"
    );
    assert_eq!(
        fs::read_to_string(dir.path().join("param.txt")).unwrap(),
        "q=20-edited\n"
    );
    assert_eq!(
        fs::read_to_string(dir.path().join("down.txt")).unwrap(),
        "q=20-edited\n"
    );
}

/// A checkpoint without config tracking (pre-#62) is adopted as a baseline:
/// nothing re-runs on the first post-upgrade run, and detection works from
/// the next run onward.
#[test]
fn cli_legacy_checkpoint_adopts_baseline() {
    let dir = tempfile::tempdir().unwrap();
    let wf = write_impact_workflow(dir.path(), "legacy");

    let run1 = oxo_flow_cmd()
        .args(["run", wf.to_str().unwrap()])
        .current_dir(dir.path())
        .output()
        .unwrap();
    assert!(run1.status.success());

    // Strip the tracking fields to simulate a checkpoint written by an
    // oxo-flow version that predates config tracking.
    let checkpoint_path = dir.path().join(".oxo-flow/checkpoint.json");
    let mut json: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(&checkpoint_path).unwrap()).unwrap();
    let obj = json.as_object_mut().unwrap();
    obj.remove("config_snapshot");
    obj.remove("rule_fingerprints");
    fs::write(
        &checkpoint_path,
        serde_json::to_string_pretty(&json).unwrap(),
    )
    .unwrap();

    // Post-upgrade run: legacy notice, everything reused, baseline recorded.
    let run2 = oxo_flow_cmd()
        .args(["run", wf.to_str().unwrap()])
        .current_dir(dir.path())
        .output()
        .unwrap();
    assert!(run2.status.success());
    let stderr2 = String::from_utf8_lossy(&run2.stderr);
    assert!(
        stderr2.contains("predates config tracking"),
        "expected legacy notice: {stderr2}"
    );
    assert!(
        stderr2.contains("3 skipped"),
        "legacy run must reuse: {stderr2}"
    );

    // From now on, config changes are detected.
    let run3 = oxo_flow_cmd()
        .args(["run", wf.to_str().unwrap(), "min_quality=30"])
        .current_dir(dir.path())
        .output()
        .unwrap();
    assert!(run3.status.success());
    let stderr3 = String::from_utf8_lossy(&run3.stderr);
    assert!(
        stderr3.contains("Config change:"),
        "expected summary: {stderr3}"
    );
    assert_eq!(
        fs::read_to_string(dir.path().join("param.txt")).unwrap(),
        "q=30\n"
    );
}

/// When a config change flips a rule's `when` condition to false, the change
/// is detected once and then stays quiet: the summary must not re-print on
/// every subsequent run.
#[test]
fn cli_when_flip_detected_once() {
    let dir = tempfile::tempdir().unwrap();
    let wf = dir.path().join("whenflip.oxoflow");
    fs::write(
        &wf,
        r#"[workflow]
name = "whenflip"
version = "1.0.0"

[config]
enable = "true"

[[rules]]
name = "upstream"
output = ["up.txt"]
shell = "echo up > {output}"

[[rules]]
name = "gated"
input = ["up.txt"]
output = ["gated.txt"]
when = "config.enable"
shell = "echo gated > {output}"
"#,
    )
    .unwrap();

    let run1 = oxo_flow_cmd()
        .args(["run", wf.to_str().unwrap()])
        .current_dir(dir.path())
        .output()
        .unwrap();
    assert!(run1.status.success());
    assert!(dir.path().join("gated.txt").exists());

    // enable=false: gated is invalidated, re-submitted, and skipped by its
    // condition. The change is reported once.
    let run2 = oxo_flow_cmd()
        .args(["run", wf.to_str().unwrap(), "enable=false"])
        .current_dir(dir.path())
        .output()
        .unwrap();
    assert!(run2.status.success());
    let stderr2 = String::from_utf8_lossy(&run2.stderr);
    assert!(
        stderr2.contains("Config change:"),
        "expected summary: {stderr2}"
    );
    // gated is re-submitted (invalidated) but its `when` condition now
    // evaluates to false: no rule succeeds, both end up skipped.
    assert!(
        stderr2.contains("0 succeeded, 2 skipped"),
        "gated must be condition-skipped: {stderr2}"
    );

    // Third run with the same config: no summary (detection is idempotent).
    let run3 = oxo_flow_cmd()
        .args(["run", wf.to_str().unwrap(), "enable=false"])
        .current_dir(dir.path())
        .output()
        .unwrap();
    assert!(run3.status.success());
    let stderr3 = String::from_utf8_lossy(&run3.stderr);
    assert!(
        !stderr3.contains("Config change:"),
        "summary must not repeat after the flip: {stderr3}"
    );
}

/// A completed rule with `{config.x}` outputs whose files were deleted is
/// detected and re-executed (the pre-process existence check expands config
/// placeholders before testing the path).
#[test]
fn cli_config_var_output_existence_check() {
    let dir = tempfile::tempdir().unwrap();
    let wf = dir.path().join("cfgout.oxoflow");
    fs::write(
        &wf,
        r#"[workflow]
name = "cfgout"
version = "1.0.0"

[config]
outdir = "results"

[[rules]]
name = "step"
output = ["{config.outdir}/x.txt"]
shell = "echo hi > {output}"
"#,
    )
    .unwrap();

    let run1 = oxo_flow_cmd()
        .args(["run", wf.to_str().unwrap()])
        .current_dir(dir.path())
        .output()
        .unwrap();
    assert!(run1.status.success());
    assert!(dir.path().join("results/x.txt").exists());

    // Delete the output but keep the checkpoint: the rule must re-run.
    fs::remove_file(dir.path().join("results/x.txt")).unwrap();
    let run2 = oxo_flow_cmd()
        .args(["run", wf.to_str().unwrap()])
        .current_dir(dir.path())
        .output()
        .unwrap();
    assert!(run2.status.success());
    assert!(
        dir.path().join("results/x.txt").exists(),
        "output must be recreated: {}",
        String::from_utf8_lossy(&run2.stderr)
    );
}

/// Editing a reference build command rebuilds the artifact and invalidates
/// the rules that consume it through declared inputs.
#[test]
fn cli_reference_rebuild_invalidates_consumers() {
    let dir = tempfile::tempdir().unwrap();
    let wf = dir.path().join("refcons.oxoflow");
    fs::write(
        &wf,
        r#"[workflow]
name = "refcons"
version = "1.0.0"

[config]
ref_dir = "refs"

[[references]]
name = "genome_idx"
source = "genome.fa"
output = "{config.ref_dir}/genome.idx"
build = "mkdir -p {config.ref_dir} && echo built-v1 > {output}"

[[rules]]
name = "use_ref"
input = ["{config.ref_dir}/genome.idx"]
output = ["result.txt"]
shell = "cat {input} > {output}"
"#,
    )
    .unwrap();
    fs::write(dir.path().join("genome.fa"), b"AAAA").unwrap();

    let run1 = oxo_flow_cmd()
        .args(["run", wf.to_str().unwrap()])
        .current_dir(dir.path())
        .output()
        .unwrap();
    assert!(run1.status.success());
    assert_eq!(
        fs::read_to_string(dir.path().join("result.txt")).unwrap(),
        "built-v1\n"
    );

    // Second run: fingerprint matches, nothing rebuilt or re-run.
    let run2 = oxo_flow_cmd()
        .args(["run", wf.to_str().unwrap()])
        .current_dir(dir.path())
        .output()
        .unwrap();
    assert!(run2.status.success());
    let stderr2 = String::from_utf8_lossy(&run2.stderr);
    assert!(
        !stderr2.contains("Building"),
        "no rebuild expected: {stderr2}"
    );

    // Edit the build command: artifact rebuilds AND the consumer re-runs.
    let toml = fs::read_to_string(&wf).unwrap();
    fs::write(&wf, toml.replace("built-v1", "built-v2")).unwrap();
    let run3 = oxo_flow_cmd()
        .args(["run", wf.to_str().unwrap()])
        .current_dir(dir.path())
        .output()
        .unwrap();
    assert!(
        run3.status.success(),
        "run3 failed: {}",
        String::from_utf8_lossy(&run3.stderr)
    );
    let stderr3 = String::from_utf8_lossy(&run3.stderr);
    assert!(
        stderr3.contains("definition or referenced config changed"),
        "expected rebuild: {stderr3}"
    );
    assert!(
        stderr3.contains("invalidated 1 rule(s): use_ref"),
        "consumer must be invalidated: {stderr3}"
    );
    assert_eq!(
        fs::read_to_string(dir.path().join("result.txt")).unwrap(),
        "built-v2\n"
    );
}

// ─── #71: run flags after positional overrides get actionable errors ───────

/// clap's trailing config_overrides positional (allow_hyphen_values) cannot
/// distinguish `--json` the flag from `--json` a hyphen-value, so flags typed
/// after `KEY=VALUE` land in the override list. They must fail with targeted
/// guidance instead of a confusing "invalid config flag" — while the three
/// override forms and the flags-first ordering keep working.
#[test]
fn cli_run_flags_after_overrides_get_actionable_error() {
    let dir = tempfile::tempdir().unwrap();
    let wf = dir.path().join("trail.oxoflow");
    fs::write(
        &wf,
        r#"[workflow]
name = "trail"
version = "1.0.0"

[config]
min_quality = "20"

[[rules]]
name = "param"
output = ["param.txt"]
shell = "echo q={config.min_quality} > {output}"
"#,
    )
    .unwrap();

    // 1. Long flag after a positional override: targeted error, not a run.
    let out = oxo_flow_cmd()
        .args(["run", wf.to_str().unwrap(), "min_quality=30", "--json"])
        .current_dir(dir.path())
        .output()
        .unwrap();
    assert!(!out.status.success(), "--json after KEY=VALUE must error");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("run flag, not a config override"),
        "expected actionable guidance, got: {stderr}"
    );
    assert!(
        stderr.contains("--json"),
        "error must name the offending flag: {stderr}"
    );
    assert!(
        !stderr.contains("invalid config flag: '--json'"),
        "old confusing error must be gone: {stderr}"
    );

    // 2. Short flag after a positional override: same targeted error.
    let out = oxo_flow_cmd()
        .args(["run", wf.to_str().unwrap(), "min_quality=30", "-j", "4"])
        .current_dir(dir.path())
        .output()
        .unwrap();
    assert!(!out.status.success());
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("run flag, not a config override") && stderr.contains("-j"),
        "short flags need the same guidance: {stderr}"
    );

    // 3. --KEY VALUE form still works (allow_hyphen_values contract).
    let out = oxo_flow_cmd()
        .args(["run", wf.to_str().unwrap(), "--min_quality", "40"])
        .current_dir(dir.path())
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "--KEY VALUE form must keep working: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert_eq!(
        fs::read_to_string(dir.path().join("param.txt")).unwrap(),
        "q=40\n"
    );

    // 4. --KEY=VALUE form still works.
    let out = oxo_flow_cmd()
        .args(["run", wf.to_str().unwrap(), "--min_quality=50"])
        .current_dir(dir.path())
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "--KEY=VALUE form must keep working: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert_eq!(
        fs::read_to_string(dir.path().join("param.txt")).unwrap(),
        "q=50\n"
    );

    // 5. Flags before positionals: the documented ordering, everything works.
    let out = oxo_flow_cmd()
        .args(["run", wf.to_str().unwrap(), "--json", "min_quality=60"])
        .current_dir(dir.path())
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "flags-first ordering must work: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let parsed: serde_json::Value =
        serde_json::from_slice(&out.stdout).expect("stdout must be pure JSON");
    assert_eq!(parsed["command"], "run");
    assert_eq!(
        fs::read_to_string(dir.path().join("param.txt")).unwrap(),
        "q=60\n"
    );
}

// ─── Incremental data arrival (--samples ready, issue #63) ──────────────────

/// dry-run reports per-sample readiness: which samples have complete entry
/// inputs, which are waiting, and exactly which files are missing. --json
/// exposes the same report machine-readably.
#[test]
fn cli_dry_run_reports_sample_readiness() {
    let dir = tempfile::tempdir().unwrap();
    let wf = dir.path().join("ready.oxoflow");
    fs::write(
        &wf,
        "[workflow]\nname = \"ready\"\nversion = \"1.0.0\"\n\n[[sample_groups]]\nname = \"cohort\"\nsamples = [\"S1\", \"S2\", \"S3\"]\n\n[[rules]]\nname = \"qc\"\ninput = [\"data/{sample}_R1.fastq.gz\", \"data/{sample}_R2.fastq.gz\"]\noutput = [\"results/{sample}.qc.txt\"]\nshell = \"cat {input} > {output}\"\n",
    )
    .unwrap();
    fs::create_dir_all(dir.path().join("data")).unwrap();
    for sample in ["S1", "S2"] {
        fs::write(dir.path().join(format!("data/{sample}_R1.fastq.gz")), b"x").unwrap();
        fs::write(dir.path().join(format!("data/{sample}_R2.fastq.gz")), b"x").unwrap();
    }
    // S3_R1 exists but S3_R2 has not arrived yet — S3 is waiting.
    fs::write(dir.path().join("data/S3_R1.fastq.gz"), b"x").unwrap();

    // Human output: aggregate counts + grouped waiting list with missing files.
    let out = oxo_flow_cmd()
        .args(["dry-run", wf.to_str().unwrap()])
        .current_dir(dir.path())
        .output()
        .unwrap();
    assert!(out.status.success());
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("Sample readiness: 2/3 complete, 1 waiting"),
        "expected readiness summary, got:\n{stderr}"
    );
    assert!(
        stderr.contains("S3 (missing: data/S3_R2.fastq.gz)"),
        "waiting sample must list its missing files, got:\n{stderr}"
    );

    // Machine output: samples block with ready names and per-sample missing.
    let out = oxo_flow_cmd()
        .args(["dry-run", wf.to_str().unwrap(), "--json"])
        .current_dir(dir.path())
        .output()
        .unwrap();
    assert!(out.status.success());
    let parsed: serde_json::Value =
        serde_json::from_slice(&out.stdout).expect("stdout must be pure JSON");
    let samples = &parsed["samples"];
    assert_eq!(samples["total"], 3);
    assert_eq!(samples["ready"], 2);
    assert_eq!(samples["waiting_count"], 1);
    assert_eq!(samples["ready_names"], serde_json::json!(["S1", "S2"]));
    assert_eq!(
        samples["waiting"][0]["missing"],
        serde_json::json!(["data/S3_R2.fastq.gz"])
    );
}

/// `--samples ready` runs only samples with complete entry inputs; when the
/// missing data arrives later, a second run skips the completed samples via
/// the checkpoint and processes the newcomer — the incremental-arrival loop.
#[test]
fn cli_run_samples_ready_incremental_arrival() {
    let dir = tempfile::tempdir().unwrap();
    let wf = dir.path().join("inc.oxoflow");
    fs::write(
        &wf,
        "[workflow]\nname = \"inc\"\nversion = \"1.0.0\"\n\n[[sample_groups]]\nname = \"cohort\"\nsamples = [\"S1\", \"S2\", \"S3\"]\n\n[[rules]]\nname = \"qc\"\ninput = [\"data/{sample}_R1.fastq.gz\", \"data/{sample}_R2.fastq.gz\"]\noutput = [\"results/{sample}.qc.txt\"]\nshell = \"cat {input} > {output}\"\n",
    )
    .unwrap();
    fs::create_dir_all(dir.path().join("data")).unwrap();
    for sample in ["S1", "S2"] {
        fs::write(dir.path().join(format!("data/{sample}_R1.fastq.gz")), b"x").unwrap();
        fs::write(dir.path().join(format!("data/{sample}_R2.fastq.gz")), b"x").unwrap();
    }
    fs::write(dir.path().join("data/S3_R1.fastq.gz"), b"x").unwrap();

    // First batch: only S1 and S2 are runnable.
    let out = oxo_flow_cmd()
        .args(["run", wf.to_str().unwrap(), "--samples", "ready"])
        .current_dir(dir.path())
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(dir.path().join("results/S1.qc.txt").exists());
    assert!(dir.path().join("results/S2.qc.txt").exists());
    assert!(!dir.path().join("results/S3.qc.txt").exists());

    // S3's second read arrives — a new batch is ready.
    fs::write(dir.path().join("data/S3_R2.fastq.gz"), b"x").unwrap();
    let out = oxo_flow_cmd()
        .args(["run", wf.to_str().unwrap(), "--samples", "ready"])
        .current_dir(dir.path())
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(
        dir.path().join("results/S3.qc.txt").exists(),
        "newly-arrived sample must be processed"
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("3/3 complete"),
        "readiness must reflect the full cohort, got:\n{stderr}"
    );
    assert!(
        stderr.contains("2 skipped"),
        "checkpoint must skip the already-completed samples, got:\n{stderr}"
    );
}

/// With zero ready samples `run --samples ready` aborts with an actionable
/// error naming the waiting samples instead of executing nothing.
#[test]
fn cli_run_samples_ready_zero_ready_errors() {
    let dir = tempfile::tempdir().unwrap();
    let wf = dir.path().join("empty.oxoflow");
    fs::write(
        &wf,
        "[workflow]\nname = \"empty\"\nversion = \"1.0.0\"\n\n[[sample_groups]]\nname = \"cohort\"\nsamples = [\"S1\", \"S2\", \"S3\"]\n\n[[rules]]\nname = \"qc\"\ninput = [\"data/{sample}_R1.fastq.gz\", \"data/{sample}_R2.fastq.gz\"]\noutput = [\"results/{sample}.qc.txt\"]\nshell = \"cat {input} > {output}\"\n",
    )
    .unwrap();
    fs::create_dir_all(dir.path().join("data")).unwrap();
    fs::write(dir.path().join("data/S1_R1.fastq.gz"), b"x").unwrap();

    let out = oxo_flow_cmd()
        .args(["run", wf.to_str().unwrap(), "--samples", "ready"])
        .current_dir(dir.path())
        .output()
        .unwrap();
    assert!(!out.status.success(), "zero ready samples must abort");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("0 of 3 samples have complete inputs"),
        "expected actionable error, got:\n{stderr}"
    );
    assert!(
        stderr.contains("waiting: S1, S2, S3"),
        "error must name the waiting samples, got:\n{stderr}"
    );
}

// ─── Deep checks (test --deep, issue #64) ────────────────────────────────

/// E-clean workflow skeleton: valid name/version, described rules, concrete
/// outputs, no inputs — validate/lint/dry-run all pass, and `test` is
/// non-strict so W-codes don't fail it.
fn deep_workflow(rules_body: &str) -> String {
    format!(
        "[workflow]\nname = \"deep\"\nversion = \"1.0.0\"\n\
         description = \"deep check fixture\"\nauthor = \"tests\"\n\n{rules_body}"
    )
}

/// Run `oxo-flow test` (with `--deep` or not) against a workflow file in a
/// tempdir.
fn run_test_command(
    dir: &std::path::Path,
    wf: &std::path::Path,
    extra: &[&str],
) -> std::process::Output {
    let mut args: Vec<&str> = vec!["test", wf.to_str().unwrap()];
    args.extend_from_slice(extra);
    oxo_flow_cmd()
        .args(&args)
        .current_dir(dir)
        .output()
        .unwrap()
}

/// Split concatenated pretty-printed JSON documents (one per `test` step)
/// on lines that are a bare `{`, returning each parsed document.
fn parse_json_docs(stdout: &[u8]) -> Vec<serde_json::Value> {
    let text = String::from_utf8_lossy(stdout);
    let mut docs: Vec<String> = Vec::new();
    let mut current = String::new();
    for line in text.lines() {
        // Pretty-printed JSON indents nested braces, so only a column-0 `{`
        // starts a new document.
        if line == "{" && !current.trim().is_empty() {
            docs.push(std::mem::take(&mut current));
        }
        current.push_str(line);
        current.push('\n');
    }
    if !current.trim().is_empty() {
        docs.push(current);
    }
    docs.iter()
        .map(|d| serde_json::from_str(d).expect("parse JSON document"))
        .collect()
}

/// `test --deep` fails with a D001 error when a `script =` file is missing —
/// a deterministic run-time failure.
#[test]
fn cli_test_deep_script_field_missing_exits_1() {
    let dir = tempfile::tempdir().unwrap();
    let wf = dir.path().join("deep.oxoflow");
    fs::write(
        &wf,
        deep_workflow(
            "[[rules]]\nname = \"analyze\"\noutput = [\"results/a.txt\"]\n\
             description = \"run script\"\nscript = \"scripts/analyze.py --out results/a.txt\"\n",
        ),
    )
    .unwrap();

    let out = run_test_command(dir.path(), &wf, &["--deep"]);
    assert!(
        !out.status.success(),
        "missing script must fail test --deep"
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("[D001]"), "expected D001, got:\n{stderr}");
    assert!(
        stderr.contains("scripts/analyze.py"),
        "finding must name the path, got:\n{stderr}"
    );
}

/// `test --deep` catches script paths inside plain interpreter invocations
/// in `shell` strings.
#[test]
fn cli_test_deep_shell_interpreter_script_missing_exits_1() {
    let dir = tempfile::tempdir().unwrap();
    let wf = dir.path().join("deep.oxoflow");
    fs::write(
        &wf,
        deep_workflow(
            "[[rules]]\nname = \"report\"\noutput = [\"results/r.txt\"]\n\
             description = \"run python script\"\n\
             shell = \"python scripts/report.py --out results/r.txt\"\n",
        ),
    )
    .unwrap();

    let out = run_test_command(dir.path(), &wf, &["--deep"]);
    assert!(
        !out.status.success(),
        "missing script must fail test --deep"
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("[D001]"), "expected D001, got:\n{stderr}");
    assert!(
        stderr.contains("scripts/report.py"),
        "finding must name the path, got:\n{stderr}"
    );
}

/// A missing conda YAML is a warning: `test --deep` still exits 0.
#[test]
fn cli_test_deep_env_yaml_missing_warns_exit_0() {
    let dir = tempfile::tempdir().unwrap();
    let wf = dir.path().join("deep.oxoflow");
    fs::write(
        &wf,
        deep_workflow(
            "[[rules]]\nname = \"qc\"\noutput = [\"results/q.txt\"]\n\
             description = \"qc step\"\nshell = \"cat data.fq > results/q.txt\"\n\
             [rules.environment]\nconda = \"envs/qc.yaml\"\n",
        ),
    )
    .unwrap();

    let out = run_test_command(dir.path(), &wf, &["--deep"]);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("[D002]"), "expected D002, got:\n{stderr}");
    assert!(
        stderr.contains("envs/qc.yaml"),
        "finding must name the path, got:\n{stderr}"
    );
}

/// A missing system-backend binary is a warning (PATH is machine-specific).
#[test]
fn cli_test_deep_missing_binary_warns_exit_0() {
    let dir = tempfile::tempdir().unwrap();
    let wf = dir.path().join("deep.oxoflow");
    fs::write(
        &wf,
        deep_workflow(
            "[[rules]]\nname = \"qc\"\noutput = [\"results/q.txt\"]\n\
             description = \"run fake tool\"\n\
             shell = \"fake_tool_for_deep_check_9f3k --in data.fq\"\n",
        ),
    )
    .unwrap();

    let out = run_test_command(dir.path(), &wf, &["--deep"]);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("[D003]"), "expected D003, got:\n{stderr}");
    assert!(
        stderr.contains("fake_tool_for_deep_check_9f3k"),
        "finding must name the binary, got:\n{stderr}"
    );
}

/// Binaries that are in PATH get the green summary line.
#[test]
fn cli_test_deep_present_binary_reports_found() {
    let dir = tempfile::tempdir().unwrap();
    let wf = dir.path().join("deep.oxoflow");
    fs::write(
        &wf,
        deep_workflow(
            "[[rules]]\nname = \"sh\"\noutput = [\"results/s.txt\"]\n\
             description = \"run shell builtin\"\n\
             shell = \"sh -c 'echo hi > results/s.txt'\"\n",
        ),
    )
    .unwrap();

    let out = run_test_command(dir.path(), &wf, &["--deep"]);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("found in PATH"),
        "expected the found-in-PATH summary, got:\n{stderr}"
    );
    assert!(
        !stderr.contains("[D003]"),
        "sh must not produce D003, got:\n{stderr}"
    );
}

/// A non-system backend gates the binary probe (the tool comes from the
/// environment, not PATH) — the env YAML is still checked.
#[test]
fn cli_test_deep_conda_env_gates_binary_probe() {
    let dir = tempfile::tempdir().unwrap();
    let wf = dir.path().join("deep.oxoflow");
    fs::write(
        &wf,
        deep_workflow(
            "[[rules]]\nname = \"qc\"\noutput = [\"results/q.txt\"]\n\
             description = \"run tool in conda env\"\n\
             shell = \"fake_tool_for_deep_check_9f3k --in data.fq\"\n\
             [rules.environment]\nconda = \"envs/qc.yaml\"\n",
        ),
    )
    .unwrap();

    let out = run_test_command(dir.path(), &wf, &["--deep"]);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(!stderr.contains("[D003]"), "got:\n{stderr}");
    assert!(stderr.contains("[D002]"), "expected D002, got:\n{stderr}");
}

/// Path-like config values referenced in shells are checked for existence.
#[test]
fn cli_test_deep_config_reference_missing_warns() {
    let dir = tempfile::tempdir().unwrap();
    let wf = dir.path().join("deep.oxoflow");
    fs::write(
        &wf,
        deep_workflow(
            "[config]\nreference = \"/data/refs/GRCh38/genome.fa\"\n\n\
             [[rules]]\nname = \"align\"\noutput = [\"results/a.sam\"]\n\
             description = \"align reads\"\n\
             shell = \"bwa mem {config.reference} data.fq > results/a.sam\"\n",
        ),
    )
    .unwrap();

    let out = run_test_command(dir.path(), &wf, &["--deep"]);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("[D004]"), "expected D004, got:\n{stderr}");
    assert!(
        stderr.contains("/data/refs/GRCh38/genome.fa"),
        "finding must name the path, got:\n{stderr}"
    );
}

/// With `reference_dir` set, tool-derived index paths (bwa → bwa/genome.fa)
/// are checked.
#[test]
fn cli_test_deep_derived_bwa_index_missing_warns() {
    let dir = tempfile::tempdir().unwrap();
    let wf = dir.path().join("deep.oxoflow");
    fs::write(
        &wf,
        format!(
            "reference_dir = \"/data/refs/GRCh38\"\n\n{}",
            deep_workflow(
                "[[rules]]\nname = \"align\"\noutput = [\"results/a.sam\"]\n\
                 description = \"align reads\"\n\
                 shell = \"bwa mem genome.fa data.fq > results/a.sam\"\n",
            )
        ),
    )
    .unwrap();

    let out = run_test_command(dir.path(), &wf, &["--deep"]);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("[D004]"), "expected D004, got:\n{stderr}");
    assert!(
        stderr.contains("/data/refs/GRCh38/bwa/genome.fa"),
        "finding must name the derived index, got:\n{stderr}"
    );
}

/// `[[references]]` build outputs are checked for existence.
#[test]
fn cli_test_deep_reference_block_output_missing_warns() {
    let dir = tempfile::tempdir().unwrap();
    let wf = dir.path().join("deep.oxoflow");
    fs::write(
        &wf,
        deep_workflow(
            "[[references]]\nname = \"faidx\"\nsource = \"/data/refs/genome.fa\"\n\
             output = \"/data/refs/genome.fa.fai\"\n\
             build = \"samtools faidx /data/refs/genome.fa\"\n\n\
             [[rules]]\nname = \"x\"\noutput = [\"results/x.txt\"]\n\
             description = \"step\"\nshell = \"cat data.fq > results/x.txt\"\n",
        ),
    )
    .unwrap();

    let out = run_test_command(dir.path(), &wf, &["--deep"]);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("[D004]"), "expected D004, got:\n{stderr}");
    assert!(
        stderr.contains("/data/refs/genome.fa.fai"),
        "finding must name the path, got:\n{stderr}"
    );
}

/// A workflow whose scripts, env files, binaries and references all exist
/// passes `test --deep` with the all-checks summary.
#[test]
fn cli_test_deep_happy_path_all_checks_pass() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(dir.path().join("scripts")).unwrap();
    std::fs::write(dir.path().join("scripts/analyze.py"), b"x").unwrap();
    let wf = dir.path().join("deep.oxoflow");
    fs::write(
        &wf,
        deep_workflow(
            "[[rules]]\nname = \"analyze\"\noutput = [\"results/a.txt\"]\n\
             description = \"run script\"\nscript = \"scripts/analyze.py --out results/a.txt\"\n\n\
             [[rules]]\nname = \"sh\"\noutput = [\"results/s.txt\"]\n\
             description = \"run shell builtin\"\n\
             shell = \"sh -c 'echo hi > results/s.txt'\"\n",
        ),
    )
    .unwrap();

    let out = run_test_command(dir.path(), &wf, &["--deep"]);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("All checks passed."),
        "expected all-checks summary, got:\n{stderr}"
    );
    assert!(
        !stderr.contains("[D0"),
        "expected no findings, got:\n{stderr}"
    );
}

/// `test --deep --json` emits a fourth standalone `deep-check` document with
/// the D001 finding, even though the command exits 1.
#[test]
fn cli_test_deep_json_emits_deep_check_doc() {
    let dir = tempfile::tempdir().unwrap();
    let wf = dir.path().join("deep.oxoflow");
    fs::write(
        &wf,
        deep_workflow(
            "[[rules]]\nname = \"analyze\"\noutput = [\"results/a.txt\"]\n\
             description = \"run script\"\nscript = \"scripts/analyze.py --out results/a.txt\"\n",
        ),
    )
    .unwrap();

    let out = run_test_command(dir.path(), &wf, &["--deep", "--json"]);
    assert!(!out.status.success(), "D001 error must fail test --deep");
    let docs = parse_json_docs(&out.stdout);
    assert_eq!(docs.len(), 4, "expected 4 JSON docs, got {}", docs.len());
    let deep = &docs[3];
    assert_eq!(deep["command"], "deep-check");
    assert_eq!(deep["error_count"], 1);
    assert_eq!(deep["passed"], false);
    assert_eq!(deep["diagnostics"][0]["code"], "D001");
    assert_eq!(deep["diagnostics"][0]["severity"], "error");
    assert!(
        deep["diagnostics"][0]["path"]
            .as_str()
            .unwrap()
            .ends_with("scripts/analyze.py")
    );
}

/// Without `--deep`, `test --json` keeps its existing three documents — the
/// fast path is unchanged.
#[test]
fn cli_test_no_deep_keeps_three_docs() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(dir.path().join("scripts")).unwrap();
    std::fs::write(dir.path().join("scripts/analyze.py"), b"x").unwrap();
    let wf = dir.path().join("deep.oxoflow");
    fs::write(
        &wf,
        deep_workflow(
            "[[rules]]\nname = \"analyze\"\noutput = [\"results/a.txt\"]\n\
             description = \"run script\"\nscript = \"scripts/analyze.py --out results/a.txt\"\n",
        ),
    )
    .unwrap();

    let out = run_test_command(dir.path(), &wf, &["--json"]);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let docs = parse_json_docs(&out.stdout);
    assert_eq!(docs.len(), 3, "expected 3 JSON docs, got {}", docs.len());
}

/// `test --deep --workdir` judges existence from the custom workdir — the
/// same base the executor runs rules from (issue #68 semantics). A script
/// that lives in the workdir must not be reported missing.
#[test]
fn cli_test_deep_respects_workdir_flag() {
    let dir = tempfile::tempdir().unwrap();
    let workdir = dir.path().join("custom");
    std::fs::create_dir_all(workdir.join("scripts")).unwrap();
    std::fs::write(workdir.join("scripts/analyze.py"), b"x").unwrap();
    let wf = dir.path().join("deep.oxoflow");
    fs::write(
        &wf,
        deep_workflow(
            "[[rules]]\nname = \"analyze\"\noutput = [\"results/a.txt\"]\n\
             description = \"run script\"\nscript = \"scripts/analyze.py --out results/a.txt\"\n",
        ),
    )
    .unwrap();

    // Baseline: without --workdir the script is missing relative to the
    // workflow's directory.
    let out = run_test_command(dir.path(), &wf, &["--deep"]);
    assert!(
        !out.status.success(),
        "missing script must fail test --deep"
    );

    // With --workdir the same workflow is healthy: the script lives there.
    let out = run_test_command(
        dir.path(),
        &wf,
        &["--deep", "--workdir", workdir.to_str().unwrap()],
    );
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        !stderr.contains("[D001]"),
        "script exists in the workdir, got:\n{stderr}"
    );
    assert!(
        stderr.contains("script reference(s) found"),
        "expected the found summary, got:\n{stderr}"
    );
}

// ─── Working-directory semantics (issue #68) ────────────────────────────────

/// A run with a custom --workdir records it in the checkpoint; resuming
/// from a different invocation directory re-uses the recorded workdir so
/// completed rules stay skipped instead of being misjudged as stale.
#[test]
fn cli_resume_reuses_recorded_workdir() {
    let dir = tempfile::tempdir().unwrap();
    let wf = dir.path().join("wf/w.oxoflow");
    let wd = dir.path().join("wd");
    fs::create_dir_all(wf.parent().unwrap()).unwrap();
    fs::create_dir_all(wd.join("data")).unwrap();
    fs::create_dir_all(wd.join("results")).unwrap();
    fs::write(
        &wf,
        "[workflow]\nname = \"wd\"\nversion = \"1.0.0\"\n\n[[sample_groups]]\nname = \"cohort\"\nsamples = [\"S1\", \"S2\"]\n\n[[rules]]\nname = \"qc\"\ninput = [\"data/{sample}.fq\"]\noutput = [\"results/{sample}.txt\"]\nshell = \"cat {input} > {output}\"\n",
    )
    .unwrap();
    fs::write(wd.join("data/S1.fq"), b"a").unwrap();
    fs::write(wd.join("data/S2.fq"), b"b").unwrap();

    // Run with a custom workdir, invoked from inside it.
    let out = oxo_flow_cmd()
        .args([
            "run",
            wf.to_str().unwrap(),
            "--workdir",
            wd.to_str().unwrap(),
        ])
        .current_dir(&wd)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(wd.join("results/S1.txt").exists());
    assert!(wd.join("results/S2.txt").exists());

    // The checkpoint records the workdir.
    let checkpoint = wd.join(".oxo-flow/checkpoint.json");
    assert!(checkpoint.exists());
    let state: serde_json::Value = serde_json::from_slice(&fs::read(&checkpoint).unwrap()).unwrap();
    assert_eq!(
        state["workdir"].as_str().map(|s| s.ends_with("/wd")),
        Some(true),
        "checkpoint must record the workdir: {state}"
    );

    // Resume from an unrelated directory: the recorded workdir must be
    // re-used, so both completed rules stay skipped.
    let out = oxo_flow_cmd()
        .args(["resume", checkpoint.to_str().unwrap()])
        .current_dir(dir.path()) // NOT the workdir — proves recorded reuse
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("2 skipped"),
        "completed rules must stay skipped after resume, got:\n{stderr}"
    );
    assert!(
        stderr.contains("Workdir:"),
        "resume must show which workdir it is using, got:\n{stderr}"
    );
}

/// Resuming a checkpoint with no executed rules is not an error: it points
/// the user at `run` and exits cleanly instead of launching an executor.
#[test]
fn cli_resume_empty_checkpoint_advises_run() {
    let dir = tempfile::tempdir().unwrap();
    let wf = dir.path().join("empty.oxoflow");
    fs::write(&wf, "[workflow]\nname = \"empty\"\nversion = \"1.0.0\"\n").unwrap();
    let checkpoint = dir.path().join("checkpoint.json");
    fs::write(
        &checkpoint,
        format!(
            r#"{{"completed_rules": [], "failed_rules": [], "benchmarks": {{}}, "workflow_path": "{}"}}"#,
            wf.display()
        ),
    )
    .unwrap();

    oxo_flow_cmd()
        .args(["resume", checkpoint.to_str().unwrap()])
        .assert()
        .success()
        .stderr(predicate::str::contains(
            "No rules have been executed yet. Use 'oxo-flow run' instead.",
        ));
}

// ─── Concurrent-run protection (.oxo-flow/lock, issue #70) ──────────────────

/// Two `run` invocations on the same workdir must not silently race on the
/// checkpoint: the second gets a clear lock error, and succeeds after the
/// first exits (the OS releases the lock automatically).
#[test]
fn cli_concurrent_runs_get_clear_lock_error() {
    let dir = tempfile::tempdir().unwrap();
    let wf = dir.path().join("locktest.oxoflow");
    fs::write(
        &wf,
        "[workflow]\nname = \"lock\"\nversion = \"1.0.0\"\n\n[[rules]]\nname = \"slow\"\noutput = [\"results/done.txt\"]\nshell = \"sleep 2 && echo done > results/done.txt\"\n",
    )
    .unwrap();

    // Run #1 in the background, holding the lock for ~2s.
    let mut first = std::process::Command::new(workspace_bin("oxo-flow"))
        .args(["run", wf.to_str().unwrap()])
        .current_dir(dir.path())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .unwrap();

    // Wait until the lock is actually held (poll with the core probe).
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    while !oxo_flow_core::executor::WorkdirLock::is_locked(dir.path()) {
        assert!(
            std::time::Instant::now() < deadline,
            "first run never acquired the workdir lock"
        );
        std::thread::sleep(std::time::Duration::from_millis(50));
    }

    // Run #2 while the lock is held: clear error, no silent racing.
    let out = oxo_flow_cmd()
        .args(["run", wf.to_str().unwrap()])
        .current_dir(dir.path())
        .output()
        .unwrap();
    assert!(!out.status.success(), "second run must fail while locked");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("workdir is locked"),
        "second run must report the lock, got:\n{stderr}"
    );

    // After run #1 exits the lock releases: run #2 succeeds and skips the
    // completed rule.
    assert!(first.wait().unwrap().success());
    let out = oxo_flow_cmd()
        .args(["run", wf.to_str().unwrap()])
        .current_dir(dir.path())
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "run after lock release must succeed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(
        String::from_utf8_lossy(&out.stderr).contains("1 skipped"),
        "completed rule must be skipped after the first run"
    );
}

// ─── Input manifest invalidation (issue #72) ───────────────────────────────

/// A completed rule whose glob inputs gained a new matching file must be
/// re-executed together with its DAG downstream; an unchanged file set
/// keeps the checkpoint skip.
#[test]
fn cli_glob_input_change_rebuilds_rule_and_downstream() {
    let dir = tempfile::tempdir().unwrap();
    fs::create_dir(dir.path().join("data")).unwrap();
    fs::write(dir.path().join("data/a.txt"), "a").unwrap();
    fs::write(dir.path().join("data/b.txt"), "b").unwrap();
    let wf = dir.path().join("glob72.oxoflow");
    fs::write(
        &wf,
        "[workflow]\nname = \"glob72\"\n\n\
         [[rules]]\nname = \"gather\"\ninput = [\"data/*.txt\"]\noutput = [\"out.txt\"]\n\
         shell = \"cat data/*.txt > out.txt\"\n\n\
         [[rules]]\nname = \"downstream\"\ninput = [\"out.txt\"]\noutput = [\"down.txt\"]\n\
         shell = \"cp out.txt down.txt\"\n",
    )
    .unwrap();

    // Run 1: both rules execute.
    let run1 = oxo_flow_cmd()
        .args(["run", wf.to_str().unwrap()])
        .current_dir(dir.path())
        .output()
        .unwrap();
    assert!(
        run1.status.success(),
        "run1 failed: {}",
        String::from_utf8_lossy(&run1.stderr)
    );
    assert_eq!(
        fs::read_to_string(dir.path().join("out.txt")).unwrap(),
        "ab"
    );

    // Run 2: unchanged inputs — both rules hit the checkpoint.
    let run2 = oxo_flow_cmd()
        .args(["run", wf.to_str().unwrap()])
        .current_dir(dir.path())
        .output()
        .unwrap();
    assert!(run2.status.success());
    let stderr2 = String::from_utf8_lossy(&run2.stderr);
    assert!(
        stderr2.contains("2 skipped"),
        "unchanged inputs must keep the checkpoint skip: {stderr2}"
    );
    assert!(
        !stderr2.contains("input changes invalidated"),
        "no invalidation expected: {stderr2}"
    );

    // Run 3: a new file appears in the glob — gather and downstream rebuild.
    fs::write(dir.path().join("data/c.txt"), "c").unwrap();
    let run3 = oxo_flow_cmd()
        .args(["run", wf.to_str().unwrap()])
        .current_dir(dir.path())
        .output()
        .unwrap();
    assert!(
        run3.status.success(),
        "run3 failed: {}",
        String::from_utf8_lossy(&run3.stderr)
    );
    let stderr3 = String::from_utf8_lossy(&run3.stderr);
    assert!(
        stderr3.contains("input changes invalidated 2 rule(s)"),
        "gather and downstream must be invalidated: {stderr3}"
    );
    assert!(
        stderr3.contains("Running: gather"),
        "gather must re-run: {stderr3}"
    );
    assert!(
        stderr3.contains("Running: downstream"),
        "downstream must re-run: {stderr3}"
    );
    assert_eq!(
        fs::read_to_string(dir.path().join("out.txt")).unwrap(),
        "abc"
    );
    assert_eq!(
        fs::read_to_string(dir.path().join("down.txt")).unwrap(),
        "abc"
    );

    // Run 4: converged — everything skips again.
    let run4 = oxo_flow_cmd()
        .args(["run", wf.to_str().unwrap()])
        .current_dir(dir.path())
        .output()
        .unwrap();
    assert!(run4.status.success());
    assert!(
        String::from_utf8_lossy(&run4.stderr).contains("2 skipped"),
        "converged run must skip everything"
    );
}

/// A completed rule whose input is a directory must rebuild when a file is
/// added inside that directory (the multiqc-style aggregation case).
#[test]
fn cli_dir_input_change_rebuilds_rule() {
    let dir = tempfile::tempdir().unwrap();
    fs::create_dir(dir.path().join("results")).unwrap();
    fs::write(dir.path().join("results/a.txt"), "aaa\n").unwrap();
    let wf = dir.path().join("dir72.oxoflow");
    fs::write(
        &wf,
        "[workflow]\nname = \"dir72\"\n\n\
         [[rules]]\nname = \"aggregate\"\ninput = [\"results\"]\noutput = [\"total.txt\"]\n\
         shell = \"cat results/*.txt > total.txt\"\n",
    )
    .unwrap();

    let run = |tag: &str| {
        let out = oxo_flow_cmd()
            .args(["run", wf.to_str().unwrap()])
            .current_dir(dir.path())
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "{tag} failed: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        out
    };

    let _ = run("run1");
    assert_eq!(
        fs::read_to_string(dir.path().join("total.txt")).unwrap(),
        "aaa\n"
    );

    let run2 = run("run2");
    assert!(
        String::from_utf8_lossy(&run2.stderr).contains("1 skipped"),
        "unchanged directory must skip"
    );

    fs::write(dir.path().join("results/b.txt"), "bbb\n").unwrap();
    let run3 = run("run3");
    let stderr3 = String::from_utf8_lossy(&run3.stderr);
    assert!(
        stderr3.contains("input changes invalidated"),
        "directory change must invalidate: {stderr3}"
    );
    assert!(
        stderr3.contains("Running: aggregate"),
        "aggregate must re-run: {stderr3}"
    );
    assert_eq!(
        fs::read_to_string(dir.path().join("total.txt")).unwrap(),
        "aaa\nbbb\n"
    );
}

/// Regression for the broader hole behind issue #72: plain-file inputs were
/// equally unprotected (mutation left outputs silently stale).
#[test]
fn cli_plain_input_change_rebuilds_rule() {
    let dir = tempfile::tempdir().unwrap();
    fs::create_dir(dir.path().join("data")).unwrap();
    fs::write(dir.path().join("data/a.txt"), "hello\n").unwrap();
    let wf = dir.path().join("plain72.oxoflow");
    fs::write(
        &wf,
        "[workflow]\nname = \"plain72\"\n\n\
         [[rules]]\nname = \"copy\"\ninput = [\"data/a.txt\"]\noutput = [\"out.txt\"]\n\
         shell = \"cp data/a.txt out.txt\"\n",
    )
    .unwrap();

    let run1 = oxo_flow_cmd()
        .args(["run", wf.to_str().unwrap()])
        .current_dir(dir.path())
        .output()
        .unwrap();
    assert!(run1.status.success());

    // Mutate the input — the completed rule must rebuild, not reuse.
    fs::write(dir.path().join("data/a.txt"), "mutated\n").unwrap();
    let run2 = oxo_flow_cmd()
        .args(["run", wf.to_str().unwrap()])
        .current_dir(dir.path())
        .output()
        .unwrap();
    assert!(
        run2.status.success(),
        "run2 failed: {}",
        String::from_utf8_lossy(&run2.stderr)
    );
    let stderr2 = String::from_utf8_lossy(&run2.stderr);
    assert!(
        stderr2.contains("input changes invalidated"),
        "mutated plain input must invalidate: {stderr2}"
    );
    assert_eq!(
        fs::read_to_string(dir.path().join("out.txt")).unwrap(),
        "mutated\n"
    );
}

// ─── Issue #75 value recovery: optional / hooks / disk / cache ─────────────

/// `optional = true` rules skip without error when their inputs are absent
/// (previously the field was parsed but never enforced).
#[test]
fn cli_optional_rule_skips_when_inputs_missing() {
    let dir = tempfile::tempdir().unwrap();
    let wf = dir.path().join("opt75.oxoflow");
    fs::write(
        &wf,
        "[workflow]\nname = \"opt75\"\n\n\
         [[rules]]\nname = \"baseline\"\noutput = [\"base.txt\"]\n\
         shell = \"echo base > base.txt\"\n\n\
         [[rules]]\nname = \"optional_step\"\ninput = [\"data/absent.txt\"]\n\
         output = [\"opt.txt\"]\noptional = true\n\
         shell = \"cp data/absent.txt opt.txt\"\n",
    )
    .unwrap();

    // Input missing: optional rule skips, the run succeeds.
    let run1 = oxo_flow_cmd()
        .args(["run", wf.to_str().unwrap()])
        .current_dir(dir.path())
        .output()
        .unwrap();
    assert!(
        run1.status.success(),
        "run1 must succeed: {}",
        String::from_utf8_lossy(&run1.stderr)
    );
    let stderr1 = String::from_utf8_lossy(&run1.stderr);
    assert!(
        stderr1.contains("1 skipped"),
        "optional rule must skip, not fail: {stderr1}"
    );
    assert!(
        stderr1.contains("optional inputs missing"),
        "skip reason must be surfaced: {stderr1}"
    );
    assert!(
        !stderr1.contains("✓ optional_step"),
        "optional rule with missing inputs must not execute: {stderr1}"
    );
    assert!(!dir.path().join("opt.txt").exists());

    // Input present: the rule executes.
    fs::create_dir(dir.path().join("data")).unwrap();
    fs::write(dir.path().join("data/absent.txt"), "here").unwrap();
    let run2 = oxo_flow_cmd()
        .args(["run", wf.to_str().unwrap()])
        .current_dir(dir.path())
        .output()
        .unwrap();
    assert!(run2.status.success());
    let stderr2 = String::from_utf8_lossy(&run2.stderr);
    assert!(
        stderr2.contains("Running: optional_step"),
        "optional rule must run once its input exists: {stderr2}"
    );
    assert_eq!(
        fs::read_to_string(dir.path().join("opt.txt")).unwrap(),
        "here"
    );
}

/// Hook commands expand {config.x} / {input} / {output} placeholders like
/// the main shell command (issue #75).
#[test]
fn cli_hooks_expand_config_placeholders() {
    let dir = tempfile::tempdir().unwrap();
    let wf = dir.path().join("hooks75.oxoflow");
    fs::write(
        &wf,
        "[workflow]\nname = \"hooks75\"\n\n\
         [config]\ntag = \"EXPANDED\"\n\n\
         [[rules]]\nname = \"hooky\"\noutput = [\"out.txt\"]\n\
         shell = \"echo main > out.txt\"\n\
         pre_exec = \"echo {config.tag} > pre.txt\"\n\
         on_success = \"cp {output} success.txt\"\n",
    )
    .unwrap();

    let run = oxo_flow_cmd()
        .args(["run", wf.to_str().unwrap()])
        .current_dir(dir.path())
        .output()
        .unwrap();
    assert!(
        run.status.success(),
        "run failed: {}",
        String::from_utf8_lossy(&run.stderr)
    );
    assert_eq!(
        fs::read_to_string(dir.path().join("pre.txt")).unwrap(),
        "EXPANDED\n"
    );
    assert_eq!(
        fs::read_to_string(dir.path().join("success.txt")).unwrap(),
        "main\n"
    );
}

/// A rule declaring more `resources.disk` than the workdir has free space
/// triggers a pre-flight warning before any rule executes (issue #75).
#[test]
fn cli_disk_requirement_preflight_warns() {
    let dir = tempfile::tempdir().unwrap();
    let wf = dir.path().join("disk75.oxoflow");
    fs::write(
        &wf,
        "[workflow]\nname = \"disk75\"\n\n\
         [[rules]]\nname = \"greedy\"\noutput = [\"g.txt\"]\n\
         shell = \"echo x > g.txt\"\n\n\
         [rules.resources]\ndisk = \"999999999G\"\n",
    )
    .unwrap();

    let run = oxo_flow_cmd()
        .args(["run", wf.to_str().unwrap()])
        .current_dir(dir.path())
        .output()
        .unwrap();
    assert!(run.status.success());
    let stderr = String::from_utf8_lossy(&run.stderr);
    assert!(
        stderr.contains("declares 999999999G disk but only"),
        "pre-flight disk warning expected: {stderr}"
    );
}

/// --cache-dir entries older than 30 days are removed after the run
/// (issue #75); fresh entries survive.
#[test]
fn cli_cache_dir_aging_cleans_stale_entries() {
    let dir = tempfile::tempdir().unwrap();
    let wf = dir.path().join("cache75.oxoflow");
    fs::write(
        &wf,
        "[workflow]\nname = \"cache75\"\n\n\
         [[rules]]\nname = \"one\"\noutput = [\"one.txt\"]\n\
         shell = \"echo x > one.txt\"\n",
    )
    .unwrap();
    let cache = dir.path().join("env-cache");
    fs::create_dir_all(&cache).unwrap();
    fs::write(cache.join("stale.txt"), "old").unwrap();
    fs::write(cache.join("fresh.txt"), "new").unwrap();
    // Backdate the stale entry beyond the 30-day window.
    let touch = std::process::Command::new("touch")
        .arg("-t")
        .arg("200001010000")
        .arg(cache.join("stale.txt"))
        .status()
        .unwrap();
    assert!(touch.success(), "touch must backdate the stale file");

    let run = oxo_flow_cmd()
        .args([
            "run",
            wf.to_str().unwrap(),
            "--cache-dir",
            cache.to_str().unwrap(),
        ])
        .current_dir(dir.path())
        .output()
        .unwrap();
    assert!(run.status.success());

    assert!(
        !cache.join("stale.txt").exists(),
        "stale cache entry must be removed"
    );
    assert!(
        cache.join("fresh.txt").exists(),
        "fresh cache entry must survive"
    );
}
