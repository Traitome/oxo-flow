//! End-to-end CLI test for the type-admission repair seam (issue #1265,
//! verification gap filed as issue #1352 F5): a real `oxo-flow run` hits the
//! TYPE-002 admission bail, routes it through the verified-repair machinery
//! with a PROVIDER answering over HTTP, and the full seam control flow is
//! asserted from the outside — eligibility → diagnose → authorize → apply →
//! rebuild → `Cleared`/`NotCleared` → `REPAIR_*` narration → checkpoint
//! record.
//!
//! The provider is a local OpenAI-compatible HTTP mock (`POST
//! /v1/chat/completions`), not the `scripted` test backend: the scripted
//! constructor is compiled out of production builds (`cfg(test)` /
//! `test-scripted` feature only), so a spawned production CLI binary cannot
//! install it. The HTTP mock exercises MORE of the real path — request
//! building, HTTP transport, response parsing — with the same offline,
//! deterministic turn semantics (one fixed scripted reply per call).
//!
//! Each scenario runs a cold run in its own temp dir; no network beyond
//! 127.0.0.1 is touched.

use std::path::Path;
use std::process::Command;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

/// A workflow that fails the BioType admission gate with TYPE-002
/// (bio_sort_order_mismatch): a queryname-sorted producer feeds a
/// coordinate-requiring consumer. The shells are real executables (`echo`)
/// so a CLEARED run actually executes both rules end to end.
const VIOLATING_WORKFLOW: &str = r#"[workflow]
name = "repair-e2e"
version = "1.0.0"

[ai]
enabled = true
auto_fix = "always"

[[rules]]
name = "align"
output = ["aln.bam"]
shell = "echo fake > {output[0]}"
[rules.types]
outputs = { default = "BAM[sort_order=queryname]" }

[[rules]]
name = "pileup"
input = ["aln.bam"]
output = ["pileup.txt"]
shell = "echo piled > {output[0]}"
[rules.types]
inputs = { default = "BAM[sort_order=coordinate]" }
"#;

/// The CLEARED patch: same workflow shape (nothing dropped — `validate_fix`
/// refuses section drops), producer re-declared coordinate-sorted so the
/// type gate passes.
const CLEARING_PATCH_BODY: &str = r#"Root Cause: the producer declares a queryname-sorted BAM, but the consumer requires coordinate order.

Fix: declare the producer output as coordinate-sorted BAM.

Safe to auto-apply: yes

```toml
[workflow]
name = "repair-e2e"
version = "1.0.0"

[ai]
enabled = true
auto_fix = "always"

[[rules]]
name = "align"
output = ["aln.bam"]
shell = "echo fake > {output[0]}"
[rules.types]
outputs = { default = "BAM[sort_order=coordinate]" }

[[rules]]
name = "pileup"
input = ["aln.bam"]
output = ["pileup.txt"]
shell = "echo piled > {output[0]}"
[rules.types]
inputs = { default = "BAM[sort_order=coordinate]" }
```
"#;

/// The NOT-CLEARED patch: parses, survives `validate_fix` (nothing dropped),
/// but does NOT change the declared types — the rebuilt gate still bails.
const NON_FIXING_PATCH_BODY: &str = r#"Root Cause: unclear.

Fix: adjust the pipeline.

Safe to auto-apply: yes

```toml
[workflow]
name = "repair-e2e"
version = "1.0.0"

[ai]
enabled = true
auto_fix = "always"

[[rules]]
name = "align"
output = ["aln.bam"]
shell = "echo still-unsorted > {output[0]}"
[rules.types]
outputs = { default = "BAM[sort_order=queryname]" }

[[rules]]
name = "pileup"
input = ["aln.bam"]
output = ["pileup.txt"]
shell = "echo piled > {output[0]}"
[rules.types]
inputs = { default = "BAM[sort_order=coordinate]" }
```
"#;

/// One OpenAI-compatible chat-completions endpoint that replays `content`
/// for every call (a scripted provider over real HTTP) and records the
/// request bodies it received.
struct MockProvider {
    port: u16,
    requests: Arc<AtomicUsize>,
    bodies: Arc<std::sync::Mutex<Vec<String>>>,
}

impl MockProvider {
    fn spawn(content: &'static str) -> Self {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let requests = Arc::new(AtomicUsize::new(0));
        let bodies: Arc<std::sync::Mutex<Vec<String>>> = Arc::default();
        let counter = Arc::clone(&requests);
        let sink = Arc::clone(&bodies);
        std::thread::spawn(move || {
            use std::io::{Read, Write};
            for stream in listener.incoming().flatten() {
                counter.fetch_add(1, Ordering::SeqCst);
                let mut stream = stream;
                let mut buf = Vec::new();
                let mut chunk = [0u8; 4096];
                let header_end = loop {
                    match stream.read(&mut chunk) {
                        Ok(0) | Err(_) => break None,
                        Ok(n) => buf.extend_from_slice(&chunk[..n]),
                    }
                    if let Some(pos) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
                        break Some(pos);
                    }
                };
                let Some(header_end) = header_end else {
                    continue;
                };
                let headers = String::from_utf8_lossy(&buf[..header_end]).to_ascii_lowercase();
                let content_length: usize = headers
                    .lines()
                    .find_map(|l| l.strip_prefix("content-length:"))
                    .and_then(|v| v.trim().parse().ok())
                    .unwrap_or(0);
                let body_start = header_end + 4;
                while buf.len() < body_start + content_length {
                    match stream.read(&mut chunk) {
                        Ok(0) | Err(_) => break,
                        Ok(n) => buf.extend_from_slice(&chunk[..n]),
                    }
                }
                if let Ok(text) = std::str::from_utf8(&buf[body_start..]) {
                    sink.lock().unwrap().push(text.to_string());
                }
                let body = serde_json::json!({
                    "choices": [{
                        "message": {"role": "assistant", "content": content},
                        "finish_reason": "stop"
                    }],
                    "usage": {"prompt_tokens": 12, "completion_tokens": 34}
                });
                let payload = serde_json::to_string(&body).unwrap();
                let _ = stream.write_all(
                    format!(
                        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\n\
                         Content-Length: {}\r\nConnection: close\r\n\r\n{}",
                        payload.len(),
                        payload
                    )
                    .as_bytes(),
                );
            }
        });
        Self {
            port,
            requests,
            bodies,
        }
    }

    fn base_url(&self) -> String {
        format!("http://127.0.0.1:{}/v1", self.port)
    }

    fn request_count(&self) -> usize {
        self.requests.load(Ordering::SeqCst)
    }

    fn request_bodies(&self) -> Vec<String> {
        self.bodies.lock().unwrap().clone()
    }
}

/// Run `oxo-flow` with the provider env pointed at the mock and the AI
/// config redirected to a per-test file (hermetic: no global
/// `~/.oxo-flow/ai_config.json` can leak a real provider in).
fn run_cli(dir: &Path, mock: &MockProvider, args: &[&str]) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_oxo-flow"))
        .args(args)
        .current_dir(dir)
        .env("OXO_FLOW_AI_PROVIDER", "openai")
        .env("OPENAI_API_KEY", "test-key")
        .env("OPENAI_BASE_URL", mock.base_url())
        .env("OXO_FLOW_AI_CONFIG", dir.join("ai-config-redirect.json"))
        .env("NO_COLOR", "1")
        .output()
        .unwrap()
}

fn checkpoint(dir: &Path) -> serde_json::Value {
    serde_json::from_str(&std::fs::read_to_string(dir.join(".oxo-flow/checkpoint.json")).unwrap())
        .unwrap()
}

/// The CLEARED path: the mock's patch clears the type gate, so the run
/// CONTINUES (both rules execute), the `REPAIR_*` contract narrates, and the
/// checkpoint carries the final repair record.
#[test]
fn admission_abort_repairs_clears_and_completes_the_run() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("wf.oxoflow"), VIOLATING_WORKFLOW).unwrap();
    let mock = MockProvider::spawn(CLEARING_PATCH_BODY);

    let out = run_cli(dir.path(), &mock, &["run", "wf.oxoflow", "--ai-recover"]);
    assert!(
        out.status.success(),
        "the repaired workflow must complete the run: {}",
        String::from_utf8_lossy(&out.stderr)
    );

    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("REPAIR_APPLIED session_id="),
        "the apply contract line must narrate: {stderr}"
    );
    assert!(
        stderr.contains("REPAIR_RERUN_SUCCEEDED patched workflow clears the type-admission gate"),
        "the verification contract line must narrate the cleared gate: {stderr}"
    );
    // The diagnosis was fed the ORIGINAL gate findings: the provider's
    // request body carries the bail marker and the TYPE-002 finding text.
    let sent = mock.request_bodies().join("\n");
    assert!(
        sent.contains("declared biological type contracts are violated"),
        "the diagnosis request must carry the admission bail text: {sent}"
    );
    assert!(
        sent.contains("TYPE-002"),
        "the diagnosis request must carry the TYPE-002 finding: {sent}"
    );

    // The run genuinely continued: both rules executed.
    assert!(dir.path().join("aln.bam").exists(), "align must have run");
    assert!(
        dir.path().join("pileup.txt").exists(),
        "pileup must have run after the cleared gate"
    );

    // The checkpoint carries the FINAL repair record alongside the run's
    // own rule records.
    let ck = checkpoint(dir.path());
    let record = ck
        .get("repair_record")
        .expect("repair_record persisted in the checkpoint");
    assert_eq!(record["applied"], serde_json::json!(true));
    assert_eq!(record["rerun_attempted"], serde_json::json!(true));
    assert_eq!(record["rerun_succeeded"], serde_json::json!(true));
    assert!(
        ck["completed_rules"]
            .as_array()
            .unwrap()
            .iter()
            .any(|r| r == "align"),
        "the run's own completion records must coexist with the repair record"
    );

    // The provider was really dialed (the diagnosis turn) — at least once.
    assert!(
        mock.request_count() >= 1,
        "the repair diagnosis must reach the provider"
    );
}

/// The NOT-CLEARED path: the mock's patch parses and survives `validate_fix`
/// but does not change the declared types, so the rebuilt gate still bails —
/// the run fails with the seam's error, the contract narrates the failed
/// verification, and the record honestly says `rerun_succeeded: false`.
#[test]
fn admission_abort_patch_that_still_fails_the_gate_fails_the_run() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("wf.oxoflow"), VIOLATING_WORKFLOW).unwrap();
    let mock = MockProvider::spawn(NON_FIXING_PATCH_BODY);

    let out = run_cli(dir.path(), &mock, &["run", "wf.oxoflow", "--ai-recover"]);
    assert!(
        !out.status.success(),
        "a patch that does not clear the gate must fail the run"
    );

    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("REPAIR_APPLIED session_id="),
        "the patch was written, so the apply line must narrate: {stderr}"
    );
    assert!(
        stderr.contains("REPAIR_RERUN_FAILED patched workflow still fails the gate"),
        "the verification contract line must narrate the failed rebuild: {stderr}"
    );
    assert!(
        stderr.contains("still fails the type-admission gate"),
        "the caller's error must name the NotCleared outcome: {stderr}"
    );

    let ck = checkpoint(dir.path());
    let record = ck.get("repair_record").expect("repair_record persisted");
    assert_eq!(record["applied"], serde_json::json!(true));
    assert_eq!(record["rerun_attempted"], serde_json::json!(true));
    assert_eq!(record["rerun_succeeded"], serde_json::json!(false));
    assert!(
        ck["completed_rules"].as_array().unwrap().is_empty(),
        "no rule ran — the gate aborted in preflight before and after the patch"
    );
    assert!(
        mock.request_count() >= 1,
        "the repair diagnosis must reach the provider"
    );
}

/// Eligibility off (no `--ai-recover`, no `[ai] enabled`): the seam must not
/// engage at all — the historical fail-fast bail answers byte-for-byte and
/// the provider is NEVER dialed (zero HTTP requests to a listening mock).
#[test]
fn admission_abort_without_recovery_fails_fast_and_never_dials_the_provider() {
    let dir = tempfile::tempdir().unwrap();
    // Strip the [ai] section: eligibility must come from the workflow OR the
    // flag, and this scenario has neither.
    let wf = VIOLATING_WORKFLOW.replace(
        r#"[ai]
enabled = true
auto_fix = "always"

"#,
        "",
    );
    assert!(!wf.contains("[ai]"), "setup: the [ai] section must be gone");
    std::fs::write(dir.path().join("wf.oxoflow"), wf).unwrap();
    let mock = MockProvider::spawn(CLEARING_PATCH_BODY);

    let out = run_cli(dir.path(), &mock, &["run", "wf.oxoflow"]);
    assert!(
        !out.status.success(),
        "the type-gate bail must fail the run"
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("declared biological type contracts are violated")
            && stderr.contains("TYPE-002"),
        "the fail-fast bail must carry the admission marker and the finding: {stderr}"
    );
    assert!(
        !stderr.contains("REPAIR_"),
        "no repair contract line may narrate when recovery is off: {stderr}"
    );
    assert!(
        !dir.path().join(".oxo-flow/checkpoint.json").exists(),
        "the fail-fast path must not write a repair checkpoint"
    );
    assert_eq!(
        mock.request_count(),
        0,
        "recovery off must never dial the provider"
    );
}
