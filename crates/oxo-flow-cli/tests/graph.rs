//! Integration tests for the `graph` subcommand: the format matrix, the
//! metro granularity ladder, `--expanded`, and the option constraints
//! (`--granularity` is metro-only; unknown formats fail at the clap layer).

use std::path::Path;
use std::process::Command;

fn oxo() -> Command {
    Command::new(env!("CARGO_BIN_EXE_oxo-flow"))
}

/// Write a workflow into `dir` and return its path string.
fn write_workflow(dir: &Path, name: &str, content: &str) -> String {
    let path = dir.join(name);
    std::fs::write(&path, content).unwrap();
    path.to_string_lossy().into_owned()
}

/// Run `oxo-flow graph <args..>` in `dir`; return (status, stdout, stderr).
fn run_graph(dir: &Path, workflow: &str, args: &[&str]) -> (bool, String, String) {
    let out = oxo()
        .args(["graph", "--quiet", workflow])
        .args(args)
        .current_dir(dir)
        .output()
        .unwrap();
    (
        out.status.success(),
        String::from_utf8_lossy(&out.stdout).to_string(),
        String::from_utf8_lossy(&out.stderr).to_string(),
    )
}

/// A two-rule chain `generate_data → transform` (the docs' canonical demo).
fn two_step() -> &'static str {
    r#"
[workflow]
name = "graph-demo"

[[rules]]
name = "generate_data"
output = ["data/greeting.txt"]
shell = "mkdir -p data && echo hello > data/greeting.txt"

[[rules]]
name = "transform"
input = ["data/greeting.txt"]
output = ["results/uppercase.txt"]
shell = "mkdir -p results && tr a-z A-Z < data/greeting.txt > results/uppercase.txt"
"#
}

/// A same-tool chain (`fastp` trim → `fastp` index) for the process tier.
fn fastp_chain() -> &'static str {
    r#"
[workflow]
name = "fastp-demo"

[[rules]]
name = "fastp_trim"
output = ["out/trimmed.txt"]
shell = "mkdir -p out && fastp -i in.fq -o out/trimmed.txt"

[[rules]]
name = "fastp_index"
input = ["out/trimmed.txt"]
output = ["final/idx.txt"]
shell = "mkdir -p final && fastp -i out/trimmed.txt -o final/idx.txt"
"#
}

/// A `module::`-prefixed pair for the module tier.
fn two_modules() -> &'static str {
    r#"
[workflow]
name = "module-demo"

[[rules]]
name = "qc::fastqc_raw"
output = ["qc_raw/"]
shell = "fastqc raw.fq"

[[rules]]
name = "align::bwa_mem"
input = ["qc_raw/"]
output = ["aln.bam"]
shell = "bwa mem ref.fa raw.fq > aln.bam"
"#
}

/// A workflow with a live `sample_pattern` (two real samples on disk).
fn sampled() -> &'static str {
    r#"
[workflow]
name = "expand-demo"
sample_pattern = "raw/{sample}.txt"

[[rules]]
name = "step1"
output = ["out/{sample}.txt"]
shell = "mkdir -p out && cp raw/{sample}.txt out/{sample}.txt"

[[rules]]
name = "step2"
input = ["out/{sample}.txt"]
output = ["final/{sample}.txt"]
shell = "mkdir -p final && cp out/{sample}.txt final/{sample}.txt"
"#
}

/// Station emission lines — `n<digits>[...]` only, never subgraph
/// declarations (whose sanitized section ids may contain an `n`).
fn count_stations(mmd: &str) -> usize {
    mmd.lines()
        .filter(|l| {
            let t = l.trim_start();
            t.starts_with('n')
                && t.chars().nth(1).is_some_and(|c| c.is_ascii_digit())
                && t.contains('[')
        })
        .count()
}

#[test]
fn ascii_is_the_default_format() {
    let dir = tempfile::tempdir().unwrap();
    let wf = write_workflow(dir.path(), "w.oxoflow", two_step());
    let (ok, stdout, _) = run_graph(dir.path(), &wf, &[]);
    assert!(ok);
    assert!(
        stdout.contains("Workflow DAG"),
        "metrics box missing:\n{stdout}"
    );
    assert!(
        stdout.contains("Level 0"),
        "level grouping missing:\n{stdout}"
    );
    assert!(stdout.contains("generate_data"));
    assert!(stdout.contains("transform"));
}

#[test]
fn dot_format_emits_graphviz_digraph() {
    let dir = tempfile::tempdir().unwrap();
    let wf = write_workflow(dir.path(), "w.oxoflow", two_step());
    let (ok, stdout, _) = run_graph(dir.path(), &wf, &["-f", "dot"]);
    assert!(ok);
    assert!(
        stdout.starts_with("digraph"),
        "digraph header missing:\n{stdout}"
    );
    assert!(stdout.contains("generate_data"));
    assert!(stdout.contains(" -> "));
}

#[test]
fn mermaid_format_emits_plain_graph_lr() {
    let dir = tempfile::tempdir().unwrap();
    let wf = write_workflow(dir.path(), "w.oxoflow", two_step());
    let (ok, stdout, _) = run_graph(dir.path(), &wf, &["-f", "mermaid"]);
    assert!(ok);
    assert!(
        stdout.contains("graph LR"),
        "graph LR header missing:\n{stdout}"
    );
    assert!(
        !stdout.contains("%%metro"),
        "mermaid must carry no metro directives:\n{stdout}"
    );
    assert!(stdout.contains("n0 --> n1"));
}

#[test]
fn metro_rule_granularity_keeps_every_rule_a_station() {
    let dir = tempfile::tempdir().unwrap();
    let wf = write_workflow(dir.path(), "w.oxoflow", two_step());
    let (ok, stdout, _) = run_graph(dir.path(), &wf, &["-f", "metro"]);
    assert!(ok);
    assert!(
        stdout.contains("%%metro line:"),
        "line directives missing:\n{stdout}"
    );
    assert!(
        stdout.contains("n0[\"generate_data\"]"),
        "station missing:\n{stdout}"
    );
    assert!(
        stdout.contains("n1[\"transform\"]"),
        "station missing:\n{stdout}"
    );
    assert_eq!(count_stations(&stdout), 2);
}

#[test]
fn metro_process_granularity_collapses_tool_chains() {
    let dir = tempfile::tempdir().unwrap();
    let wf = write_workflow(dir.path(), "w.oxoflow", fastp_chain());
    let (ok, stdout, _) = run_graph(
        dir.path(),
        &wf,
        &["-f", "metro", "--granularity", "process"],
    );
    assert!(ok);
    assert!(
        stdout.contains("n0[\"fastp\"]"),
        "tool chain must collapse into one station:\n{stdout}"
    );
    assert!(
        !stdout.contains("n1["),
        "merged rules keep no stations:\n{stdout}"
    );
    assert_eq!(count_stations(&stdout), 1);
}

#[test]
fn metro_module_granularity_emits_one_station_per_section() {
    let dir = tempfile::tempdir().unwrap();
    let wf = write_workflow(dir.path(), "w.oxoflow", two_modules());
    let (ok, stdout, _) = run_graph(dir.path(), &wf, &["-f", "metro", "--granularity", "module"]);
    assert!(ok);
    assert!(
        !stdout.contains("subgraph"),
        "module tier is a flat graph:\n{stdout}"
    );
    assert_eq!(count_stations(&stdout), 2);
    assert!(stdout.contains("n0["), "qc station missing:\n{stdout}");
    assert!(stdout.contains("n1["), "align station missing:\n{stdout}");
}

#[test]
fn granularity_is_rejected_outside_the_metro_format() {
    let dir = tempfile::tempdir().unwrap();
    let wf = write_workflow(dir.path(), "w.oxoflow", two_step());
    for format in ["ascii", "dot", "mermaid"] {
        let (ok, stdout, stderr) =
            run_graph(dir.path(), &wf, &["-f", format, "--granularity", "module"]);
        assert!(!ok, "granularity + {format} must fail");
        assert!(
            stdout.is_empty(),
            "no graph output on a rejected combo:\n{stdout}"
        );
        assert!(
            stderr.contains("only applies to the metro format"),
            "explaining error expected:\n{stderr}"
        );
    }
}

#[test]
fn unknown_format_is_rejected_by_clap() {
    let dir = tempfile::tempdir().unwrap();
    let wf = write_workflow(dir.path(), "w.oxoflow", two_step());
    let (ok, stdout, stderr) = run_graph(dir.path(), &wf, &["-f", "svg"]);
    assert!(!ok);
    assert!(stdout.is_empty(), "no partial output on a clap error");
    assert!(
        stderr.contains("possible values"),
        "clap must list the valid formats:\n{stderr}"
    );
}

#[test]
fn expanded_shows_per_sample_stations() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(dir.path().join("raw")).unwrap();
    std::fs::write(dir.path().join("raw/a.txt"), "a").unwrap();
    std::fs::write(dir.path().join("raw/b.txt"), "b").unwrap();
    let wf = write_workflow(dir.path(), "w.oxoflow", sampled());

    let (ok, template, _) = run_graph(dir.path(), &wf, &["-f", "metro"]);
    assert!(ok);
    assert_eq!(
        count_stations(&template),
        2,
        "template DAG has 2 stations:\n{template}"
    );

    let (ok, expanded, _) = run_graph(dir.path(), &wf, &["-f", "metro", "--expanded"]);
    assert!(ok);
    assert_eq!(
        count_stations(&expanded),
        4,
        "expanded DAG has 4 stations:\n{expanded}"
    );
    assert!(
        expanded.contains("step1_auto-discovered_a"),
        "instance naming must follow the runtime DAG:\n{expanded}"
    );
}

#[test]
fn output_file_keeps_stdout_clean_for_pipes() {
    let dir = tempfile::tempdir().unwrap();
    let wf = write_workflow(dir.path(), "w.oxoflow", two_step());
    let target = dir.path().join("graph.dot");
    let (ok, stdout, stderr) = run_graph(
        dir.path(),
        &wf,
        &["-f", "dot", "-o", target.to_str().unwrap()],
    );
    assert!(ok);
    assert!(
        stdout.is_empty(),
        "stdout reserved for machine output:\n{stdout}"
    );
    assert!(
        stderr.contains("Graph saved to"),
        "save notice expected:\n{stderr}"
    );
    let written = std::fs::read_to_string(&target).unwrap();
    assert!(
        written.starts_with("digraph"),
        "file holds the DOT body:\n{written}"
    );
}
