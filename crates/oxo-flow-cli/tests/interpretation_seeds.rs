//! Contract gate for the `report --ai` interpretation seed corpus
//! (`eval/interpretation/seeds/`, issue #359 / audit wave 4 #1168).
//!
//! Every seed's `manifest.json` declares `run_expect_exit`: the exit status
//! the PLANTED fault must produce when the benchmark runner executes the
//! documented command (`run` for the report surfaces, `dry-run` for the
//! preview surfaces). That value is a contract, not an engine snapshot: the
//! fault must stay *reachable* under the engine's documented execution
//! contracts — fail-fast on a missing required source input
//! (`docs/guide/src/commands/run.md` → "missing required source input"),
//! the wildcard safe-default charset gate
//! (`docs/guide/src/reference/wildcards.md`), and declared resource-group
//! capacity.
//!
//! The gate exists because the benchmark runner marks a seed whose exit
//! mismatches as `broken` and EXCLUDES it from the denominator instead of
//! failing: a fixture that no longer reaches its planted fault silently
//! shrinks the measured corpus. `s04-silent-failure` was exactly that class
//! after run-time input readiness became fail-closed — it declared an
//! absent input, so the engine aborted before the silent exit-0 pipeline
//! could run, and the old manifest still expected 0.
//!
//! Determinism: the children run with a pinned `PATH` (`/usr/bin:/bin`) so
//! host-installed bioinformatics tools cannot flip a seed. Seeds whose
//! contract depends on a tool being absent declare it in `requires_absent`;
//! if such a tool is present on the pinned path the seed is skipped with a
//! message (the benchmark's own documented host precondition), never
//! silently mis-scored.

#![cfg(unix)]

use serde_json::Value;
use std::path::{Path, PathBuf};
use std::process::Command;

const PINNED_PATH: &str = "/usr/bin:/bin";

fn seeds_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../eval/interpretation/seeds")
}

fn tool_on_pinned_path(name: &str) -> bool {
    PINNED_PATH
        .split(':')
        .any(|dir| Path::new(dir).join(name).is_file())
}

fn write_setup(workdir: &Path, manifest: &Value) {
    let Some(setup) = manifest.get("setup").and_then(Value::as_object) else {
        return;
    };
    for (rel, content) in setup {
        let content = content
            .as_str()
            .unwrap_or_else(|| panic!("seed {rel}: setup values must be strings"));
        let path = workdir.join(rel);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).unwrap();
        }
        std::fs::write(path, content).unwrap();
    }
}

#[test]
fn interpretation_seed_exit_expectations_match_the_planted_fault() {
    let mut checked = 0usize;
    let mut skipped = Vec::new();
    let mut seed_dirs: Vec<PathBuf> = std::fs::read_dir(seeds_dir())
        .unwrap_or_else(|error| panic!("seed corpus {}: {error}", seeds_dir().display()))
        .map(|entry| entry.unwrap().path())
        .filter(|path| path.is_dir())
        .collect();
    seed_dirs.sort();

    for seed_dir in seed_dirs {
        let manifest: Value =
            serde_json::from_str(&std::fs::read_to_string(seed_dir.join("manifest.json")).unwrap())
                .unwrap();
        let id = manifest["id"].as_str().expect("seed id");
        let surface = manifest["surface"].as_str().expect("seed surface");
        let expected = manifest["run_expect_exit"]
            .as_i64()
            .expect("seed run_expect_exit");

        let absent = manifest
            .get("requires_absent")
            .and_then(Value::as_array)
            .map(|tools| {
                tools
                    .iter()
                    .filter_map(Value::as_str)
                    .filter(|tool| tool_on_pinned_path(tool))
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        if !absent.is_empty() {
            skipped.push(format!("{id} (present on PATH: {})", absent.join(", ")));
            continue;
        }

        let work = tempfile::tempdir().unwrap();
        std::fs::copy(
            seed_dir.join("workflow.oxoflow"),
            work.path().join("workflow.oxoflow"),
        )
        .unwrap();
        write_setup(work.path(), &manifest);

        let output = Command::new(env!("CARGO_BIN_EXE_oxo-flow"))
            .args(match surface {
                "dry-run" => ["dry-run", "workflow.oxoflow"].as_slice(),
                _ => ["run", "workflow.oxoflow"].as_slice(),
            })
            .current_dir(work.path())
            .env("PATH", PINNED_PATH)
            .env("OXO_FLOW_AI_PROVIDER", "disabled")
            .output()
            .unwrap();

        assert_eq!(
            output.status.code(),
            Some(expected as i32),
            "seed {id}: the planted fault must exit {expected}, got {:?}\n\
             The manifest expectation must trace to the engine's documented \
             execution contract, not to a previous binary's behavior.\n\
             stdout:\n{}\nstderr:\n{}",
            output.status.code(),
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr),
        );
        checked += 1;
    }

    assert!(
        checked >= 10,
        "expected the seed corpus to cover at least 10 runnable seeds, checked {checked} (skipped: {skipped:?})"
    );
    assert!(
        skipped.len() <= 1,
        "at most the declared-absent-binary seed may be skipped: {skipped:?}"
    );
}
