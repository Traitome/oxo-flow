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
//! Legs:
//!
//! - **A — CLI apply**: `oxo-flow run <wf> --apply-patch <patch>` (the
//!   direct CLI surface).
//! - **B — Web apply**: `POST /api/runs/{id}/apply-repair` against a real
//!   `oxo-flow-web` server. The route stages the patch and shells the SAME
//!   CLI path through the web executor's spawn machinery.
//! - **C — Extension apply**: the exact argv the VS Code command builds
//!   (`run <wf> --apply-patch <patch> --workdir <dir>`; pinned by the
//!   extension's own unit test on `applyRepairArgs`).
//!
//! Parity honesty (issue #1302 scope note): leg C shells the CLI binary —
//! the extension path IS the CLI path by construction, so this leg proves
//! the CLI path a second time under the extension's exact argv (including
//! `--workdir`), not the TypeScript surface itself. The TS-side argv shape
//! is pinned separately by `editors/vscode/src/test/unit/cliArgs.test.ts`.
//! No three-way equality is claimed beyond what these legs actually prove:
//! A == B end-to-end (byte-identical workflow + repair record), C == A/B on
//! the same facts via the extension's argv.
//!
//! Falsifiability: breaking any surface's routing to the shared
//! `apply_and_verify_repair` path (a rewritten workflow that diverges, a
//! skipped verification rerun, a lost `repair_record`) makes the observed
//! facts diverge and the parity assertions fail.

#[path = "common/mod.rs"]
mod common;

use common::{api_get, api_post, oxo_flow_cmd, spawn_web_server, workspace_bin};
use serde_json::{Value, json};
use std::path::Path;
use std::process::Output;
use std::time::{Duration, Instant};

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

/// Poll a JSON GET until `done` says stop; panics with the server log tail
/// when the deadline expires so a stalled run is diagnosable.
async fn poll_until<F>(base: &str, path: &str, log_path: &Path, mut done: F) -> Value
where
    F: FnMut(&Value) -> bool,
{
    let deadline = Instant::now() + Duration::from_secs(180);
    loop {
        assert!(
            Instant::now() < deadline,
            "timed out polling GET {path}\n{}",
            std::fs::read_to_string(log_path).unwrap_or_else(|_| "(no server log)".into())
        );
        let (status, body) = api_get(base, path, None).await;
        assert_eq!(status.as_u16(), 200, "GET {path} failed: {body}");
        if done(&body) {
            return body;
        }
        tokio::time::sleep(Duration::from_millis(400)).await;
    }
}

/// Leg B — the web surface: `POST /api/runs/{id}/apply-repair` against a
/// real server. The seeded fault is created THROUGH the web (the same way a
/// UI user would), the run fails, and the reviewed patch is applied via the
/// route; the outcome is read back through `GET /api/runs/{id}/repair` and
/// the applied workflow bytes from the run workdir.
async fn leg_web(dir: &Path) -> Outcome {
    let ws = dir.join("ws");
    let port = common::free_port();
    let server = spawn_web_server(dir, port, &[("OXO_FLOW_WORKSPACE", ws.to_str().unwrap())]);
    let base = server.base.clone();

    // 1. Seed the fault through the web: create the run from the poisoned
    //    manifest and wait for it to fail.
    let (status, created) = api_post(
        &base,
        "/api/runs",
        None,
        json!({"toml_content": POISONED_RULES}),
    )
    .await;
    assert_eq!(status.as_u16(), 200, "create run must succeed: {created}");
    let seed_run = created["run_id"]
        .as_str()
        .or_else(|| created["id"].as_str())
        .expect("create response carries the run id")
        .to_string();
    let failed = poll_until(
        &base,
        &format!("/api/runs/{seed_run}"),
        &server.log_path,
        |b| matches!(b["status"].as_str(), Some("failed")),
    )
    .await;
    let workdir = failed["workdir"]
        .as_str()
        .expect("failed run carries its workdir")
        .to_string();

    // 2. Failure-path first: a patch that is not a workflow document must be
    //    refused BEFORE anything is staged or spawned.
    let (status, body) = api_post(
        &base,
        &format!("/api/runs/{seed_run}/apply-repair"),
        None,
        json!({"patch": "not [toml at all"}),
    )
    .await;
    assert_eq!(
        status.as_u16(),
        400,
        "a non-workflow patch must be rejected: {body}"
    );
    assert_eq!(
        body["code"], "INVALID_PATCH",
        "structured error code: {body}"
    );

    // 3. Apply the reviewed patch.
    let (status, applied) = api_post(
        &base,
        &format!("/api/runs/{seed_run}/apply-repair"),
        None,
        json!({"patch": REVIEWED_PATCH}),
    )
    .await;
    assert_eq!(
        status.as_u16(),
        200,
        "apply-repair must stage and spawn: {applied}"
    );
    let repair_run = applied["run_id"]
        .as_str()
        .expect("apply-repair response carries the new run id")
        .to_string();

    // 4. Wait for the apply-and-verify run to reach a terminal state. The
    //    patch applies only when `boom` fails again, then the bounded
    //    verification rerun completes the workflow — so the run COMPLETES.
    poll_until(
        &base,
        &format!("/api/runs/{repair_run}"),
        &server.log_path,
        |b| {
            matches!(
                b["status"].as_str(),
                Some("completed") | Some("failed") | Some("cancelled")
            )
        },
    )
    .await;

    // 5. Honesty guard: a repair on a run with nothing to repair is refused.
    let (status, body) = api_post(
        &base,
        &format!("/api/runs/{repair_run}/apply-repair"),
        None,
        json!({"patch": REVIEWED_PATCH}),
    )
    .await;
    assert_eq!(
        status.as_u16(),
        409,
        "a completed run has nothing to repair: {body}"
    );
    assert_eq!(
        body["code"], "RUN_NOT_FAILED",
        "structured error code: {body}"
    );

    // 6. Read the REPAIR_* record from the checkpoint through the GET route.
    let (_, repair_doc) = api_get(&base, &format!("/api/runs/{repair_run}/repair"), None).await;
    assert_eq!(
        repair_doc["checkpoint_present"], true,
        "the repaired run's checkpoint must be readable: {repair_doc}"
    );
    let rec = &repair_doc["repair_record"];
    let applied_log =
        std::fs::read_to_string(Path::new(&workdir).join("execution.log")).unwrap_or_default();
    Outcome {
        applied_workflow: trim_end(
            &std::fs::read_to_string(Path::new(&workdir).join("workflow.oxoflow")).unwrap(),
        ),
        applied: rec["applied"].as_bool(),
        session_id: rec["session_id"].as_str().map(str::to_string),
        rerun_attempted: rec["rerun_attempted"].as_bool(),
        rerun_succeeded: rec["rerun_succeeded"].as_bool(),
        narrated_applied: applied_log.contains("REPAIR_APPLIED session_id=reviewed-patch"),
        narrated_rerun_succeeded: applied_log.contains("REPAIR_RERUN_SUCCEEDED "),
    }
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
/// sequentially (the web leg spawns a real server; `OXO_FLOW_BIN` and the
/// workspace root are process-global).
#[tokio::test]
async fn reviewed_patch_applies_identically_on_every_surface() {
    // CLI leg first (fast, no server): establishes the parity baseline.
    let cli = leg_cli();
    // Extension-argv leg: same CLI binary under the extension's invocation.
    let extension = leg_extension_argv();
    // Web leg: the full route path through a real server.
    let dir = tempfile::tempdir().unwrap();
    let web = leg_web(dir.path()).await;
    assert_parity(&[("cli", cli), ("extension-argv", extension), ("web", web)]);
}

/// The binaries this harness needs — a clearer panic than the generic
/// workspace_bin message when a test binary runs without the build.
#[test]
fn harness_prerequisites() {
    let _ = workspace_bin("oxo-flow");
}
