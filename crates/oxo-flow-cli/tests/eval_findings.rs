//! Regression tests for the nine-dimension harness evaluation findings
//! (issue #324), fixed on v0.17.2:
//!
//! - F-1: a rule whose outputs exist but was never recorded as completed
//!   (e.g. the engine was SIGKILLed after its shell wrote outputs, so
//!   resume verdicts it "outputs up-to-date") must still be recorded into
//!   `completed_rules`/`benchmarks` — otherwise every later run re-submits
//!   it and the audit trail stays wrong forever.
//! - F-2: `transform.cleanup = true` must delete chunk files on a default
//!   run; the deletion was gated on the checksum migration, which only
//!   records under `--provenance`.
//!
//! The F-1 scenario is simulated by editing the checkpoint instead of a
//! real SIGKILL: both leave the rule's outputs on disk while its completion
//! records are missing, which is exactly the state the engine must recover.

use std::path::Path;
use std::process::Command;

fn oxo() -> Command {
    Command::new(env!("CARGO_BIN_EXE_oxo-flow"))
}

/// Write a 3-rule chain `step1 → step2 → step3` into `dir`.
fn write_chain(dir: &Path) {
    std::fs::write(
        dir.join("w.oxoflow"),
        r#"
[workflow]
name = "eval-f1-chain"
version = "1.0.0"

[[rules]]
name = "step1"
output = ["r/out1.txt"]
shell = "mkdir -p r && echo step1 > {output[0]}"

[[rules]]
name = "step2"
input = ["r/out1.txt"]
output = ["r/out2.txt"]
shell = "cat {input[0]} > {output[0]} && echo step2 >> {output[0]}"

[[rules]]
name = "step3"
input = ["r/out2.txt"]
output = ["r/out3.txt"]
shell = "cat {input[0]} > {output[0]} && echo step3 >> {output[0]}"
"#,
    )
    .unwrap();
}

fn checkpoint_json(dir: &Path) -> serde_json::Value {
    let raw = std::fs::read_to_string(dir.join(".oxo-flow/checkpoint.json"))
        .expect("checkpoint exists after a run");
    serde_json::from_str(&raw).unwrap()
}

fn run(dir: &Path, workflow: &str) -> (String, String) {
    let out = oxo()
        .current_dir(dir)
        .args(["run", workflow])
        .output()
        .expect("oxo-flow run spawns");
    (
        String::from_utf8_lossy(&out.stdout).to_string(),
        String::from_utf8_lossy(&out.stderr).to_string(),
    )
}

fn resume(dir: &Path) -> (String, String) {
    let out = oxo()
        .current_dir(dir)
        .args(["resume", ".oxo-flow/checkpoint.json"])
        .output()
        .expect("oxo-flow resume spawns");
    (
        String::from_utf8_lossy(&out.stdout).to_string(),
        String::from_utf8_lossy(&out.stderr).to_string(),
    )
}

#[test]
fn f1_up_to_date_rule_is_recorded_as_completed_after_resume() {
    // Arrange — a full successful chain, then simulate the SIGKILL window:
    // step2's outputs stay on disk while its completion records vanish.
    let dir = tempfile::tempdir().unwrap();
    write_chain(dir.path());
    let (_, err) = run(dir.path(), "w.oxoflow");
    assert!(err.contains("3 succeeded"), "initial run: {err}");

    let ck_path = dir.path().join(".oxo-flow/checkpoint.json");
    let mut ck = checkpoint_json(dir.path());
    ck["completed_rules"]
        .as_array_mut()
        .unwrap()
        .retain(|r| r.as_str() != Some("step2"));
    ck["benchmarks"].as_object_mut().unwrap().remove("step2");
    std::fs::write(&ck_path, serde_json::to_string_pretty(&ck).unwrap()).unwrap();

    // Act — resume from the interrupted state.
    let (out, err) = resume(dir.path());
    let combined = format!("{out}\n{err}");

    // Assert — step2 is verdicted up-to-date (its output exists) AND is
    // recorded as completed again, so the audit trail matches the disk.
    assert!(
        combined.contains("step2 (outputs up-to-date)"),
        "resume should skip step2 as up-to-date: {combined}"
    );
    let ck = checkpoint_json(dir.path());
    let completed: Vec<&str> = ck["completed_rules"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| r.as_str().unwrap())
        .collect();
    assert!(
        completed.contains(&"step2"),
        "up-to-date step2 must re-enter completed_rules, got {completed:?}"
    );
    assert!(
        ck["benchmarks"].get("step2").is_some(),
        "up-to-date step2 must get a benchmark entry"
    );
}

#[test]
fn f1_rerun_after_resume_does_not_resubmit_up_to_date_rule() {
    // Arrange — same interrupted state as above.
    let dir = tempfile::tempdir().unwrap();
    write_chain(dir.path());
    let (_, err) = run(dir.path(), "w.oxoflow");
    assert!(err.contains("3 succeeded"), "initial run: {err}");

    let ck_path = dir.path().join(".oxo-flow/checkpoint.json");
    let mut ck = checkpoint_json(dir.path());
    ck["completed_rules"]
        .as_array_mut()
        .unwrap()
        .retain(|r| r.as_str() != Some("step2"));
    ck["benchmarks"].as_object_mut().unwrap().remove("step2");
    std::fs::write(&ck_path, serde_json::to_string_pretty(&ck).unwrap()).unwrap();
    resume(dir.path());

    // Act — a plain run after the resume.
    let (out, err) = run(dir.path(), "w.oxoflow");
    let combined = format!("{out}\n{err}");

    // Assert — step2 is short-circuited as already completed instead of
    // being submitted (and silently skip-counted) on every run.
    assert!(
        combined.contains("step2 (already completed)"),
        "rerun must treat step2 as completed: {combined}"
    );
    assert!(
        !combined.contains("Running: step2"),
        "rerun must not re-submit step2: {combined}"
    );
    assert!(
        combined.contains("0 succeeded, 3 skipped"),
        "no rule re-executes after the recovery: {combined}"
    );
}

#[test]
fn f3_validate_warns_on_wildcard_inputs_without_sample_domain() {
    // Arrange — a wildcard input with NO sample domain declared anywhere.
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(
        dir.path().join("bad.oxoflow"),
        r#"
[workflow]
name = "eval-f3-wildcard"
version = "1.0.0"

[[rules]]
name = "consume"
input = ["missing/{sample}.txt"]
output = ["out/ok.txt"]
shell = "cp {input[0]} {output[0]}"
"#,
    )
    .unwrap();

    // Act — validate must flag the unresolvable wildcard instead of
    // approving with "valid": true.
    let out = oxo()
        .current_dir(dir.path())
        .args(["validate", "bad.oxoflow", "--json"])
        .output()
        .expect("oxo-flow validate spawns");
    let json: serde_json::Value =
        serde_json::from_str(&String::from_utf8_lossy(&out.stdout)).unwrap();

    // Assert — the wildcard input is reported so the approve layer no
    // longer gives false confidence.
    let missing: Vec<&str> = json["missing_inputs"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_str().unwrap())
        .collect();
    assert!(
        missing.iter().any(|m| m.contains("missing/{sample}.txt")),
        "validate must report the unresolved wildcard input, got {missing:?}"
    );
}

#[test]
fn f3_validate_is_silent_when_sample_domain_is_declared() {
    // Arrange — the same wildcard input, but with [[sample_groups]].
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(
        dir.path().join("ok.oxoflow"),
        r#"
[workflow]
name = "eval-f3-declared"
version = "1.0.0"

[[sample_groups]]
name = "tumor"
samples = ["S1"]

[[rules]]
name = "consume"
input = ["missing/{sample}.txt"]
output = ["out/ok.txt"]
shell = "cp {input[0]} {output[0]}"
"#,
    )
    .unwrap();

    // Act
    let out = oxo()
        .current_dir(dir.path())
        .args(["validate", "ok.oxoflow", "--json"])
        .output()
        .expect("oxo-flow validate spawns");
    let json: serde_json::Value =
        serde_json::from_str(&String::from_utf8_lossy(&out.stdout)).unwrap();

    // Assert — a declared sample domain keeps the input out of the report
    // (runtime discovery resolves it; nothing to warn about statically).
    let missing = json["missing_inputs"].as_array().unwrap();
    assert!(
        missing.is_empty(),
        "declared sample domain must not be reported, got {missing:?}"
    );
}

#[test]
fn f2_default_run_cleans_up_transform_chunks() {
    // Arrange — a scatter-gather transform with cleanup = true.
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(
        dir.path().join("t.oxoflow"),
        r#"
[workflow]
name = "eval-f2-transform"
version = "1.0.0"

[config]
parts = ["a", "b", "c"]

[[rules]]
name = "seed"
output = ["in/all.txt"]
shell = "mkdir -p in && printf 'a,1\nb,2\nc,3\n' > {output[0]}"

[[rules]]
name = "by_part"
input = ["in/all.txt"]
output = ["out/per_part.txt"]

[rules.transform.split]
by = "part"
values_from = "config.parts"

[rules.transform]
map = "grep '^{part},' {input} > {output}"
cleanup = true

[rules.transform.combine]
shell = "cat {chunks} > {output} && wc -l < {output} | tr -d ' ' >> {output}"
"#,
    )
    .unwrap();

    // Act — a default run (no --provenance).
    let (_, err) = run(dir.path(), "t.oxoflow");
    assert!(err.contains("5 succeeded"), "run: {err}");

    // Assert — the chunk files are deleted even though no checksums were
    // ever recorded (the non-provenance path).
    let chunks = dir.path().join(".oxo-flow/chunks");
    let leftover: Vec<_> = std::fs::read_dir(&chunks)
        .map(|rd| {
            rd.filter_map(|e| e.ok())
                .filter(|e| e.file_type().map(|t| t.is_file()).unwrap_or(false))
                .collect()
        })
        .unwrap_or_default();
    assert!(
        leftover.is_empty(),
        "cleanup=true must delete chunk files on a default run, left {leftover:?}"
    );
}
