//! Cross-surface remediation continuity (issue #1302, ROADMAP 12.3).
//!
//! Self-healing's value proposition is that a human reviews ONE repair patch
//! and it executes IDENTICALLY on every surface. The reviewed patch — a whole
//! `.oxoflow` workflow in the exact shape the CLI's `apply_fix` writes — is
//! the contract: this harness feeds the SAME patch through every apply
//! surface and asserts byte-identical applied workflow plus an identical
//! `REPAIR_APPLIED` / `REPAIR_RERUN_*` checkpoint outcome (`repair_record`
//! in checkpoint.json).
//!
//! This first cut pins the contract on the CLI engine itself:
//!
//! - **A — CLI apply**: `oxo-flow run <wf> --apply-patch <patch>` (the
//!   direct CLI surface).
//! - **C — Extension apply**: the exact argv the VS Code command builds
//!   (`run <wf> --apply-patch <patch> --workdir <dir>`).
//!
//! The web adapter lands with its own leg (B — `POST
//! /api/runs/{id}/apply-repair` against a real `oxo-flow-web` server), and
//! the VS Code command's argv shape is pinned by
//! `editors/vscode/src/test/unit/cliArgs.test.ts` (`applyRepairArgs`).
//!
//! Parity honesty (issue #1302 scope note): leg C shells the CLI binary —
//! the extension path IS the CLI path by construction, so this leg proves
//! the CLI path a second time under the extension's exact argv (including
//! `--workdir`), not the TypeScript surface itself. No cross-surface
//! equality is claimed beyond what each leg actually proves.
//!
//! Falsifiability: breaking any surface's routing to the shared
//! `apply_and_verify_repair` path (a rewritten workflow that diverges, a
//! skipped verification rerun, a lost `repair_record`) makes the observed
//! facts diverge and the parity assertions fail.

#[path = "common/mod.rs"]
mod common;

use common::{oxo_flow_cmd, workspace_bin};
use serde_json::{Value, json};
use std::path::Path;
use std::process::Output;

/// The seeded repairable fault: `boom` always fails after `prepare`
/// succeeds. Both surfaces start from this exact manifest.
const POISONED_RULES: &str = "\
[workflow]
name = \"remediation-continuity\"

[[rules]]
name = \"prepare\"
output = [\"out/prep.txt\"]
shell = \"mkdir -p out && echo ready > {output[0]}\"

[[rules]]
name = \"boom\"
input = [\"out/prep.txt\"]
output = [\"out/final.txt\"]
shell = \"exit 1\"
";

/// The reviewed repair — the ONE behavioral change every surface must land:
/// the complete workflow with `boom`'s shell fixed.
const REVIEWED_PATCH: &str = "\
[workflow]
name = \"remediation-continuity\"

[[rules]]
name = \"prepare\"
output = [\"out/prep.txt\"]
shell = \"mkdir -p out && echo ready > {output[0]}\"

[[rules]]
name = \"boom\"
input = [\"out/prep.txt\"]
output = [\"out/final.txt\"]
shell = \"echo fixed > {output[0]}\"
";

/// AI provider env vars a dev shell may export; the seeded manifest carries
/// no `[ai]` section so recovery can never trigger, but the legs strip them
/// anyway so the harness outcome cannot depend on ambient credentials.
const AI_ENV_VARS: &[&str] = &[
    "OXO_FLOW_AI_PROVIDER",
    "OXO_FLOW_AI_API_KEY",
    "OXO_FLOW_AI_API_URL",
    "OXO_FLOW_AI_MODEL",
    "ANTHROPIC_AUTH_TOKEN",
    "ANTHROPIC_API_KEY",
    "ANTHROPIC_BASE_URL",
    "ANTHROPIC_MODEL",
    "OPENAI_API_KEY",
    "OPENAI_BASE_URL",
    "OPENAI_MODEL",
    "DEEPSEEK_API_KEY",
    "OLLAMA_HOST",
];

/// The observed parity facts of one repair leg — everything the surfaces
/// must agree on. `applied_workflow` is the repaired workflow bytes (trailing
/// whitespace trimmed, matching the existing `apply_patch.rs` harness);
/// `repair_record` is the checkpoint's record minus `timestamp`/`detail`
/// (the only fields that legitimately differ run to run).
#[derive(Debug)]
struct Outcome {
    applied_workflow: String,
    applied: Option<bool>,
    session_id: Option<String>,
    rerun_attempted: Option<bool>,
    rerun_succeeded: Option<bool>,
    narrated_applied: bool,
    narrated_rerun_succeeded: bool,
}

fn trim_end(s: &str) -> String {
    s.trim_end().to_string()
}

fn cli_output(out: &Output) -> String {
    format!(
        "{}{}",
        String::from_utf8_lossy(&out.stderr),
        String::from_utf8_lossy(&out.stdout)
    )
}

fn checkpoint_json(dir: &Path) -> Value {
    let path = dir.join(".oxo-flow/checkpoint.json");
    serde_json::from_str(&std::fs::read_to_string(path).unwrap_or_else(|e| {
        panic!(
            "checkpoint at {} must exist and parse: {e}",
            dir.join(".oxo-flow/checkpoint.json").display()
        )
    }))
    .expect("checkpoint.json parses")
}

/// The facts one CLI leg produced: applied workflow, checkpoint repair
/// record, narration lines.
fn observe_cli_leg(dir: &Path, out: &Output) -> Outcome {
    let all = cli_output(out);
    let ck = checkpoint_json(dir);
    let rec = &ck["repair_record"];
    Outcome {
        applied_workflow: trim_end(&std::fs::read_to_string(dir.join("w.oxoflow")).unwrap()),
        applied: rec["applied"].as_bool(),
        session_id: rec["session_id"].as_str().map(str::to_string),
        rerun_attempted: rec["rerun_attempted"].as_bool(),
        rerun_succeeded: rec["rerun_succeeded"].as_bool(),
        narrated_applied: all.contains("REPAIR_APPLIED session_id=reviewed-patch"),
        // The rerun line's payload is the outcome detail, not the session id
        // (`REPAIR_RERUN_SUCCEEDED <detail>`); the session label rides only
        // REPAIR_APPLIED. Match the stable prefix.
        narrated_rerun_succeeded: all.contains("REPAIR_RERUN_SUCCEEDED "),
    }
}

/// Seed the fault: run the poisoned workflow to a failed checkpoint.
fn seed_failure(dir: &Path) -> Output {
    std::fs::write(dir.join("w.oxoflow"), POISONED_RULES).unwrap();
    let mut cmd = oxo_flow_cmd();
    cmd.env("NO_COLOR", "1").current_dir(dir);
    for var in AI_ENV_VARS {
        cmd.env_remove(var);
    }
    let out = cmd
        .args(["run", "w.oxoflow", "--no-report-snapshot"])
        .output()
        .expect("oxo-flow spawns");
    assert!(
        !out.status.success(),
        "the poisoned run must fail:\n{}",
        cli_output(&out)
    );
    out
}

/// Leg A — the CLI surface: `run --apply-patch <reviewed file>`.
fn leg_cli() -> Outcome {
    let dir = tempfile::tempdir().unwrap();
    seed_failure(dir.path());
    std::fs::write(dir.path().join("reviewed.oxoflow"), REVIEWED_PATCH).unwrap();
    let mut cmd = oxo_flow_cmd();
    cmd.env("NO_COLOR", "1").current_dir(dir.path());
    for var in AI_ENV_VARS {
        cmd.env_remove(var);
    }
    let out = cmd
        .args([
            "run",
            "w.oxoflow",
            "--no-report-snapshot",
            "--apply-patch",
            "reviewed.oxoflow",
        ])
        .output()
        .expect("oxo-flow spawns");
    assert!(
        out.status.success(),
        "the CLI apply must apply and verify:\n{}",
        cli_output(&out)
    );
    observe_cli_leg(dir.path(), &out)
}

/// Leg C — the extension surface's exact argv (`run <wf> --apply-patch
/// <patch> --workdir <dir>`). The VS Code command shells the CLI, so this
/// proves the CLI path under the extension's invocation shape; the TS-side
/// argv construction is pinned by the extension unit tests.
fn leg_extension_argv() -> Outcome {
    let dir = tempfile::tempdir().unwrap();
    seed_failure(dir.path());
    std::fs::write(dir.path().join("reviewed.oxoflow"), REVIEWED_PATCH).unwrap();
    let mut cmd = oxo_flow_cmd();
    cmd.env("NO_COLOR", "1").current_dir(dir.path());
    for var in AI_ENV_VARS {
        cmd.env_remove(var);
    }
    let workdir = dir.path().to_string_lossy().into_owned();
    let out = cmd
        .args([
            "run",
            "w.oxoflow",
            "--apply-patch",
            "reviewed.oxoflow",
            "--workdir",
            &workdir,
        ])
        .output()
        .expect("oxo-flow spawns");
    assert!(
        out.status.success(),
        "the extension-shaped apply must apply and verify:\n{}",
        cli_output(&out)
    );
    observe_cli_leg(dir.path(), &out)
}

/// The parity assertion: every surface lands the SAME reviewed patch
/// (byte-identical applied workflow) with the SAME REPAIR_* checkpoint
/// outcome.
fn assert_parity(legs: &[(&str, Outcome)]) {
    let (_, first) = &legs[0];
    assert_eq!(
        first.applied_workflow,
        trim_end(REVIEWED_PATCH),
        "the first leg must land the reviewed patch exactly"
    );
    assert_eq!(
        first.session_id.as_deref(),
        Some("reviewed-patch"),
        "the reviewed patch narrates its stable session label"
    );
    assert!(
        first.applied == Some(true)
            && first.rerun_attempted == Some(true)
            && first.rerun_succeeded == Some(true),
        "the first leg must apply AND verify: {:?}",
        first
    );
    assert!(
        first.narrated_applied && first.narrated_rerun_succeeded,
        "the first leg must narrate the REPAIR_* contract: {:?}",
        first
    );
    for (name, outcome) in &legs[1..] {
        assert_eq!(
            outcome.applied_workflow, first.applied_workflow,
            "leg {name} must land the byte-identical repaired workflow"
        );
        assert_eq!(
            outcome.session_id, first.session_id,
            "leg {name} must carry the same reviewed-patch session label"
        );
        assert_eq!(
            outcome.applied, first.applied,
            "leg {name} applied flag must match"
        );
        assert_eq!(
            outcome.rerun_attempted, first.rerun_attempted,
            "leg {name} rerun_attempted must match"
        );
        assert_eq!(
            outcome.rerun_succeeded, first.rerun_succeeded,
            "leg {name} rerun_succeeded must match"
        );
        assert_eq!(
            outcome.narrated_applied, first.narrated_applied,
            "leg {name} must narrate REPAIR_APPLIED like the first leg"
        );
        assert_eq!(
            outcome.narrated_rerun_succeeded, first.narrated_rerun_succeeded,
            "leg {name} must narrate REPAIR_RERUN_SUCCEEDED like the first leg"
        );
    }
}

/// One test on purpose: the legs share the patch contract and run
/// sequentially (the web leg will spawn a real server).
#[test]
fn reviewed_patch_applies_identically_on_every_surface() {
    // CLI leg first (fast, no server): establishes the parity baseline.
    let cli = leg_cli();
    // Extension-argv leg: same CLI binary under the extension's invocation.
    let extension = leg_extension_argv();
    assert_parity(&[("cli", cli), ("extension-argv", extension)]);
}

/// The binaries this harness needs — a clearer panic than the generic
/// workspace_bin message when a test binary runs without the build.
#[test]
fn harness_prerequisites() {
    let _ = workspace_bin("oxo-flow");
}
