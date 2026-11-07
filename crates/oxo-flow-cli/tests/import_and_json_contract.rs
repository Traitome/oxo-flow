//! Import wiring honesty and `--json` error-contract parity.
//!
//! Dev-journey audit fixtures: the Nextflow pipe form and multi-process
//! wiring must map to real patterns (never invented paths/types), and every
//! `--json` command must emit one machine-readable document even on failure.

use serde_json::Value;
use std::path::Path;
use std::process::Command;

fn oxo() -> Command {
    Command::new(env!("CARGO_BIN_EXE_oxo-flow"))
}

fn write_source(dir: &Path, name: &str, content: &str) -> String {
    let path = dir.join(name);
    std::fs::write(&path, content).unwrap();
    path.to_string_lossy().into_owned()
}

fn write_workflow(dir: &Path, name: &str, content: &str) -> String {
    write_source(dir, name, content)
}

/// Run a command, returning (success, stdout, stderr).
fn run(args: &[&str], dir: &Path) -> (bool, String, String) {
    let out = oxo().args(args).current_dir(dir).output().unwrap();
    (
        out.status.success(),
        String::from_utf8_lossy(&out.stdout).to_string(),
        String::from_utf8_lossy(&out.stderr).to_string(),
    )
}

fn parse_json(stdout: &str) -> Value {
    serde_json::from_str(stdout)
        .unwrap_or_else(|e| panic!("stdout must be one JSON document ({e}): {stdout}"))
}

const PIPE_FORM: &str = r#"
process COUNT {
    input: path x
    output: "${x}.count"
    script: "wc -l < ${x} > ${x}.count"
}
workflow { Channel.fromPath("data/*.txt") | COUNT }
"#;

const TWO_PROCESS: &str = r#"
process TRIM {
    input: path reads
    output: "${reads}.trimmed"
    script: "trim ${reads} > ${reads}.trimmed"
}
process ALIGN {
    input: path reads
    output: "${reads}.bam"
    script: "cp ${reads} aligned.bam"
}
workflow {
    TRIM(Channel.fromPath("data/*.fastq.gz"))
    ALIGN(TRIM.out)
}
"#;

#[test]
fn import_pipe_form_maps_the_from_path_wildcard() {
    let dir = tempfile::tempdir().unwrap();
    let source = write_source(dir.path(), "count.nf", PIPE_FORM);

    let (ok, stdout, stderr) = run(&["import", "--quiet", &source], dir.path());
    assert!(ok, "pipe-form import must succeed: {stderr}");
    assert!(
        stdout.contains(r#"input = ["data/{sample}.txt"]"#),
        "pipe form must map to the wildcard input, not an invented path: {stdout}"
    );
    assert!(
        stdout.contains(r#"sample_pattern = "data/{sample}.txt""#),
        "the wildcard needs a declared source: {stdout}"
    );
    assert!(
        !stdout.contains("data/x"),
        "the input variable name must never be emitted as a path: {stdout}"
    );
}

#[test]
fn import_multi_process_wiring_resolves_and_keeps_topology() {
    let dir = tempfile::tempdir().unwrap();
    let source = write_source(dir.path(), "two-proc.nf", TWO_PROCESS);
    let output = dir.path().join("two-proc.oxoflow");

    let (ok, _stdout, stderr) = run(
        &["import", "--quiet", &source, "-o", output.to_str().unwrap()],
        dir.path(),
    );
    assert!(ok, "wiring import must succeed: {stderr}");
    let toml = std::fs::read_to_string(&output).unwrap();
    assert!(
        toml.contains(r#"input = ["data/{sample}.fastq.gz.trimmed"]"#),
        "align must consume trim's output pattern: {toml}"
    );
    assert!(
        toml.contains(r#"depends_on = ["trim"]"#),
        "the wiring must emit a depends_on edge: {toml}"
    );
    assert!(
        !toml.contains("[types.fastq]"),
        "no fabricated fastq contract may be attached to the derived intermediate: {toml}"
    );

    let (ok, stdout, stderr) = run(
        &["graph", "--quiet", output.to_str().unwrap(), "-f", "dot"],
        dir.path(),
    );
    assert!(ok, "graph must render the imported workflow: {stderr}");
    assert!(
        stdout.contains(r#"0 [ label = "trim"]"#)
            && stdout.contains(r#"1 [ label = "align"]"#)
            && stdout.contains("0 -> 1"),
        "the imported DAG must keep TRIM → ALIGN: {stdout}"
    );
}

#[test]
fn import_aborts_on_unresolvable_wiring_without_writing_output() {
    let dir = tempfile::tempdir().unwrap();
    let source = write_source(
        dir.path(),
        "bad.nf",
        r#"
process ALIGN {
    input: path reads
    output: "${reads}.sorted"
    script: "cp ${reads} sorted.out"
}
workflow { ALIGN(params.reads) }
"#,
    );
    let output = dir.path().join("bad.oxoflow");

    let (ok, _stdout, stderr) = run(
        &["import", "--quiet", &source, "-o", output.to_str().unwrap()],
        dir.path(),
    );
    assert!(
        !ok,
        "an unresolvable wiring expression must abort the import"
    );
    assert!(
        stderr.contains("params.reads"),
        "the error must name the unsupported construct: {stderr}"
    );
    assert!(
        !output.exists(),
        "a failed import must not write a workflow with invented paths"
    );
}

#[test]
fn validate_and_dry_run_report_bound_but_missing_wildcard_inputs() {
    let dir = tempfile::tempdir().unwrap();
    let workflow = write_workflow(
        dir.path(),
        "wild.oxoflow",
        r#"
[workflow]
name = "wild"

[[sample_groups]]
name = "cohort"
samples = ["A", "B"]

[[rules]]
name = "consume"
input = ["inputs/{sample}.txt"]
output = ["out/{sample}.txt"]
shell = "cp {input} {output}"
"#,
    );

    let (ok, stdout, stderr) = run(&["validate", "--json", &workflow], dir.path());
    assert!(ok, "missing inputs stay warnings for validate: {stderr}");
    let document = parse_json(&stdout);
    let missing = document["missing_inputs"].as_array().unwrap();
    assert_eq!(
        missing,
        &vec![
            Value::String("inputs/A.txt".into()),
            Value::String("inputs/B.txt".into())
        ],
        "validate must bind the declared sample domain and report the concrete paths: {document}"
    );

    let (ok, stdout, stderr) = run(&["dry-run", "--json", &workflow], dir.path());
    assert!(ok, "dry-run is a preview and stays exit 0: {stderr}");
    let document = parse_json(&stdout);
    assert_eq!(
        document["missing_inputs"],
        serde_json::json!(["inputs/A.txt", "inputs/B.txt"]),
        "dry-run --json must expose the same unproducible inputs: {document}"
    );
}

#[test]
fn validate_reports_wildcard_inputs_when_declared_domain_binds_no_values() {
    let dir = tempfile::tempdir().unwrap();
    let workflow = write_workflow(
        dir.path(),
        "no-samples.oxoflow",
        r#"
[workflow]
name = "no-samples"
sample_pattern = "data/{sample}.txt"

[[rules]]
name = "consume"
input = ["inputs/{sample}.txt"]
output = ["out/{sample}.txt"]
shell = "cp {input} {output}"
"#,
    );

    let (ok, stdout, _stderr) = run(&["validate", "--json", &workflow], dir.path());
    assert!(ok, "missing inputs stay warnings for validate");
    let document = parse_json(&stdout);
    let missing = document["missing_inputs"].as_array().unwrap();
    assert!(
        missing.iter().any(|entry| entry
            .as_str()
            .is_some_and(|entry| entry.starts_with("inputs/{sample}.txt")
                && entry.contains("declared sample domain binds no value"))),
        "a domain that binds nothing must be reported, not silently green: {document}"
    );
}

#[test]
fn dry_run_json_emits_a_structured_error_on_parse_failure() {
    let dir = tempfile::tempdir().unwrap();
    let workflow = write_workflow(dir.path(), "broken.oxoflow", "[workflow\nname = \"x\"\n");

    let (ok, stdout, _stderr) = run(&["dry-run", "--json", &workflow], dir.path());
    assert!(!ok, "an unparseable workflow must exit non-zero");
    assert!(
        !stdout.trim().is_empty(),
        "dry-run --json must never exit with EMPTY stdout"
    );
    let document = parse_json(&stdout);
    assert_eq!(document["command"], "dry-run");
    assert_eq!(document["errors"][0]["code"], "PARSE");
    assert!(
        document["errors"][0]["message"]
            .as_str()
            .is_some_and(|message| !message.is_empty()),
        "the error document must carry the parser's message: {document}"
    );
}

#[test]
fn status_json_emits_a_structured_error_on_a_missing_checkpoint() {
    let dir = tempfile::tempdir().unwrap();
    let missing = dir.path().join("absent-checkpoint.json");

    let (ok, stdout, _stderr) = run(&["status", "--json", missing.to_str().unwrap()], dir.path());
    assert!(!ok, "a missing checkpoint must exit non-zero");
    assert!(
        !stdout.trim().is_empty(),
        "status --json must never exit with EMPTY stdout"
    );
    let document = parse_json(&stdout);
    assert_eq!(document["command"], "status");
    assert_eq!(document["errors"][0]["code"], "CHECKPOINT");
    assert_eq!(
        document["checkpoint"],
        Value::String(missing.display().to_string())
    );
}

#[test]
fn imported_threads_use_the_canonical_resources_form() {
    let dir = tempfile::tempdir().unwrap();
    let source = write_source(
        dir.path(),
        "threaded.nf",
        r#"
process TRIM {
    cpus 4
    input: path reads
    output: "${reads}.trimmed"
    script: "trim ${reads} > ${reads}.trimmed"
}
workflow { TRIM(Channel.fromPath("data/*.fastq.gz")) }
"#,
    );
    let output = dir.path().join("threaded.oxoflow");
    let (ok, _stdout, stderr) = run(
        &["import", "--quiet", &source, "-o", output.to_str().unwrap()],
        dir.path(),
    );
    assert!(ok, "import must succeed: {stderr}");
    let toml = std::fs::read_to_string(&output).unwrap();
    assert!(
        toml.contains("resources = { threads = 4 }"),
        "threads must be emitted under resources: {toml}"
    );

    let (ok, stdout, _stderr) = run(&["lint", "--json", output.to_str().unwrap()], dir.path());
    assert!(ok, "the imported workflow must pass lint");
    let document = parse_json(&stdout);
    assert!(
        !stdout.contains("W025"),
        "the importer must not trip its own deprecation lint: {document}"
    );
}

#[test]
fn graph_file_output_matches_stdout_bytes() {
    let dir = tempfile::tempdir().unwrap();
    let workflow = write_workflow(
        dir.path(),
        "graph.oxoflow",
        r#"
[workflow]
name = "g"

[[rules]]
name = "ingest"
output = ["data/a.txt"]
shell = "echo a > data/a.txt"

[[rules]]
name = "convert"
input = ["data/a.txt"]
output = ["out/b.txt"]
shell = "cp data/a.txt out/b.txt"
"#,
    );
    let out_path = dir.path().join("graph.dot");

    let (ok, stdout, stderr) = run(&["graph", "--quiet", &workflow, "-f", "dot"], dir.path());
    assert!(ok, "stdout render must succeed: {stderr}");
    let (ok, _stdout, stderr) = run(
        &[
            "graph",
            "--quiet",
            &workflow,
            "-f",
            "dot",
            "-o",
            out_path.to_str().unwrap(),
        ],
        dir.path(),
    );
    assert!(ok, "file render must succeed: {stderr}");
    let written = std::fs::read_to_string(&out_path).unwrap();
    assert_eq!(
        written, stdout,
        "-o must write the same bytes stdout carries (including the trailing newline)"
    );
}

#[test]
fn metro_module_without_modules_falls_back_with_a_note() {
    let dir = tempfile::tempdir().unwrap();
    let workflow = write_workflow(
        dir.path(),
        "metro.oxoflow",
        r#"
[workflow]
name = "m"

[[rules]]
name = "ingest"
output = ["data/a.txt"]
shell = "echo a > data/a.txt"

[[rules]]
name = "convert"
input = ["data/a.txt"]
output = ["out/b.txt"]
shell = "cp data/a.txt out/b.txt"
"#,
    );

    let (ok, stdout, stderr) = run(
        &[
            "graph",
            "--quiet",
            &workflow,
            "-f",
            "metro",
            "--granularity",
            "module",
        ],
        dir.path(),
    );
    assert!(ok, "the fallback must still render: {stderr}");
    assert!(
        stderr.contains("falling back to process granularity"),
        "the fallback must be announced, not silent: {stderr}"
    );
    assert!(
        stdout.contains(r#"n0["ingest"]"#)
            && stdout.contains(r#"n1["convert"]"#)
            && stdout.contains("n0 -->"),
        "the fallback must preserve the DAG topology: {stdout}"
    );
}
