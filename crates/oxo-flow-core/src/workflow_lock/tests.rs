use super::state::is_credential_key;
use super::*;
use crate::config::WorkflowConfig;
use crate::executor::checkpoint::{compute_file_checksum, compute_workflow_resolved_hash};
use std::collections::BTreeMap;
use std::path::PathBuf;

const BASE_WORKFLOW: &str = r#"
[workflow]
name = "lockdemo"
version = "1.0"

[[rules]]
name = "hello"
shell = "echo hi"
"#;

/// Parse a workflow file the way the CLI does before observing it:
/// includes resolved, defaults applied, wildcards expanded.
fn loaded_config(workflow: &Path) -> WorkflowConfig {
    let mut config = WorkflowConfig::from_file(workflow).expect("workflow parses");
    config.apply_defaults();
    config.expand_wildcards().expect("wildcards expand");
    config
}

fn write_workflow(dir: &Path, content: &str) -> PathBuf {
    let path = dir.join("wf.oxoflow");
    std::fs::write(&path, content).expect("workflow written");
    path
}

/// The CLI-equivalent generation pair: (lock, observation).
fn generate(workflow: &Path) -> (WorkflowLock, LockState) {
    let config = loaded_config(workflow);
    let content_hash = compute_file_checksum(workflow).expect("workflow hashes");
    let state = LockState::observe(&config, Some(workflow), &content_hash);
    let lock = WorkflowLock::from_state(&state);
    (lock, state)
}

fn findings_for(lock: &WorkflowLock, workflow: &Path) -> Vec<LockFinding> {
    let config = loaded_config(workflow);
    let content_hash = compute_file_checksum(workflow).expect("workflow hashes");
    let state = LockState::observe(&config, Some(workflow), &content_hash);
    verify_lock(lock, &state).findings
}

fn codes(findings: &[LockFinding]) -> Vec<&'static str> {
    findings.iter().map(|finding| finding.code).collect()
}

fn state_with_rules(environments: &[(&str, &str)]) -> LockState {
    LockState {
        workflow_name: "t".to_string(),
        workflow_version: "1.0".to_string(),
        workflow_content_sha256: "sha256:aa".to_string(),
        workflow_resolved_hash: "sha256:bb".to_string(),
        metadata_digest: None,
        config: BTreeMap::new(),
        includes: BTreeMap::new(),
        rules: environments
            .iter()
            .map(|(rule, environment)| {
                (
                    (*rule).to_string(),
                    LockedRule {
                        environment: (*environment).to_string(),
                        declared_software: BTreeMap::new(),
                    },
                )
            })
            .collect(),
    }
}

/// One hand-built state with repo-style includes: `(key, content digest,
/// resolved sha)`.
fn state_with_includes(entries: &[(&str, &str, Option<&str>)]) -> LockState {
    let mut state = state_with_rules(&[]);
    for (key, content, resolved) in entries {
        state.includes.insert(
            (*key).to_string(),
            LockedInclude {
                source: "repo".to_string(),
                repo: Some("https://example.com/mod.git".to_string()),
                git_ref: Some("main".to_string()),
                resolved_sha: resolved.map(str::to_string),
                content_sha256: (*content).to_string(),
            },
        );
    }
    state
}

// (a) Generation determinism: the same resolved input produces the same
// bytes, twice.
#[test]
fn generation_is_deterministic() {
    let dir = tempfile::tempdir().unwrap();
    let workflow = write_workflow(dir.path(), BASE_WORKFLOW);
    let (first, _) = generate(&workflow);
    let (second, _) = generate(&workflow);
    assert_eq!(
        first.canonical_json(),
        second.canonical_json(),
        "the lock document must be byte-identical across runs"
    );
    assert!(first.digest_matches(), "generation finalizes the digest");
}

// (b) The acceptance test: the same logical workflow in two different
// working directories generates byte-identical lock bytes. Paths in the
// document are workflow-relative — no absolute or host-specific path may
// leak in.
#[test]
fn lock_bytes_are_identical_across_working_directories() {
    let workflow_text = r#"
[workflow]
name = "portable"
version = "1.0"

[[include]]
path = "rules/extra.oxoflow"

[[rules]]
name = "hello"
shell = "echo hi"
[rules.environment]
conda = "envs/env.yaml"
"#;
    let extra = "[workflow]\nname = \"extra\"\nversion = \"1.0\"\n\n[[rules]]\nname = \"extra\"\nshell = \"echo extra\"\n";
    let env = "dependencies:\n  - bwa=0.7.17\n";

    let mut locks = Vec::new();
    for _ in 0..2 {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("rules")).unwrap();
        std::fs::create_dir_all(dir.path().join("envs")).unwrap();
        std::fs::write(dir.path().join("rules/extra.oxoflow"), extra).unwrap();
        std::fs::write(dir.path().join("envs/env.yaml"), env).unwrap();
        let workflow = write_workflow(dir.path(), workflow_text);
        let (lock, _) = generate(&workflow);
        let json = lock.canonical_json();
        assert!(
            !json.contains(&dir.path().display().to_string()),
            "no absolute path may leak into the lock: {json}"
        );
        locks.push(json);
    }
    assert_eq!(
        locks[0], locks[1],
        "the same logical workflow in different cwds must produce identical lock bytes"
    );
}

// (c) Input row order must not leak into the lock: reordering metadata
// rows leaves the metadata digest (and therefore the lock) unchanged.
#[test]
fn lock_bytes_are_identical_across_metadata_row_order() {
    let workflow_text = r#"
[workflow]
name = "meta-lock"
version = "1.0"
metadata_file = "meta.tsv"

[workflow.metadata_schema]
required_columns = ["sample"]

[[rules]]
name = "hello"
shell = "echo hi"
"#;
    let header = "sample\tvalue\n";
    let rows_a = "S1\t10\nS2\t20\n";
    let rows_b = "S2\t20\nS1\t10\n";

    let mut locks = Vec::new();
    for rows in [rows_a, rows_b] {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("meta.tsv"), format!("{header}{rows}")).unwrap();
        let workflow = write_workflow(dir.path(), workflow_text);
        let (lock, _) = generate(&workflow);
        assert!(
            lock.workflow.metadata_digest.is_some(),
            "a metadata contract must be pinned"
        );
        locks.push(lock.canonical_json());
    }
    assert_eq!(
        locks[0], locks[1],
        "metadata row order must not change lock bytes"
    );
}

// (d) Config declaration order must not leak into the lock either.
#[test]
fn config_declaration_order_does_not_change_lock_bytes() {
    let dir = tempfile::tempdir().unwrap();
    let first = write_workflow(
        dir.path(),
        r#"
[workflow]
name = "cfg"
version = "1.0"

[config]
b = 2
a = 1

[[rules]]
name = "hello"
shell = "echo hi"
"#,
    );
    let config_a = loaded_config(&first);
    let text_b = std::fs::read_to_string(&first)
        .unwrap()
        .replace("b = 2\na = 1", "a = 1\nb = 2");
    let second = dir.path().join("wf2.oxoflow");
    std::fs::write(&second, text_b).unwrap();
    let config_b = loaded_config(&second);
    // The two files differ in bytes, so hold the content digest constant
    // to isolate config ordering.
    let state_a = LockState::observe(&config_a, Some(&first), "sha256:same");
    let state_b = LockState::observe(&config_b, Some(&second), "sha256:same");
    assert_eq!(state_a.config, state_b.config);
    assert_eq!(
        WorkflowLock::from_state(&state_a).canonical_json(),
        WorkflowLock::from_state(&state_b).canonical_json(),
        "config declaration order must not change lock bytes"
    );
}

// (e) Canonical on-disk form is key-ordered JSON with a trailing newline.
#[test]
fn canonical_json_is_key_sorted() {
    let dir = tempfile::tempdir().unwrap();
    let workflow = write_workflow(
        dir.path(),
        &BASE_WORKFLOW.replace("[[rules]]", "[config]\nthreshold = 1\n\n[[rules]]"),
    );
    let (lock, _) = generate(&workflow);
    let json = lock.canonical_json();
    assert!(json.ends_with('\n'), "the file ends with a newline");
    let config = json.find("\n  \"config\"").expect("config key");
    let format = json.find("\n  \"format\"").expect("format key");
    let workflow_key = json.find("\n  \"workflow\"").expect("workflow key");
    assert!(
        config < format && format < workflow_key,
        "top-level keys must be sorted: {json}"
    );
}

// (f) Clean verification: a freshly generated lock verifies against the
// same workflow.
#[test]
fn fresh_lock_verifies_clean() {
    let dir = tempfile::tempdir().unwrap();
    let workflow = write_workflow(dir.path(), BASE_WORKFLOW);
    let (lock, state) = generate(&workflow);
    let verification = verify_lock(&lock, &state);
    assert!(
        verification.ok(),
        "a fresh lock must verify: {:?}",
        verification.findings
    );
}

// (g) Workflow content drift.
#[test]
fn workflow_content_change_reports_lock_002() {
    let dir = tempfile::tempdir().unwrap();
    let workflow = write_workflow(dir.path(), BASE_WORKFLOW);
    let (lock, _) = generate(&workflow);
    std::fs::write(&workflow, BASE_WORKFLOW.replace("echo hi", "echo bye")).unwrap();
    let findings = findings_for(&lock, &workflow);
    assert!(codes(&findings).contains(&LOCK_002), "{findings:?}");
    assert!(
        findings
            .iter()
            .any(|finding| finding.subject == "workflow.content_sha256")
    );
}

// (h) Metadata drift: an edited metadata row is LOCK-002 even when the
// workflow file's bytes and config are unchanged.
#[test]
fn metadata_change_reports_lock_002() {
    let workflow_text = r#"
[workflow]
name = "meta-lock"
version = "1.0"
metadata_file = "meta.tsv"

[workflow.metadata_schema]
required_columns = ["sample"]

[[rules]]
name = "hello"
shell = "echo hi"
"#;
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("meta.tsv"), "sample\tvalue\nS1\t10\n").unwrap();
    let workflow = write_workflow(dir.path(), workflow_text);
    let (lock, _) = generate(&workflow);
    std::fs::write(dir.path().join("meta.tsv"), "sample\tvalue\nS1\t11\n").unwrap();
    let findings = findings_for(&lock, &workflow);
    assert!(codes(&findings).contains(&LOCK_002), "{findings:?}");
    assert!(
        findings
            .iter()
            .any(|finding| finding.subject == "workflow.metadata_digest")
    );
}

// (i) Config drift.
#[test]
fn config_change_reports_lock_003() {
    let workflow_text = r#"
[workflow]
name = "cfg"
version = "1.0"

[config]
threshold = 10

[[rules]]
name = "hello"
shell = "echo {config.threshold}"
"#;
    let dir = tempfile::tempdir().unwrap();
    let workflow = write_workflow(dir.path(), workflow_text);
    let (lock, _) = generate(&workflow);
    let changed = workflow_text.replace("threshold = 10", "threshold = 20");
    std::fs::write(&workflow, changed).unwrap();
    let findings = findings_for(&lock, &workflow);
    assert!(codes(&findings).contains(&LOCK_003), "{findings:?}");
    let config_finding = findings
        .iter()
        .find(|finding| finding.subject == "config.threshold")
        .expect("per-key finding");
    assert_eq!(config_finding.expected.as_deref(), Some("10"));
    assert_eq!(config_finding.actual.as_deref(), Some("20"));
}

// (j) Environment spec drift: editing the spec file changes the
// environment identity, with no network or solver involved.
#[test]
fn environment_spec_change_reports_lock_004() {
    let workflow_text = r#"
[workflow]
name = "env-lock"
version = "1.0"

[[rules]]
name = "hello"
shell = "echo hi"
[rules.environment]
conda = "envs/env.yaml"
"#;
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(dir.path().join("envs")).unwrap();
    std::fs::write(dir.path().join("envs/env.yaml"), "bwa=0.7.17\n").unwrap();
    let workflow = write_workflow(dir.path(), workflow_text);
    let (lock, _) = generate(&workflow);
    assert!(
        crate::env_lock::env_lock_is_resolved(&lock.rules["hello"].environment),
        "the conda file fixture must resolve: {:?}",
        lock.rules
    );
    std::fs::write(dir.path().join("envs/env.yaml"), "bwa=0.7.18\n").unwrap();
    let findings = findings_for(&lock, &workflow);
    assert!(codes(&findings).contains(&LOCK_004), "{findings:?}");
}

// (k) Include drift: editing an included module is LOCK-005.
#[test]
fn include_content_change_reports_lock_005() {
    let workflow_text = r#"
[workflow]
name = "inc-lock"
version = "1.0"

[[include]]
path = "rules/extra.oxoflow"

[[rules]]
name = "hello"
shell = "echo hi"
"#;
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(dir.path().join("rules")).unwrap();
    std::fs::write(
        dir.path().join("rules/extra.oxoflow"),
        "[workflow]\nname = \"extra\"\nversion = \"1.0\"\n\n[[rules]]\nname = \"extra\"\nshell = \"echo extra\"\n",
    )
    .unwrap();
    let workflow = write_workflow(dir.path(), workflow_text);
    let (lock, _) = generate(&workflow);
    assert!(lock.includes.contains_key("rules/extra.oxoflow"));
    std::fs::write(
        dir.path().join("rules/extra.oxoflow"),
        "[workflow]\nname = \"extra\"\nversion = \"1.0\"\n\n[[rules]]\nname = \"extra\"\nshell = \"echo changed\"\n",
    )
    .unwrap();
    let findings = findings_for(&lock, &workflow);
    assert!(codes(&findings).contains(&LOCK_005), "{findings:?}");
    assert!(
        findings
            .iter()
            .any(|finding| finding.subject == "rules/extra.oxoflow.content_sha256")
    );
}

// (l) A rule that disappears from the include graph is LOCK-005.
#[test]
fn include_removed_reports_lock_005() {
    let workflow_text = r#"
[workflow]
name = "inc-lock"
version = "1.0"

[[include]]
path = "rules/extra.oxoflow"

[[rules]]
name = "hello"
shell = "echo hi"
"#;
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(dir.path().join("rules")).unwrap();
    std::fs::write(
        dir.path().join("rules/extra.oxoflow"),
        "[workflow]\nname = \"extra\"\nversion = \"1.0\"\n\n[[rules]]\nname = \"extra\"\nshell = \"echo extra\"\n",
    )
    .unwrap();
    let workflow = write_workflow(dir.path(), workflow_text);
    let (lock, _) = generate(&workflow);
    let removed = workflow_text.replace("[[include]]\npath = \"rules/extra.oxoflow\"\n", "");
    std::fs::write(&workflow, removed).unwrap();
    let findings = findings_for(&lock, &workflow);
    assert!(codes(&findings).contains(&LOCK_005), "{findings:?}");
}

// (m) Unknown is never a match: an unresolved environment identity does
// not verify even against itself.
#[test]
fn unresolved_environment_never_matches_itself() {
    let state = state_with_rules(&[("align", "docker|unresolved")]);
    let lock = WorkflowLock::from_state(&state);
    let findings = verify_lock(&lock, &state).findings;
    assert_eq!(codes(&findings), vec![LOCK_006], "{findings:?}");
}

// (n) `unavailable` compares equal only to `unavailable`.
#[test]
fn unavailable_matches_only_unavailable() {
    let state = state_with_rules(&[("plain", "system|unavailable")]);
    let lock = WorkflowLock::from_state(&state);
    assert!(verify_lock(&lock, &state).ok());

    let changed = state_with_rules(&[("plain", "docker|repo@sha256:abc")]);
    let findings = verify_lock(&lock, &changed).findings;
    assert_eq!(codes(&findings), vec![LOCK_004], "{findings:?}");
}

// (o) A tampered lock document is LOCK-007, and the field-level drift it
// introduced is still reported.
#[test]
fn tampered_lock_digest_reports_lock_007() {
    let state = state_with_rules(&[("plain", "system|unavailable")]);
    let mut lock = WorkflowLock::from_state(&state);
    lock.lock_digest = "sha256:0000".to_string();
    let findings = verify_lock(&lock, &state).findings;
    assert!(codes(&findings).contains(&LOCK_007), "{findings:?}");
}

// (p) Envelope errors: malformed JSON and unsupported format tags.
#[test]
fn malformed_and_unsupported_locks_are_rejected() {
    assert!(matches!(
        WorkflowLock::parse("{not json"),
        Err(LockParseError::Malformed(_))
    ));
    let state = state_with_rules(&[("plain", "system|unavailable")]);
    let lock = WorkflowLock::from_state(&state);
    let renamed = lock
        .canonical_json()
        .replace(LOCK_FORMAT, "oxo-flow.lock/v99");
    assert!(matches!(
        WorkflowLock::parse(&renamed),
        Err(LockParseError::UnsupportedFormat { .. })
    ));
}

// (q) Round trip: the canonical on-disk bytes parse back to an equal
// document whose digest still matches.
#[test]
fn canonical_json_roundtrips() {
    let dir = tempfile::tempdir().unwrap();
    let workflow = write_workflow(dir.path(), BASE_WORKFLOW);
    let (lock, _) = generate(&workflow);
    let parsed = WorkflowLock::parse(&lock.canonical_json()).unwrap();
    assert_eq!(parsed, lock);
    assert!(parsed.digest_matches());
}

// (r) Include sources are recorded workflow-relative with `/`
// separators, and declared software is pinned.
#[test]
fn include_sources_and_declared_software_are_recorded() {
    let workflow_text = r#"
[workflow]
name = "record"
version = "1.0"

[[include]]
path = "rules/extra.oxoflow"

[[rules]]
name = "hello"
shell = "echo hi"
[rules.environment]
conda = "envs/env.yaml"
modules = ["gcc/11.2"]
"#;
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(dir.path().join("rules")).unwrap();
    std::fs::create_dir_all(dir.path().join("envs")).unwrap();
    std::fs::write(
        dir.path().join("rules/extra.oxoflow"),
        "[workflow]\nname = \"extra\"\nversion = \"1.0\"\n\n[[rules]]\nname = \"extra\"\nshell = \"echo extra\"\n",
    )
    .unwrap();
    std::fs::write(dir.path().join("envs/env.yaml"), "bwa=0.7.17\n").unwrap();
    let workflow = write_workflow(dir.path(), workflow_text);
    let config = loaded_config(&workflow);
    assert_eq!(config.include_sources.len(), 1);
    assert_eq!(config.include_sources[0].path, "rules/extra.oxoflow");
    assert!(
        config.include_sources[0]
            .content_sha256
            .starts_with("sha256:")
    );

    let (lock, _) = generate(&workflow);
    let rule = &lock.rules["hello"];
    assert_eq!(
        rule.declared_software.get("conda_file").map(String::as_str),
        Some("envs/env.yaml")
    );
    assert_eq!(
        rule.declared_software.get("modules").map(String::as_str),
        Some("gcc/11.2")
    );
}

// (s) The resolved-hash fold is additive: no lock digest keeps the
// legacy value, and a lock digest changes the identity.
#[test]
fn resolved_hash_fold_is_additive() {
    let snapshot: BTreeMap<String, String> = BTreeMap::from([("k".to_string(), "v".to_string())]);
    let legacy = compute_workflow_resolved_hash("sha256:abc", &snapshot, None);
    assert_eq!(
        legacy,
        crate::executor::checkpoint::compute_workflow_resolved_hash_with_lock(
            "sha256:abc",
            &snapshot,
            None,
            None
        ),
        "the three-argument form must stay byte-identical"
    );
    let locked = crate::executor::checkpoint::compute_workflow_resolved_hash_with_lock(
        "sha256:abc",
        &snapshot,
        None,
        Some("sha256:lock"),
    );
    assert_ne!(legacy, locked);
}

// (t) The checkpoint's lock_digest is additive: absent by default,
// round-trips when set.
#[test]
fn checkpoint_lock_digest_is_legacy_safe() {
    let mut state = crate::executor::CheckpointState::new();
    let json = serde_json::to_string(&state).unwrap();
    assert!(
        !json.contains("lock_digest"),
        "a legacy checkpoint must not grow a lock field: {json}"
    );
    state.set_lock_digest(Some("sha256:abc".to_string()));
    let json = serde_json::to_string(&state).unwrap();
    assert!(json.contains("lock_digest"));
    let loaded: crate::executor::CheckpointState = serde_json::from_str(&json).unwrap();
    assert_eq!(loaded.lock_digest.as_deref(), Some("sha256:abc"));
    // A legacy checkpoint without the field loads with None.
    let legacy: crate::executor::CheckpointState =
        serde_json::from_str(r#"{"completed_rules":[],"failed_rules":[],"benchmarks":{}}"#)
            .unwrap();
    assert!(legacy.lock_digest.is_none());
}

// (u) Review I4: an unresolved include digest never matches, including
// against itself.
#[test]
fn unresolved_include_never_matches_itself() {
    let state = state_with_includes(&[("mod.oxoflow", "unresolved", None)]);
    let lock = WorkflowLock::from_state(&state);
    let findings = verify_lock(&lock, &state).findings;
    assert_eq!(codes(&findings), vec![LOCK_005], "{findings:?}");
    assert!(findings[0].message.contains("unknown never matches"));
}

// (v) Review M5: a moved repo pin names the differing field, not just the
// (identical) content digest.
#[test]
fn include_field_change_names_the_changed_field() {
    let locked = state_with_includes(&[("mod.oxoflow", "sha256:aa", Some("sha1"))]);
    let lock = WorkflowLock::from_state(&locked);
    let current = state_with_includes(&[("mod.oxoflow", "sha256:aa", Some("sha2"))]);
    let findings = verify_lock(&lock, &current).findings;
    assert_eq!(codes(&findings), vec![LOCK_005], "{findings:?}");
    assert_eq!(findings[0].subject, "mod.oxoflow.resolved_sha");
    assert_eq!(findings[0].expected.as_deref(), Some("sha1"));
    assert_eq!(findings[0].actual.as_deref(), Some("sha2"));
}

// (w) Review M2: unknown fields are malformed documents, never silently
// ignored outside the digest.
#[test]
fn unknown_lock_fields_are_rejected() {
    let state = state_with_rules(&[("plain", "system|unavailable")]);
    let lock = WorkflowLock::from_state(&state);
    let base: serde_json::Value = serde_json::from_str(&lock.canonical_json()).unwrap();

    let mut top = base.clone();
    top["injected_unknown_field"] = serde_json::json!({"x": 1});
    assert!(matches!(
        WorkflowLock::parse(&serde_json::to_string(&top).unwrap()),
        Err(LockParseError::Malformed(_))
    ));

    let mut nested = base;
    nested["workflow"]["injected"] = serde_json::json!(true);
    assert!(matches!(
        WorkflowLock::parse(&serde_json::to_string(&nested).unwrap()),
        Err(LockParseError::Malformed(_))
    ));
}

// (x) Review I3: a credential-shaped config key is stored as a digest even
// when it was never declared `sensitive`, while ordinary keys stay verbatim;
// verification still compares correctly.
#[test]
fn credential_shaped_config_keys_are_hashed() {
    let workflow_text = r#"
[workflow]
name = "secrets"
version = "1.0"

[config]
API_TOKEN = "supersecret-value"
dbPassword = "hunter2"
apiKey = "camel-secret"
hockey = "sticks"

[[rules]]
name = "hello"
shell = "echo hi"
"#;
    let dir = tempfile::tempdir().unwrap();
    let workflow = write_workflow(dir.path(), workflow_text);
    let (lock, state) = generate(&workflow);
    let json = lock.canonical_json();
    assert!(
        !json.contains("supersecret-value")
            && !json.contains("hunter2")
            && !json.contains("camel-secret"),
        "credential-shaped values must not land in plaintext: {json}"
    );
    assert!(lock.config["API_TOKEN"].starts_with("sha256:"));
    assert!(lock.config["dbPassword"].starts_with("sha256:"));
    assert!(
        lock.config["apiKey"].starts_with("sha256:"),
        "camel-case apiKey must be hashed (review R2-1)"
    );
    assert_eq!(lock.config["hockey"], "sticks");
    assert!(verify_lock(&lock, &state).ok());

    // A different token is drift, reported as digests (never plaintext).
    let changed = workflow_text.replace("supersecret-value", "rotated-value");
    std::fs::write(&workflow, changed).unwrap();
    let findings = findings_for(&lock, &workflow);
    assert!(codes(&findings).contains(&LOCK_003), "{findings:?}");
    let finding = findings
        .iter()
        .find(|finding| finding.subject == "config.API_TOKEN")
        .expect("per-key finding");
    assert!(
        !finding
            .actual
            .as_deref()
            .unwrap_or_default()
            .contains("rotated-value"),
        "drift diagnostics must not leak the value: {finding:?}"
    );
}

// (y) A current rule with no lock entry is LOCK-004 (the "rule not pinned"
// arm of the matrix).
#[test]
fn rule_missing_from_lock_reports_lock_004() {
    let lock = WorkflowLock::from_state(&state_with_rules(&[]));
    let current = state_with_rules(&[("new_rule", "system|unavailable")]);
    let findings = verify_lock(&lock, &current).findings;
    assert_eq!(codes(&findings), vec![LOCK_004], "{findings:?}");
    assert_eq!(findings[0].subject, "new_rule");
    assert!(findings[0].message.contains("not pinned"));
}

#[test]
fn credential_key_heuristic_is_conservative() {
    for key in [
        "API_TOKEN",
        "apiToken",
        "apiKey",
        "myApiKey",
        "APIKey",
        "apikey",
        "api_key",
        "API-KEY",
        "pwd",
        "dbPassword",
        "db_password",
        "aws_secret",
        "mySecret",
        "github_credentials",
    ] {
        assert!(is_credential_key(key), "{key} should be hashed");
    }
    for key in [
        "hockey",
        "monkey",
        "threshold",
        "tokenize_mode",
        "secretary",
    ] {
        assert!(!is_credential_key(key), "{key} should stay verbatim");
    }
}
