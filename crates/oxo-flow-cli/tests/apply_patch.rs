//! Issue #1302: `run --apply-patch <FILE>` (reviewed repair file) must
//! produce the SAME outcome as the AI-decided repair path for the same
//! seeded fault — the same rewritten workflow and the same
//! `REPAIR_RERUN_*` cross-agent contract — because both route through the
//! shared `apply_and_verify_repair` path (reuse, never retype).
//!
//! Falsifiability: the two legs assert against a single shared expectation
//! of the repaired manifest. Deleting the flag threading, the reviewed-patch
//! branch, or reverting to a separate apply path makes the `--apply-patch`
//! leg stop rewriting the workflow / stop narrating REPAIR_* and the
//! equivalence assertion fails.

use std::io::{ErrorKind, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::Path;
use std::process::{Command, Output};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

/// The `[ai]` section authorizes auto-apply and routes the provider at the
/// local stub. It is carried by BOTH legs' input so the two surfaces start
/// from the identical manifest and must converge on the identical repair
/// (`validate_fix` refuses a fix that drops a top-level table, so the
/// AI-decided body must keep this section — and so must the reviewed file,
/// for the two outcomes to match exactly).
const AI_HEADER: &str = "[ai]\nenabled = true\nprovider = \"ollama\"\nauto_fix = \"always\"\n\n";

/// The seeded repairable fault: `boom` always fails after `prepare` succeeds.
const POISONED_RULES: &str = "\
[workflow]
name = \"repair-loop\"

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

/// The repaired rules — the ONE behavioral change both legs must land.
const FIXED_RULES: &str = "\
[workflow]
name = \"repair-loop\"

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

/// Both legs start here and must end at `repaired_manifest()`.
fn poisoned_manifest() -> String {
    format!("{AI_HEADER}{POISONED_RULES}")
}

/// The single repaired manifest both surfaces must produce.
fn repaired_manifest() -> String {
    format!("{AI_HEADER}{FIXED_RULES}")
}

/// A bounded loopback HTTP stub serving ONE fixed Ollama-shaped body to
/// every request; exits after a deadline so a regression that stops calling
/// the provider fails the assertion instead of hanging.
struct Stub {
    url: String,
    served: Arc<std::sync::atomic::AtomicUsize>,
    stop: Arc<AtomicBool>,
    handle: Option<std::thread::JoinHandle<()>>,
}

impl Stub {
    fn spawn(body: String) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind stub");
        listener.set_nonblocking(true).expect("nonblocking stub");
        let url = format!("http://{}", listener.local_addr().unwrap());
        let served = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let stop = Arc::new(AtomicBool::new(false));
        let thread_served = served.clone();
        let thread_stop = stop.clone();
        let handle = std::thread::spawn(move || {
            let deadline = Instant::now() + Duration::from_secs(20);
            while Instant::now() < deadline && !thread_stop.load(Ordering::Relaxed) {
                match listener.accept() {
                    Ok((mut stream, _)) => {
                        read_http_request(&mut stream);
                        thread_served.fetch_add(1, Ordering::Relaxed);
                        let response = format!(
                            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                            body.len(),
                            body
                        );
                        let _ = stream.write_all(response.as_bytes());
                    }
                    Err(e) if e.kind() == ErrorKind::WouldBlock => {
                        std::thread::sleep(Duration::from_millis(5));
                    }
                    Err(_) => break,
                }
            }
        });
        Self {
            url,
            served,
            stop,
            handle: Some(handle),
        }
    }

    fn hits(&self) -> usize {
        self.served.load(Ordering::Relaxed)
    }
}

impl Drop for Stub {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
    }
}

fn read_http_request(stream: &mut TcpStream) {
    stream.set_read_timeout(Some(Duration::from_secs(2))).ok();
    let mut buf: Vec<u8> = Vec::new();
    let mut chunk = [0u8; 8192];
    loop {
        match stream.read(&mut chunk) {
            Ok(0) => break,
            Ok(n) => {
                buf.extend_from_slice(&chunk[..n]);
                if let Some(head_end) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
                    let head = String::from_utf8_lossy(&buf[..head_end]).to_ascii_lowercase();
                    let length = head
                        .lines()
                        .find_map(|line| line.strip_prefix("content-length:"))
                        .and_then(|value| value.trim().parse::<usize>().ok())
                        .unwrap_or(0);
                    if buf.len() >= head_end + 4 + length {
                        break;
                    }
                }
            }
            Err(_) => break,
        }
    }
}

fn oxo(dir: &Path) -> Command {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_oxo-flow"));
    cmd.env("NO_COLOR", "1").current_dir(dir);
    cmd
}

fn run(cmd: &mut Command) -> Output {
    cmd.output().expect("oxo-flow spawns")
}

fn stderr(out: &Output) -> String {
    String::from_utf8_lossy(&out.stderr).into_owned()
}

fn stdout(out: &Output) -> String {
    String::from_utf8_lossy(&out.stdout).into_owned()
}

fn checkpoint(dir: &Path) -> serde_json::Value {
    let path = dir.join(".oxo-flow/checkpoint.json");
    serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap()
}

/// The observed parity facts of one repair leg — everything the two surfaces
/// must agree on, session id excluded (the only intended difference).
struct Outcome {
    workflow: String,
    failed_empty: bool,
    boom_completed: bool,
    repair_applied: bool,
    rerun_attempted: bool,
    rerun_succeeded: bool,
    applied_narrated: bool,
    rerun_succeeded_narrated: bool,
}

fn observe(dir: &Path, out: &Output) -> Outcome {
    let ck = checkpoint(dir);
    let rec = &ck["repair_record"];
    let all_err = format!("{}\n{}", stderr(out), stdout(out));
    Outcome {
        workflow: std::fs::read_to_string(dir.join("w.oxoflow"))
            .unwrap()
            .trim_end()
            .to_string(),
        failed_empty: ck["failed_rules"].as_array().unwrap().is_empty(),
        boom_completed: ck["completed_rules"]
            .as_array()
            .unwrap()
            .iter()
            .any(|r| r == "boom"),
        repair_applied: rec["applied"] == serde_json::json!(true),
        rerun_attempted: rec["rerun_attempted"] == serde_json::json!(true),
        rerun_succeeded: rec["rerun_succeeded"] == serde_json::json!(true),
        applied_narrated: all_err.contains("REPAIR_APPLIED "),
        rerun_succeeded_narrated: all_err.contains("REPAIR_RERUN_SUCCEEDED "),
    }
}

/// Seed the fault and run the workload to a failed checkpoint.
///
/// The seeding manifest is the PLAIN one (no `[ai]` table) so the seeding run
/// cannot itself invoke recovery — it just leaves a clean `boom`-failed
/// checkpoint. Callers then overwrite `w.oxoflow` with `poisoned_manifest()`
/// (the identical input every repair surface must start from).
fn seed_failure(dir: &Path) {
    std::fs::write(dir.join("w.oxoflow"), POISONED_RULES).unwrap();
    let out = run(oxo(dir).args(["run", "w.oxoflow", "--no-report-snapshot"]));
    assert!(
        !out.status.success(),
        "the poisoned run must fail:\n{}",
        stderr(&out)
    );
}

/// Leg A: apply the reviewed file through `--apply-patch`.
#[test]
fn reviewed_patch_and_ai_decided_repair_converge() {
    // --- Leg A: --apply-patch of a reviewed, whole-workflow file. ---
    let dir_a = tempfile::tempdir().unwrap();
    seed_failure(dir_a.path());
    std::fs::write(dir_a.path().join("w.oxoflow"), poisoned_manifest()).unwrap();
    std::fs::write(dir_a.path().join("reviewed.oxoflow"), repaired_manifest()).unwrap();
    let out_a = run(oxo(dir_a.path()).args([
        "run",
        "w.oxoflow",
        "--no-report-snapshot",
        "--apply-patch",
        "reviewed.oxoflow",
    ]));
    assert!(
        out_a.status.success(),
        "the reviewed patch must apply and verify:\n{}",
        stderr(&out_a)
    );
    assert!(
        format!("{}{}", stderr(&out_a), stdout(&out_a))
            .contains("REPAIR_APPLIED session_id=reviewed-patch"),
        "the reviewed patch must narrate its stable session label:\n{}",
        stderr(&out_a)
    );
    let a = observe(dir_a.path(), &out_a);

    // --- Leg B: the AI decides and applies the SAME repaired manifest. ---
    // The stub answers with the mandated diagnosis shape: a `## Corrected
    // TOML` section holding a SINGLE ```toml fence with the complete repaired
    // workflow, and `- Safe to auto-apply: yes`. `validate_fix` requires the
    // body to keep the `[ai]` table, so `repaired_manifest()` is reused here
    // verbatim — the exact manifest leg A applied.
    let body = serde_json::json!({
        "message": {
            "role": "assistant",
            "content": format!(
                "## Root Cause\nboom always exits 1.\n\n\
                 ## Proposed Fix\nmake boom write its output instead.\n\n\
                 ## Corrected TOML\n```toml\n{}```\n\n\
                 ## Safety Assessment\n- Safe to auto-apply: yes\n\
                 - Risk level: low\n- Rollback: restore the backup\n",
                repaired_manifest()
            ),
        },
        "done": true,
        "prompt_eval_count": 3,
        "eval_count": 5,
    })
    .to_string();
    let stub = Stub::spawn(body);

    let dir_b = tempfile::tempdir().unwrap();
    // Same fault (and the same `[ai]` section that authorizes auto-apply and
    // routes the provider at the local stub).
    seed_failure(dir_b.path());
    std::fs::write(dir_b.path().join("w.oxoflow"), poisoned_manifest()).unwrap();
    let mut cmd = oxo(dir_b.path());
    cmd.env("OLLAMA_HOST", &stub.url);
    let out_b = run(cmd.args(["run", "w.oxoflow", "--no-report-snapshot"]));
    assert!(
        stub.hits() >= 1,
        "the AI-decided leg must actually consult the provider:\n{}",
        stderr(&out_b)
    );
    assert!(
        out_b.status.success(),
        "the AI-decided repair must apply and verify:\n{}",
        stderr(&out_b)
    );
    let b = observe(dir_b.path(), &out_b);

    // --- Parity: identical repaired workflow + REPAIR_RERUN_* outcome. ---
    assert_eq!(
        a.workflow, b.workflow,
        "both surfaces must converge on the same repaired workflow"
    );
    assert_eq!(
        a.workflow,
        repaired_manifest().trim_end(),
        "the repaired workflow is the reviewed manifest"
    );
    assert_eq!(a.failed_empty, b.failed_empty);
    assert_eq!(a.boom_completed, b.boom_completed);
    assert_eq!(a.repair_applied, b.repair_applied);
    assert_eq!(a.rerun_attempted, b.rerun_attempted);
    assert_eq!(a.rerun_succeeded, b.rerun_succeeded);
    assert_eq!(a.applied_narrated, b.applied_narrated);
    assert_eq!(
        a.rerun_succeeded_narrated, b.rerun_succeeded_narrated,
        "both surfaces must narrate the same REPAIR_RERUN_* contract"
    );

    // Both legs actually verified (not vacuously equal to a no-op).
    assert!(a.rerun_succeeded && a.rerun_attempted, "leg A must verify");
    assert!(b.rerun_succeeded && b.rerun_attempted, "leg B must verify");
    assert!(a.repair_applied && a.boom_completed && a.failed_empty);
}

/// Fail closed: `--apply-patch` combined with an explicit `--ai-recover` is a
/// contradiction (reviewed file vs AI-decided) and must error, never guess.
/// This is the falsifiable guard on the flag's interaction semantics.
#[test]
fn apply_patch_with_ai_recover_fails_closed() {
    let dir = tempfile::tempdir().unwrap();
    let poisoned = poisoned_manifest();
    std::fs::write(dir.path().join("w.oxoflow"), &poisoned).unwrap();
    std::fs::write(dir.path().join("reviewed.oxoflow"), repaired_manifest()).unwrap();
    let out = run(oxo(dir.path()).args([
        "run",
        "w.oxoflow",
        "--apply-patch",
        "reviewed.oxoflow",
        "--ai-recover",
    ]));
    assert!(
        !out.status.success(),
        "the contradictory combo must fail:\n{}",
        stderr(&out)
    );
    assert!(
        stderr(&out).contains("cannot be combined with --ai-recover"),
        "the failure must name the contradiction:\n{}",
        stderr(&out)
    );
    // The workflow must be untouched (the run never started).
    assert_eq!(
        std::fs::read_to_string(dir.path().join("w.oxoflow")).unwrap(),
        poisoned
    );
}
