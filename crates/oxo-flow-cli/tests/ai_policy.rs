//! RB-14 / issue #1122: workflow-scoped AI provider and model precedence,
//! structured degraded-mode reasons, and `template --ai` overwrite
//! semantics.
//!
//! These are end-to-end CLI tests: they spawn the real binary with a
//! scrubbed environment (the operator's shell may export provider keys) and
//! a hermetic `OXO_FLOW_AI_CONFIG`, so what is asserted is the shipped
//! resolution path, not a unit-level mirror of it.
//!
//! The provider calls are served by bounded local stubs: the Ollama backend
//! is keyless, so pointing `OLLAMA_HOST` at a loopback listener lets the
//! analysis complete deterministically without network access. Every stub
//! stops after a deadline, so a regression that stops calling the provider
//! fails the assertion instead of hanging the test.

use std::io::{ErrorKind, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::Path;
use std::process::{Command, Output};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

const OLLAMA_BODY: &str = r#"{"message":{"role":"assistant","content":"[INFO] workflow: looks fine"},"done":true,"prompt_eval_count":3,"eval_count":5}"#;
const CLAUDE_BODY: &str = r#"{"content":[{"type":"text","text":"[INFO] workflow: looks fine"}],"stop_reason":"end_turn","usage":{"input_tokens":3,"output_tokens":5}}"#;
const DEEPSEEK_BODY: &str = r#"{"choices":[{"message":{"role":"assistant","content":"[INFO] workflow: looks fine"},"finish_reason":"stop"}],"usage":{"prompt_tokens":3,"completion_tokens":5}}"#;
/// A generated workflow a scripted provider can return verbatim; the
/// orchestrator extracts it from the fenced block and validates it.
const GENERATED_WORKFLOW_BODY: &str = r#"{"message":{"role":"assistant","content":"```toml\n[workflow]\nname = \"race\"\nversion = \"1.0.0\"\n\n[[rules]]\nname = \"step1\"\noutput = [\"out1.txt\"]\nshell = \"echo done > out1.txt\"\n```"},"done":true}"#;

/// How long a stub keeps listening for a request before exiting on its own.
const STUB_LIFETIME: Duration = Duration::from_secs(15);

/// A bounded loopback HTTP stub: answers every request with a fixed JSON
/// body, records the raw request text, and stops after [`STUB_LIFETIME`]
/// even when nothing arrives (no unbounded blocking accept).
struct Stub {
    url: String,
    requests: Arc<Mutex<Vec<String>>>,
    stop: Arc<AtomicBool>,
    handle: Option<std::thread::JoinHandle<()>>,
}

impl Stub {
    fn spawn(body: &'static str, on_first_request: Option<Box<dyn Fn() + Send + 'static>>) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind stub");
        listener.set_nonblocking(true).expect("nonblocking stub");
        let url = format!("http://{}", listener.local_addr().unwrap());
        let requests: Arc<Mutex<Vec<String>>> = Arc::default();
        let stop = Arc::new(AtomicBool::new(false));
        let thread_requests = requests.clone();
        let thread_stop = stop.clone();
        let handle = std::thread::spawn(move || {
            let deadline = Instant::now() + STUB_LIFETIME;
            while Instant::now() < deadline && !thread_stop.load(Ordering::Relaxed) {
                match listener.accept() {
                    Ok((mut stream, _)) => {
                        let request = read_http_request(&mut stream);
                        let first = match thread_requests.lock() {
                            Ok(mut recorded) => {
                                recorded.push(request);
                                recorded.len() == 1
                            }
                            Err(_) => false,
                        };
                        if first && let Some(hook) = on_first_request.as_ref() {
                            hook();
                        }
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
            requests,
            stop,
            handle: Some(handle),
        }
    }

    fn hits(&self) -> usize {
        self.requests.lock().map(|r| r.len()).unwrap_or(0)
    }

    /// Poll until `n` requests were served (or the timeout expires), so a
    /// slow stub write never turns into a flaky assertion.
    fn wait_for_hits(&self, n: usize, timeout: Duration) -> bool {
        let deadline = Instant::now() + timeout;
        while Instant::now() < deadline {
            if self.hits() >= n {
                return true;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        self.hits() >= n
    }

    /// Poll until ANY recorded request contains `needle` (case-insensitive).
    /// Under parallel test load a first connection can be accepted before
    /// its bytes arrive, so the assertion must look at the whole stream of
    /// served requests rather than assuming request 0 is the interesting
    /// one.
    fn wait_for_request_containing(&self, needle: &str, timeout: Duration) -> bool {
        let needle = needle.to_ascii_lowercase();
        let deadline = Instant::now() + timeout;
        loop {
            let found = self
                .requests
                .lock()
                .map(|requests| {
                    requests
                        .iter()
                        .any(|req| req.to_ascii_lowercase().contains(&needle))
                })
                .unwrap_or(false);
            if found {
                return true;
            }
            if Instant::now() >= deadline {
                return false;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    /// All recorded requests, lower-cased (for negative assertions).
    fn requests_lower(&self) -> Vec<String> {
        self.requests
            .lock()
            .map(|requests| {
                requests
                    .iter()
                    .map(|req| req.to_ascii_lowercase())
                    .collect()
            })
            .unwrap_or_default()
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

/// Read one HTTP request off the socket: headers first, then the declared
/// Content-Length body (bounded by the stream read timeout).
fn read_http_request(stream: &mut TcpStream) -> String {
    stream
        .set_read_timeout(Some(Duration::from_secs(2)))
        .expect("read timeout");
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
    String::from_utf8_lossy(&buf).into_owned()
}

/// A hermetic CLI command: no inherited credentials or config paths.
fn cli(workdir: &Path) -> Command {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_oxo-flow"));
    cmd.env_clear()
        .env("PATH", std::env::var("PATH").unwrap_or_default())
        .env("HOME", workdir)
        .env("NO_COLOR", "1")
        .env("OXO_FLOW_AI_CONFIG", workdir.join("no-saved-ai.json"))
        .env("OXO_FLOW_AI_SESSION_DIR", workdir.join("sessions"))
        .current_dir(workdir);
    cmd
}

const AI_WORKFLOW_BODY: &str = "\n[[rules]]\nname = \"step1\"\noutput = [\"out1.txt\"]\nshell = \"mkdir -p . && echo done > {output[0]}\"\n";

fn run(cmd: &mut Command) -> Output {
    cmd.output().expect("oxo-flow spawns")
}

fn stderr(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).into_owned()
}

/// Workflow `[ai]` provider/model win over the project file, the
/// environment, and the saved global config — the full documented chain in
/// one invocation. The Ollama stub proves the provider actually switched
/// (a stale DeepSeek pick would dial `DEEPSEEK_BASE_URL` instead).
#[test]
fn workflow_provider_and_model_override_project_env_and_global() {
    let dir = tempfile::tempdir().unwrap();
    let stub = Stub::spawn(OLLAMA_BODY, None);

    // Global config file: names yet another model (lowest tier).
    std::fs::write(
        dir.path().join("no-saved-ai.json"),
        r#"{"provider":"deepseek","model":"global-model","api_key":"sk-global"}"#,
    )
    .unwrap();
    // Project `.oxo-flow/ai.toml` (middle tier).
    std::fs::create_dir_all(dir.path().join(".oxo-flow")).unwrap();
    std::fs::write(
        dir.path().join(".oxo-flow").join("ai.toml"),
        "[ai]\nprovider = \"openai\"\nmodel = \"project-model\"\n",
    )
    .unwrap();
    // Workflow (highest config tier).
    let workflow = dir.path().join("w.oxoflow");
    std::fs::write(
        &workflow,
        format!(
            "[workflow]\nname = \"ai-policy\"\nversion = \"1.0.0\"\n\n\
             [ai]\nenabled = true\nprovider = \"ollama\"\nmodel = \"workflow-model\"\n{AI_WORKFLOW_BODY}"
        ),
    )
    .unwrap();

    let output = run(cli(dir.path())
        .args(["validate", workflow.to_str().unwrap()])
        // Environment tier (above global, below project/workflow).
        .env("OXO_FLOW_AI_PROVIDER", "deepseek")
        .env("DEEPSEEK_API_KEY", "sk-env-deepseek")
        .env("DEEPSEEK_BASE_URL", "http://127.0.0.1:1")
        .env("OLLAMA_HOST", &stub.url));
    let err = stderr(&output);

    assert!(
        output.status.success(),
        "validate should succeed against the Ollama stub; stderr:\n{err}"
    );
    assert!(
        err.contains("Model: workflow-model"),
        "the workflow's [ai] model must be applied end to end (RB-14); stderr:\n{err}"
    );
    assert!(
        stub.wait_for_hits(1, Duration::from_secs(10)),
        "the request must go to the workflow-selected Ollama endpoint, not the env's DeepSeek"
    );
}

/// A workflow that enables AI but selects no provider still honors the
/// project file's provider/model over the environment.
#[test]
fn project_ai_config_applies_when_the_workflow_selects_nothing() {
    let dir = tempfile::tempdir().unwrap();
    let stub = Stub::spawn(OLLAMA_BODY, None);
    std::fs::create_dir_all(dir.path().join(".oxo-flow")).unwrap();
    std::fs::write(
        dir.path().join(".oxo-flow").join("ai.toml"),
        "[ai]\nprovider = \"ollama\"\nmodel = \"project-model\"\n",
    )
    .unwrap();
    let workflow = dir.path().join("w.oxoflow");
    std::fs::write(
        &workflow,
        format!(
            "[workflow]\nname = \"ai-policy\"\nversion = \"1.0.0\"\n\n\
             [ai]\nenabled = true\n{AI_WORKFLOW_BODY}"
        ),
    )
    .unwrap();

    let output = run(cli(dir.path())
        .args(["validate", workflow.to_str().unwrap()])
        .env("OXO_FLOW_AI_PROVIDER", "deepseek")
        .env("DEEPSEEK_API_KEY", "sk-env-deepseek")
        .env("DEEPSEEK_BASE_URL", "http://127.0.0.1:1")
        .env("OLLAMA_HOST", &stub.url));
    let err = stderr(&output);
    assert!(
        output.status.success(),
        "validate should succeed against the Ollama stub; stderr:\n{err}"
    );
    assert!(
        err.contains("Model: project-model"),
        "the project file's model must beat the environment; stderr:\n{err}"
    );
    assert!(stub.wait_for_hits(1, Duration::from_secs(10)));
}

/// Enabled AI with unusable credentials prints a structured degraded-mode
/// reason (stable code + actionable suggestion) instead of silently
/// skipping AI.
#[test]
fn enabled_ai_with_unusable_credentials_reports_a_structured_reason() {
    let dir = tempfile::tempdir().unwrap();
    let workflow = dir.path().join("w.oxoflow");
    std::fs::write(
        &workflow,
        format!(
            "[workflow]\nname = \"ai-policy\"\nversion = \"1.0.0\"\n\n\
             [ai]\nenabled = true\n{AI_WORKFLOW_BODY}"
        ),
    )
    .unwrap();

    let output = run(cli(dir.path())
        .args(["validate", workflow.to_str().unwrap()])
        // A provider selected without its credential: claude needs
        // ANTHROPIC_AUTH_TOKEN, which the scrubbed env does not carry.
        .env("OXO_FLOW_AI_PROVIDER", "claude"));
    let err = stderr(&output);

    assert!(
        output.status.success(),
        "the deterministic validation still runs; stderr:\n{err}"
    );
    assert!(
        err.contains("AI degraded [AI_CREDENTIAL_MISSING]"),
        "a machine-readable degraded code must be emitted; stderr:\n{err}"
    );
    assert!(
        err.contains("'claude'") && err.contains("ANTHROPIC_AUTH_TOKEN"),
        "the reason must name the provider and the missing credential variable; stderr:\n{err}"
    );
}

/// `template --ai` refuses an existing destination in a non-interactive
/// session and leaves the reviewed file byte-for-byte untouched.
#[test]
fn template_ai_refuses_to_overwrite_an_existing_destination() {
    let dir = tempfile::tempdir().unwrap();
    let destination = dir.path().join("reviewed.oxoflow");
    std::fs::write(&destination, "# reviewed by a human\n").unwrap();

    let output = run(cli(dir.path()).args([
        "template",
        "rna seq analysis",
        "--ai",
        "-o",
        destination.to_str().unwrap(),
    ]));
    let err = stderr(&output);

    assert!(!output.status.success(), "the overwrite must be refused");
    assert!(err.contains("already exists"), "stderr:\n{err}");
    assert!(
        err.contains("--force"),
        "the refusal must name the consent flag; stderr:\n{err}"
    );
    assert_eq!(
        std::fs::read_to_string(&destination).unwrap(),
        "# reviewed by a human\n",
        "the existing workflow must not be modified"
    );
}

/// `--force` is explicit consent: the overwrite guard passes and the
/// command proceeds to provider resolution (which here fails with the
/// structured disabled pin, proving the guard no longer blocks).
#[test]
fn template_ai_force_passes_the_overwrite_guard() {
    let dir = tempfile::tempdir().unwrap();
    let destination = dir.path().join("reviewed.oxoflow");
    std::fs::write(&destination, "# reviewed by a human\n").unwrap();

    let output = run(cli(dir.path())
        .args([
            "template",
            "rna seq analysis",
            "--ai",
            "--force",
            "-o",
            destination.to_str().unwrap(),
        ])
        .env("OXO_FLOW_AI_PROVIDER", "disabled"));
    let err = stderr(&output);

    assert!(
        !output.status.success(),
        "AI is disabled in this invocation"
    );
    assert!(
        err.contains("AI_DISABLED_BY_ENV"),
        "the structured degraded reason must surface; stderr:\n{err}"
    );
    assert!(
        !err.contains("already exists"),
        "the guard must not fire with --force; stderr:\n{err}"
    );
    assert_eq!(
        std::fs::read_to_string(&destination).unwrap(),
        "# reviewed by a human\n",
        "a failed generation must never modify the destination"
    );
}

/// RB-14 review (HIGH): when the environment selects a different provider
/// than the saved global config, the saved config's endpoint and model
/// belong to the saved provider and must never be reused. Two stubs prove
/// the request (with the env credential) reaches the env origin and the
/// saved origin receives nothing.
#[test]
fn provider_switch_never_sends_the_env_credential_to_the_saved_origin() {
    let dir = tempfile::tempdir().unwrap();
    let saved_origin = Stub::spawn(DEEPSEEK_BODY, None);
    let claude_origin = Stub::spawn(CLAUDE_BODY, None);

    std::fs::write(
        dir.path().join("no-saved-ai.json"),
        format!(
            r#"{{"provider":"deepseek","api_key":"sk-saved-deepseek","api_url":"{}","model":"saved-deepseek-model"}}"#,
            saved_origin.url
        ),
    )
    .unwrap();
    let workflow = dir.path().join("w.oxoflow");
    std::fs::write(
        &workflow,
        format!(
            "[workflow]\nname = \"ai-policy\"\nversion = \"1.0.0\"\n\n\
             [ai]\nenabled = true\n{AI_WORKFLOW_BODY}"
        ),
    )
    .unwrap();

    let output = run(cli(dir.path())
        .args(["validate", workflow.to_str().unwrap()])
        .env("OXO_FLOW_AI_PROVIDER", "claude")
        .env("ANTHROPIC_AUTH_TOKEN", "sk-env-claude")
        .env("ANTHROPIC_BASE_URL", &claude_origin.url));
    let err = stderr(&output);

    assert!(
        output.status.success(),
        "validate should succeed against the Claude stub; stderr:\n{err}"
    );
    assert!(
        claude_origin.wait_for_hits(1, Duration::from_secs(10)),
        "the env-selected Claude origin must receive the request; stderr:\n{err}"
    );
    assert_eq!(
        saved_origin.hits(),
        0,
        "the saved DeepSeek origin must never receive the Claude request (credential-origin leak)"
    );
    assert!(
        claude_origin.wait_for_request_containing("sk-env-claude", Duration::from_secs(10)),
        "the environment credential must be used; requests:\n{:?}",
        claude_origin.requests_lower()
    );
    let requests = claude_origin.requests_lower();
    assert!(
        requests
            .iter()
            .all(|req| !req.contains("saved-deepseek-model")),
        "the saved provider's model must not leak into the Claude request:\n{requests:?}"
    );
    assert!(
        requests
            .iter()
            .all(|req| !req.contains("sk-saved-deepseek")),
        "the saved provider's key must not leak into the Claude request:\n{requests:?}"
    );
}

/// `ai explain` resolves the workflow-scoped provider/model too (RB-14
/// review): the header must name the workflow model.
#[test]
fn ai_explain_uses_the_workflow_provider_and_model() {
    let dir = tempfile::tempdir().unwrap();
    let stub = Stub::spawn(OLLAMA_BODY, None);
    let workflow = dir.path().join("w.oxoflow");
    std::fs::write(
        &workflow,
        format!(
            "[workflow]\nname = \"ai-policy\"\nversion = \"1.0.0\"\n\n\
             [ai]\nenabled = true\nprovider = \"ollama\"\nmodel = \"explain-model\"\n{AI_WORKFLOW_BODY}"
        ),
    )
    .unwrap();

    let output = run(cli(dir.path())
        .args(["ai", "explain", workflow.to_str().unwrap()])
        .env("OXO_FLOW_AI_PROVIDER", "deepseek")
        .env("DEEPSEEK_API_KEY", "sk-env-deepseek")
        .env("DEEPSEEK_BASE_URL", "http://127.0.0.1:1")
        .env("OLLAMA_HOST", &stub.url));
    let out = String::from_utf8_lossy(&output.stdout).into_owned();
    let err = stderr(&output);

    assert!(
        output.status.success(),
        "ai explain degrades rather than failing; stderr:\n{err}"
    );
    assert!(
        out.contains("Model: explain-model"),
        "ai explain must use the workflow's [ai] model; stdout:\n{out}"
    );
    assert!(
        stub.wait_for_hits(1, Duration::from_secs(10)),
        "ai explain must call the workflow-selected Ollama endpoint"
    );
}

/// `ai explain`'s degraded path carries the structured reason.
#[test]
fn ai_explain_reports_a_structured_degraded_reason() {
    let dir = tempfile::tempdir().unwrap();
    let workflow = dir.path().join("w.oxoflow");
    std::fs::write(
        &workflow,
        format!(
            "[workflow]\nname = \"ai-policy\"\nversion = \"1.0.0\"\n\n\
             [ai]\nenabled = true\n{AI_WORKFLOW_BODY}"
        ),
    )
    .unwrap();

    let output = run(cli(dir.path())
        .args(["ai", "explain", workflow.to_str().unwrap()])
        .env("OXO_FLOW_AI_PROVIDER", "claude"));
    let err = stderr(&output);

    assert!(
        output.status.success(),
        "the deterministic skeleton is a first-class output; stderr:\n{err}"
    );
    assert!(
        err.contains("AI degraded [AI_CREDENTIAL_MISSING]"),
        "ai explain must report the structured reason; stderr:\n{err}"
    );
}

/// RB-14 review (LOW, TOCTOU): a destination that appears WHILE the model
/// is generating was never consented to; the pre-write re-check must refuse
/// rather than let the atomic rename clobber it.
#[test]
fn destination_created_while_generating_is_not_clobbered() {
    let dir = tempfile::tempdir().unwrap();
    let destination = dir.path().join("raced.oxoflow");
    let raced = destination.clone();
    let stub = Stub::spawn(
        GENERATED_WORKFLOW_BODY,
        Some(Box::new(move || {
            std::fs::write(&raced, "# created while generating\n").unwrap();
        })),
    );

    let output = run(cli(dir.path())
        .args([
            "template",
            "race workflow",
            "--ai",
            "-o",
            destination.to_str().unwrap(),
        ])
        .env("OXO_FLOW_AI_PROVIDER", "ollama")
        .env("OLLAMA_HOST", &stub.url));
    let err = stderr(&output);

    assert!(
        !output.status.success(),
        "the raced destination must be refused; stderr:\n{err}"
    );
    assert!(
        err.contains("appeared while the workflow was being generated"),
        "the refusal must name the race; stderr:\n{err}"
    );
    assert_eq!(
        std::fs::read_to_string(&destination).unwrap(),
        "# created while generating\n",
        "the file created during generation must be left untouched"
    );
}
