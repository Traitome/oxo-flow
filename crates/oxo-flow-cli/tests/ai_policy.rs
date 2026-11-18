//! RB-14 / issue #1122: workflow-scoped AI provider and model precedence,
//! structured degraded-mode reasons, and `template --ai` overwrite
//! semantics.
//!
//! These are end-to-end CLI tests: they spawn the real binary with a
//! scrubbed environment (the operator's shell may export provider keys) and
//! a hermetic `OXO_FLOW_AI_CONFIG`, so what is asserted is the shipped
//! resolution path, not a unit-level mirror of it.
//!
//! The provider calls are served by a local stub: the Ollama backend is
//! keyless, so pointing `OLLAMA_HOST` at a one-shot TCP listener lets the
//! analysis complete deterministically without network access.

use std::io::{Read, Write};
use std::net::TcpListener;
use std::path::Path;
use std::process::{Command, Output};

/// A one-shot Ollama `/chat` endpoint. Returns the base URL and a join
/// handle yielding how many requests it served.
fn ollama_stub() -> (String, std::thread::JoinHandle<usize>) {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind stub");
    let url = format!("http://{}", listener.local_addr().unwrap());
    let handle = std::thread::spawn(move || {
        let mut served = 0usize;
        // The analysis makes exactly one call; a second accept would block,
        // so stop after the first.
        if let Ok((mut stream, _)) = listener.accept() {
            let mut buf = [0u8; 65536];
            let _ = stream.read(&mut buf);
            let body = r#"{"message":{"role":"assistant","content":"[INFO] workflow: looks fine"},"done":true,"prompt_eval_count":3,"eval_count":5}"#;
            let response = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                body.len(),
                body
            );
            let _ = stream.write_all(response.as_bytes());
            served += 1;
        }
        served
    });
    (url, handle)
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
    let (ollama_url, stub) = ollama_stub();

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
        .env("OLLAMA_HOST", &ollama_url));
    let err = stderr(&output);

    assert!(
        output.status.success(),
        "validate should succeed against the Ollama stub; stderr:\n{err}"
    );
    assert!(
        err.contains("Model: workflow-model"),
        "the workflow's [ai] model must be applied end to end (RB-14); stderr:\n{err}"
    );
    assert_eq!(
        stub.join().unwrap(),
        1,
        "the request must go to the workflow-selected Ollama endpoint, not the env's DeepSeek"
    );
}

/// A workflow that enables AI but selects no provider still honors the
/// project file's provider/model over the environment.
#[test]
fn project_ai_config_applies_when_the_workflow_selects_nothing() {
    let dir = tempfile::tempdir().unwrap();
    let (ollama_url, stub) = ollama_stub();
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
        .env("OLLAMA_HOST", &ollama_url));
    let err = stderr(&output);
    assert!(
        output.status.success(),
        "validate should succeed against the Ollama stub; stderr:\n{err}"
    );
    assert!(
        err.contains("Model: project-model"),
        "the project file's model must beat the environment; stderr:\n{err}"
    );
    assert_eq!(stub.join().unwrap(), 1);
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
