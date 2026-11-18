//! Issue #1153 (WF-02): degenerate scatter/split domains must fail closed.
//!
//! One test per enumerated degenerate input. Each test runs the real CLI
//! and asserts a NON-ZERO exit plus the diagnostic text naming the rule and
//! the degenerate input — a successful plan that executes nothing (or the
//! wrong number of instances) is the bug.
//!
//! Data-dependent sources (a `sample_pattern` matching zero files, a
//! `split.glob` matching zero files, a `config.<key>` that `--arg` could
//! still supply) stay valid at `validate` time by design: lint/validate
//! must work without the data mounted. Those tests gate on `dry-run`, the
//! command that builds the plan.

use std::path::{Path, PathBuf};
use std::process::Command;

fn oxo() -> Command {
    Command::new(env!("CARGO_BIN_EXE_oxo-flow"))
}

/// Write `content` to `name` under `dir` and return the path.
fn write_wf(dir: &Path, name: &str, content: &str) -> PathBuf {
    let path = dir.join(name);
    std::fs::write(&path, content).expect("workflow written");
    path
}

/// Run `oxo-flow <args..>` in `dir`; return (success, stdout, stderr).
fn run_cli(dir: &Path, args: &[&str]) -> (bool, String, String) {
    let out = oxo()
        .args(args)
        .current_dir(dir)
        .output()
        .expect("CLI runs");
    (
        out.status.success(),
        String::from_utf8_lossy(&out.stdout).to_string(),
        String::from_utf8_lossy(&out.stderr).to_string(),
    )
}

/// `validate` must fail with E023 and all needles in the diagnostic.
fn assert_validate_e023(dir: &Path, name: &str, needles: &[&str]) {
    let (ok, stdout, stderr) = run_cli(dir, &["validate", name]);
    let combined = format!("{stdout}\n{stderr}");
    assert!(!ok, "validate must exit non-zero for {name}: {combined}");
    assert!(
        combined.contains("E023"),
        "E023 code missing for {name}: {combined}"
    );
    for needle in needles {
        assert!(
            combined.contains(needle),
            "diagnostic for {name} must contain {needle:?}: {combined}"
        );
    }
}

/// `dry-run --json` must fail with all needles in the error payload.
fn assert_dry_run_fails(dir: &Path, name: &str, needles: &[&str]) {
    let (ok, stdout, stderr) = run_cli(dir, &["--json", "dry-run", name]);
    let combined = format!("{stdout}\n{stderr}");
    assert!(!ok, "dry-run must exit non-zero for {name}: {combined}");
    for needle in needles {
        assert!(
            combined.contains(needle),
            "dry-run diagnostic for {name} must contain {needle:?}: {combined}"
        );
    }
}

const EMPTY_SCATTER: &str = r#"
[workflow]
name = "empty_scatter"

[[rules]]
name = "per_chunk"
input = ["sample.txt"]
output = ["chunks/{chunk}.bam"]
shell = "cp {input} {output}"

[rules.scatter]
variable = "chunk"
values = []
"#;

#[test]
fn empty_scatter_values_fails_closed() {
    let dir = tempfile::tempdir().unwrap();
    write_wf(dir.path(), "w.oxoflow", EMPTY_SCATTER);
    assert_validate_e023(
        dir.path(),
        "w.oxoflow",
        &["per_chunk", "empty value domain", "chunk"],
    );
    assert_dry_run_fails(
        dir.path(),
        "w.oxoflow",
        &["per_chunk", "empty value domain"],
    );
}

#[test]
fn empty_scatter_values_from_fails_closed() {
    let dir = tempfile::tempdir().unwrap();
    write_wf(
        dir.path(),
        "w.oxoflow",
        r#"
[workflow]
name = "empty_scatter_values_from"

[config]
items = []

[[rules]]
name = "per_item"
input = ["sample.txt"]
output = ["out/{item}.bam"]
shell = "cp {input} {output}"

[rules.scatter]
variable = "item"
values_from = "config.items"
"#,
    );
    assert_validate_e023(
        dir.path(),
        "w.oxoflow",
        &["per_item", "empty value domain", "item"],
    );
    assert_dry_run_fails(dir.path(), "w.oxoflow", &["per_item", "empty value domain"]);
}

#[test]
fn unresolvable_scatter_values_from_fails_closed_on_plan() {
    // An undefined config key can still be supplied by `--arg`, so validate
    // does not claim it is statically empty; planning without it must fail.
    let dir = tempfile::tempdir().unwrap();
    write_wf(
        dir.path(),
        "w.oxoflow",
        r#"
[workflow]
name = "missing_scatter_values_from"

[[rules]]
name = "per_item"
input = ["sample.txt"]
output = ["out/{item}.bam"]
shell = "cp {input} {output}"

[rules.scatter]
variable = "item"
values_from = "config.items"
"#,
    );
    assert_dry_run_fails(dir.path(), "w.oxoflow", &["per_item", "empty value domain"]);
}

#[test]
fn empty_scatter_variable_fails_closed() {
    let dir = tempfile::tempdir().unwrap();
    write_wf(
        dir.path(),
        "w.oxoflow",
        r#"
[workflow]
name = "empty_scatter_variable"

[[rules]]
name = "per"
input = ["sample.txt"]
output = ["out/a.bam"]
shell = "cp {input} {output}"

[rules.scatter]
variable = ""
values = ["a", "b"]
"#,
    );
    assert_validate_e023(
        dir.path(),
        "w.oxoflow",
        &["scatter rule 'per'", "empty scatter variable"],
    );
    assert_dry_run_fails(
        dir.path(),
        "w.oxoflow",
        &["scatter rule 'per'", "empty scatter variable"],
    );
}

#[test]
fn empty_sample_group_samples_fails_closed() {
    let dir = tempfile::tempdir().unwrap();
    write_wf(
        dir.path(),
        "w.oxoflow",
        r#"
[workflow]
name = "empty_sample_group"

[[sample_groups]]
name = "cohort"
samples = []

[[rules]]
name = "per_sample"
input = ["raw/{sample}.txt"]
output = ["out/{sample}.txt"]
shell = "cp {input} {output}"
"#,
    );
    assert_validate_e023(dir.path(), "w.oxoflow", &["cohort", "has no samples"]);
    assert_dry_run_fails(
        dir.path(),
        "w.oxoflow",
        &[
            "per_sample",
            "{sample}",
            "no sample domain produced any instance",
        ],
    );
}

#[test]
fn empty_values_table_fails_closed() {
    let dir = tempfile::tempdir().unwrap();
    write_wf(
        dir.path(),
        "w.oxoflow",
        r#"
[workflow]
name = "empty_values"

[[values]]
name = "assembler"
values = []

[[rules]]
name = "assemble"
input = ["reads.fq"]
output = ["out/{assembler}.fa"]
shell = "assemble -o {output} {input}"
"#,
    );
    // [[values]] emptiness fails at parse (before validate_format), so the
    // diagnostic carries no E-code; the message must still name the table.
    let (ok, stdout, stderr) = run_cli(dir.path(), &["validate", "w.oxoflow"]);
    let combined = format!("{stdout}\n{stderr}");
    assert!(!ok, "validate must exit non-zero: {combined}");
    assert!(
        combined.contains("[[values]] table 'assembler' has no values"),
        "{combined}"
    );
    assert_dry_run_fails(
        dir.path(),
        "w.oxoflow",
        &["[[values]] table 'assembler' has no values"],
    );
}

#[test]
fn empty_values_from_fails_closed() {
    let dir = tempfile::tempdir().unwrap();
    write_wf(
        dir.path(),
        "w.oxoflow",
        r#"
[workflow]
name = "empty_values_from"

[config]
assemblers = []

[[values]]
name = "assembler"
values_from = "config.assemblers"

[[rules]]
name = "assemble"
input = ["reads.fq"]
output = ["out/{assembler}.fa"]
shell = "assemble -o {output} {input}"
"#,
    );
    let (ok, stdout, stderr) = run_cli(dir.path(), &["validate", "w.oxoflow"]);
    let combined = format!("{stdout}\n{stderr}");
    assert!(!ok, "validate must exit non-zero: {combined}");
    assert!(combined.contains("resolved to no values"), "{combined}");
    assert_dry_run_fails(dir.path(), "w.oxoflow", &["resolved to no values"]);
}

#[test]
fn zero_match_sample_pattern_fails_closed_on_plan() {
    let dir = tempfile::tempdir().unwrap();
    write_wf(
        dir.path(),
        "w.oxoflow",
        r#"
[workflow]
name = "zero_match_sample_pattern"
sample_pattern = "data/{sample}.txt"

[[rules]]
name = "per_sample"
input = ["{sample}.txt"]
output = ["out/{sample}.txt"]
shell = "cp {input} {output}"
"#,
    );
    assert_dry_run_fails(
        dir.path(),
        "w.oxoflow",
        &["per_sample", "{sample}", "matched no files"],
    );
}

#[test]
fn split_n_zero_fails_closed() {
    let dir = tempfile::tempdir().unwrap();
    write_wf(dir.path(), "w.oxoflow", &split_n_workflow("0"));
    assert_validate_e023(
        dir.path(),
        "w.oxoflow",
        &[
            "transform rule 'chunked'",
            "split.n '0'",
            "greater than zero",
        ],
    );
    assert_dry_run_fails(
        dir.path(),
        "w.oxoflow",
        &["split.n '0'", "greater than zero"],
    );
}

#[test]
fn split_n_negative_fails_closed() {
    let dir = tempfile::tempdir().unwrap();
    write_wf(dir.path(), "w.oxoflow", &split_n_workflow("-3"));
    assert_validate_e023(
        dir.path(),
        "w.oxoflow",
        &["transform rule 'chunked'", "split.n '-3'", "negative"],
    );
    assert_dry_run_fails(dir.path(), "w.oxoflow", &["split.n '-3'", "negative"]);
}

#[test]
fn split_n_non_integer_fails_closed() {
    let dir = tempfile::tempdir().unwrap();
    write_wf(dir.path(), "w.oxoflow", &split_n_workflow("not-a-number"));
    assert_validate_e023(
        dir.path(),
        "w.oxoflow",
        &[
            "transform rule 'chunked'",
            "not-a-number",
            "neither a positive integer",
        ],
    );
    assert_dry_run_fails(
        dir.path(),
        "w.oxoflow",
        &["split.n 'not-a-number'", "neither a positive integer"],
    );
}

#[test]
fn split_n_above_expansion_cap_fails_closed() {
    // 100_001 is just past the shared MAX_COMBINATIONS ceiling. Before the
    // fix this ground through 100k map instances instead of failing.
    let dir = tempfile::tempdir().unwrap();
    write_wf(dir.path(), "w.oxoflow", &split_n_workflow("100001"));
    assert_validate_e023(
        dir.path(),
        "w.oxoflow",
        &[
            "transform rule 'chunked'",
            "split.n '100001'",
            "expansion cap",
        ],
    );
    assert_dry_run_fails(
        dir.path(),
        "w.oxoflow",
        &["split.n '100001'", "expansion cap"],
    );
}

#[test]
fn split_n_integer_overflow_fails_closed() {
    let dir = tempfile::tempdir().unwrap();
    write_wf(
        dir.path(),
        "w.oxoflow",
        &split_n_workflow("99999999999999999999999999"),
    );
    assert_validate_e023(
        dir.path(),
        "w.oxoflow",
        &["transform rule 'chunked'", "neither a positive integer"],
    );
    assert_dry_run_fails(dir.path(), "w.oxoflow", &["neither a positive integer"]);
}

#[test]
fn empty_split_by_fails_closed() {
    let dir = tempfile::tempdir().unwrap();
    write_wf(
        dir.path(),
        "w.oxoflow",
        r#"
[workflow]
name = "empty_split_by"

[[rules]]
name = "chunked"
input = ["sample.bam"]
output = ["merged.vcf"]

[rules.transform.split]
by = ""
values = ["a", "b"]

[rules.transform]
map = "call {input}"

[rules.transform.combine]
shell = "cat {chunks} > {output}"
"#,
    );
    assert_validate_e023(
        dir.path(),
        "w.oxoflow",
        &["transform rule 'chunked'", "empty split.by"],
    );
    assert_dry_run_fails(
        dir.path(),
        "w.oxoflow",
        &["transform rule 'chunked'", "empty split.by"],
    );
}

#[test]
fn missing_split_by_fails_closed_naming_the_rule() {
    // `by` deserializes to an empty string so the diagnostic names the
    // rule instead of a bare serde "missing field `by`" parse error.
    let dir = tempfile::tempdir().unwrap();
    write_wf(
        dir.path(),
        "w.oxoflow",
        r#"
[workflow]
name = "missing_split_by"

[[rules]]
name = "chunked"
input = ["sample.bam"]
output = ["merged.vcf"]

[rules.transform.split]
values = ["a", "b"]

[rules.transform]
map = "call {input}"

[rules.transform.combine]
shell = "cat {chunks} > {output}"
"#,
    );
    assert_validate_e023(
        dir.path(),
        "w.oxoflow",
        &["transform rule 'chunked'", "empty split.by"],
    );
    assert_dry_run_fails(
        dir.path(),
        "w.oxoflow",
        &["transform rule 'chunked'", "empty split.by"],
    );
}

#[test]
fn transform_without_split_source_fails_closed() {
    let dir = tempfile::tempdir().unwrap();
    write_wf(
        dir.path(),
        "w.oxoflow",
        r#"
[workflow]
name = "no_split_source"

[[rules]]
name = "chunked"
input = ["sample.bam"]
output = ["merged.vcf"]

[rules.transform.split]
by = "chunk"

[rules.transform]
map = "call {input}"

[rules.transform.combine]
shell = "cat {chunks} > {output}"
"#,
    );
    assert_validate_e023(
        dir.path(),
        "w.oxoflow",
        &["transform rule 'chunked'", "no split values"],
    );
    assert_dry_run_fails(
        dir.path(),
        "w.oxoflow",
        &["transform rule 'chunked'", "no split values"],
    );
}

#[test]
fn split_glob_zero_matches_fails_closed_on_plan() {
    let dir = tempfile::tempdir().unwrap();
    write_wf(
        dir.path(),
        "w.oxoflow",
        r#"
[workflow]
name = "split_glob_zero"

[[rules]]
name = "chunked"
input = ["sample.bam"]
output = ["merged.vcf"]

[rules.transform.split]
by = "chunk"
glob = "no-such-dir/*.txt"

[rules.transform]
map = "call {input}"

[rules.transform.combine]
shell = "cat {chunks} > {output}"
"#,
    );
    assert_dry_run_fails(
        dir.path(),
        "w.oxoflow",
        &["glob pattern", "matched no files"],
    );
}

/// A transform rule with `n = <value>` and no other split source.
fn split_n_workflow(n: &str) -> String {
    format!(
        r#"
[workflow]
name = "split_n"

[[rules]]
name = "chunked"
input = ["sample.bam"]
output = ["merged.vcf"]

[rules.transform.split]
by = "chunk"
n = "{n}"

[rules.transform]
map = "call --chunk {{chunk}} {{input}}"

[rules.transform.combine]
shell = "cat {{chunks}} > {{output}}"
"#
    )
}
