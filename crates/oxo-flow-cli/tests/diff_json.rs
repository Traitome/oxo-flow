//! Integration tests for `diff --json` (issue #1330): the machine-readable
//! DIFF must be consumable by CI/tooling. `diff` previously emitted only
//! colored human text. The JSON path must emit ONE document on stdout, keep
//! a stable shape, and still exit 1 when the workflows differ.

use std::process::Command;

fn oxo() -> Command {
    Command::new(env!("CARGO_BIN_EXE_oxo-flow"))
}

/// Two workflows used by the differing-pair cases.
const WF_A: &str = r#"
[workflow]
name = "demo"
version = "1.0.0"

[config]
threads = 4

[[rules]]
name = "align"
shell = "bwa mem ref.fa {input} > {output}"

[defaults]
threads = 2
"#;

const WF_B: &str = r#"
[workflow]
name = "demo"
version = "2.0.0"

[config]
threads = 8

[[rules]]
name = "align"
shell = "bwa mem -t 8 ref.fa {input} > {output}"

[[rules]]
name = "sort"
shell = "samtools sort {input} > {output}"

[defaults]
threads = 2
"#;

fn write_pair(dir: &std::path::Path, a: &str, b: &str) -> (std::path::PathBuf, std::path::PathBuf) {
    let pa = dir.join("a.oxoflow");
    let pb = dir.join("b.oxoflow");
    std::fs::write(&pa, a).unwrap();
    std::fs::write(&pb, b).unwrap();
    (pa, pb)
}

#[test]
fn diff_json_equal_pair_reports_equal_and_exits_zero() {
    let dir = std::env::temp_dir().join(format!("oxoflow_diff_json_eq_{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let (pa, pb) = write_pair(&dir, WF_A, WF_A);

    let out = oxo()
        .args(["diff", "--json"])
        .arg(&pa)
        .arg(&pb)
        .output()
        .expect("diff runs");
    let _ = std::fs::remove_dir_all(&dir);

    assert!(out.status.success(), "equal pair exits 0: {:?}", out.status);
    let stdout = String::from_utf8_lossy(&out.stdout);
    let parsed: serde_json::Value = serde_json::from_str(&stdout)
        .unwrap_or_else(|e| panic!("stdout is not valid JSON: {e}\n{stdout}"));

    assert_eq!(parsed["tool"], "oxo-flow diff");
    assert_eq!(parsed["equal"], true);
    assert_eq!(parsed["difference_count"], 0);
    assert_eq!(parsed["differences"], serde_json::json!({}));
}

#[test]
fn diff_json_differing_pair_reports_categories_and_exits_one() {
    let dir = std::env::temp_dir().join(format!("oxoflow_diff_json_ne_{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let (pa, pb) = write_pair(&dir, WF_A, WF_B);

    let out = oxo()
        .args(["diff", "--json"])
        .arg(&pa)
        .arg(&pb)
        .output()
        .expect("diff runs");
    let _ = std::fs::remove_dir_all(&dir);

    assert!(!out.status.success(), "differing pair exits non-zero");
    assert_eq!(out.status.code(), Some(1));

    let stdout = String::from_utf8_lossy(&out.stdout);
    let parsed: serde_json::Value = serde_json::from_str(&stdout)
        .unwrap_or_else(|e| panic!("stdout is not valid JSON: {e}\n{stdout}"));

    assert_eq!(parsed["tool"], "oxo-flow diff");
    assert_eq!(parsed["equal"], false);

    // Every category present in the diff must appear; the top-level shape
    // must stay {equal, differences} so a dropped category fails this test.
    let differences = parsed["differences"]
        .as_object()
        .expect("differences is an object");
    for required in ["workflow", "rules"] {
        assert!(
            differences.contains_key(required),
            "category {required:?} missing from {differences:?}"
        );
        assert!(
            differences[required]
                .as_array()
                .is_some_and(|a| !a.is_empty()),
            "category {required:?} must list at least one difference"
        );
    }
    assert_eq!(
        differences["workflow"][0], "version changed: \"1.0.0\" → \"2.0.0\"",
        "workflow category content"
    );

    // difference_count must match the number of listed descriptions.
    let listed: usize = differences
        .values()
        .map(|v| v.as_array().map_or(0, Vec::len))
        .sum();
    assert_eq!(parsed["difference_count"], listed);
}

#[test]
fn diff_without_json_keeps_human_output() {
    // Guard: the non-JSON path stays human (a colored report, exit 1).
    let dir = std::env::temp_dir().join(format!("oxoflow_diff_txt_{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let (pa, pb) = write_pair(&dir, WF_A, WF_B);

    let out = oxo()
        .arg("diff")
        .arg(&pa)
        .arg(&pb)
        .output()
        .expect("diff runs");
    let _ = std::fs::remove_dir_all(&dir);

    assert_eq!(out.status.code(), Some(1));
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("difference(s) between"),
        "human header: {stdout}"
    );
    assert!(
        serde_json::from_str::<serde_json::Value>(&stdout).is_err(),
        "human output must not be valid JSON"
    );
}
