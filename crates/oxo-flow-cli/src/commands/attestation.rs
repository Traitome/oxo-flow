//! Signed portable run attestations (issue #1370).
//!
//! A run already records its evidence in `checkpoint.json`: the workflow
//! bytes digest, the override-aware resolved fingerprint, include checkout
//! SHAs, the resolved config snapshot, per-output SHA-256 checksums and the
//! environment identity map. Those records live in a run-local file; this
//! module binds them into ONE canonical, versioned document that can be
//! handed to another party and, optionally, signed with Ed25519.
//!
//! What the document binds: the workflow identity triple/quadruple, include
//! resolutions, resolved config (values only — sensitive keys are already
//! digests in `config_snapshot`), per-artifact checksums, environment
//! identity, and the completion outcome.
//!
//! What it does NOT bind: biological correctness of any output, the
//! presence/absence of files on a *different* machine, or legal
//! qualification. Verification proves the bytes of this document and its
//! signature; it cannot prove the run happened as recorded.
//!
//! Canonical form: `serde_json::Value` maps are `BTreeMap`-backed (no
//! `preserve_order`), so key order is deterministic; the signature covers
//! the document with the `signature` field removed. Emitting twice yields
//! byte-identical output.

use anyhow::{Context, Result};
use ed25519_dalek::{Signature, Signer, Verifier, VerifyingKey};
use serde_json::{Map, Value, json};
use std::path::{Path, PathBuf};

use super::bundle_signing::load_signing_key;

/// Format tag carried by every attestation document.
pub const ATTESTATION_FORMAT: &str = "oxo-flow.attestation/v1";

const EXIT_OK: i32 = 0;
const EXIT_INVALID_SIGNATURE: i32 = 2;
const EXIT_MISMATCHED_KEY: i32 = 3;
const EXIT_MALFORMED: i32 = 4;

/// Build the canonical (unsigned) attestation for one checkpoint.
///
/// Only recorded values are used; nothing is fabricated. A missing optional
/// record is simply absent from the document.
pub fn build_attestation(checkpoint_path: &Path) -> Result<Value> {
    let text = std::fs::read_to_string(checkpoint_path)
        .with_context(|| format!("failed to read checkpoint {}", checkpoint_path.display()))?;
    let checkpoint: Value = serde_json::from_str(&text).with_context(|| {
        format!(
            "failed to parse checkpoint JSON: {}",
            checkpoint_path.display()
        )
    })?;
    let object = checkpoint.as_object().ok_or_else(|| {
        anyhow::anyhow!(
            "checkpoint {} is not a JSON object",
            checkpoint_path.display()
        )
    })?;

    // Outputs: recorded checksums plus the cleaned-intermediate records,
    // kept separate so a reader can tell "produced and kept" from "produced
    // then cleaned because temporary".
    let outputs = string_map(object.get("checksums"));
    let cleaned_outputs = string_map(object.get("cleaned_checksums"));
    let config = string_map(object.get("config_snapshot"));
    let includes = string_map(object.get("include_resolutions"));
    let environment = string_map(object.get("env_lock_identity"));
    let inputs = input_manifest_map(object.get("input_manifests"));

    let completed = string_array(object.get("completed_rules"));
    let failed = string_array(object.get("failed_rules"));
    let status = if !failed.is_empty() {
        "failed"
    } else if !completed.is_empty() {
        "completed"
    } else {
        "empty"
    };

    let mut workflow = Map::new();
    insert_optional(&mut workflow, "path", object.get("workflow_path"));
    insert_optional(&mut workflow, "git_sha", object.get("workflow_git_sha"));
    if let Some(value) = object.get("workflow_git_dirty") {
        workflow.insert("git_dirty".to_string(), value.clone());
    }
    insert_optional(
        &mut workflow,
        "content_hash",
        object.get("workflow_content_hash"),
    );
    insert_optional(
        &mut workflow,
        "resolved_hash",
        object.get("workflow_resolved_hash"),
    );
    // Issue #1368: bind the canonical metadata-contract digest — the typed
    // schema plus effective rows the run consumed — so a signed attestation
    // covers the cohort table, not only the workflow/config text.
    insert_optional(
        &mut workflow,
        "metadata_digest",
        object.get("metadata_digest"),
    );

    let mut doc = Map::new();
    doc.insert(
        "format".to_string(),
        Value::String(ATTESTATION_FORMAT.to_string()),
    );
    doc.insert(
        "generated_by".to_string(),
        json!({
            "tool": "oxo-flow",
            "engine_version": env!("CARGO_PKG_VERSION"),
        }),
    );
    doc.insert("workflow".to_string(), Value::Object(workflow));
    doc.insert("includes".to_string(), Value::Object(includes));
    doc.insert("config".to_string(), Value::Object(config));
    doc.insert("outputs".to_string(), Value::Object(outputs));
    doc.insert(
        "cleaned_outputs".to_string(),
        Value::Object(cleaned_outputs),
    );
    doc.insert("inputs".to_string(), Value::Object(inputs));
    doc.insert(
        "environment".to_string(),
        json!({ "env_lock_identity": Value::Object(environment) }),
    );
    doc.insert(
        "outcome".to_string(),
        json!({
            "status": status,
            "completed_rules": completed,
            "failed_rules": failed,
        }),
    );
    doc.insert(
        "checkpoint".to_string(),
        json!({
            "format_version": object
                .get("checkpoint_format_version")
                .cloned()
                .unwrap_or(Value::from(0)),
        }),
    );

    Ok(Value::Object(doc))
}

fn insert_optional(map: &mut Map<String, Value>, key: &str, value: Option<&Value>) {
    if let Some(value) = value.filter(|v| !v.is_null()) {
        map.insert(key.to_string(), value.clone());
    }
}

fn string_map(value: Option<&Value>) -> Map<String, Value> {
    match value.and_then(Value::as_object) {
        Some(map) => map.clone(),
        None => Map::new(),
    }
}

/// Flatten the checkpoint's per-rule input manifests into one key-ordered
/// map (`rule:path` → recorded hash, or the size/mtime fallback). Only
/// recorded entries appear; nothing is recomputed here.
fn input_manifest_map(value: Option<&Value>) -> Map<String, Value> {
    let mut out = Map::new();
    let Some(rules) = value.and_then(Value::as_object) else {
        return out;
    };
    for (rule, manifest) in rules {
        let Some(entries) = manifest.as_array() else {
            continue;
        };
        for entry in entries {
            let Some(path) = entry.get("path").and_then(Value::as_str) else {
                continue;
            };
            let mut record = Map::new();
            if let Some(hash) = entry.get("hash").filter(|v| !v.is_null()) {
                record.insert("hash".to_string(), hash.clone());
            }
            if let Some(size) = entry.get("size") {
                record.insert("size".to_string(), size.clone());
            }
            if let Some(mtime) = entry.get("mtime_nanos") {
                record.insert("mtime_nanos".to_string(), mtime.clone());
            }
            out.insert(format!("{rule}:{path}"), Value::Object(record));
        }
    }
    out
}

fn string_array(value: Option<&Value>) -> Vec<Value> {
    value
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter(|item| item.is_string())
                .cloned()
                .collect()
        })
        .unwrap_or_default()
}

/// Canonical signing bytes: the document without its `signature` field.
fn canonical_bytes(document: &Value) -> Result<Vec<u8>> {
    let mut unsigned = document.clone();
    if let Some(object) = unsigned.as_object_mut() {
        object.remove("signature");
    }
    serde_json::to_vec(&unsigned).context("failed to serialize the attestation canonically")
}

/// Sign `document` in place with the seed stored at `key_path`.
fn sign_document(document: &mut Value, key_path: &Path) -> Result<()> {
    let signing_key = load_signing_key(key_path)?;
    let bytes = canonical_bytes(document)?;
    let signature = signing_key.sign(&bytes);
    let mut object = document
        .as_object_mut()
        .ok_or_else(|| anyhow::anyhow!("attestation document is not an object"))?
        .clone();
    object.insert(
        "signature".to_string(),
        json!({
            "alg": "ed25519",
            "public_key": hex::encode(signing_key.verifying_key().to_bytes()),
            "signature": hex::encode(signature.to_bytes()),
        }),
    );
    *document = Value::Object(object);
    Ok(())
}

/// Outcome of verifying one attestation file.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SignatureStatus {
    Valid,
    Invalid,
    MismatchedKey,
    Unsigned,
}

impl SignatureStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Valid => "valid",
            Self::Invalid => "invalid",
            Self::MismatchedKey => "mismatched_key",
            Self::Unsigned => "unsigned",
        }
    }
}

/// Verify the signature of a parsed attestation.
///
/// `expected_key` is a seed file the caller trusts; when given, a document
/// signed by a DIFFERENT key is `MismatchedKey` (a trust failure, distinct
/// from a cryptographically invalid signature).
fn verify_document(document: &Value, expected_key: Option<&Path>) -> Result<SignatureStatus> {
    let Some(signature) = document.get("signature").and_then(Value::as_object) else {
        return Ok(SignatureStatus::Unsigned);
    };
    let alg = signature.get("alg").and_then(Value::as_str).unwrap_or("");
    if alg != "ed25519" {
        anyhow::bail!("unsupported signature algorithm {alg:?}");
    }
    let public_key = signature
        .get("public_key")
        .and_then(Value::as_str)
        .ok_or_else(|| anyhow::anyhow!("signature.public_key is missing"))?;
    let signature_hex = signature
        .get("signature")
        .and_then(Value::as_str)
        .ok_or_else(|| anyhow::anyhow!("signature.signature is missing"))?;

    let document_key =
        decode_hex_array::<32>(public_key).context("invalid signature.public_key")?;
    let signature_bytes =
        decode_hex_array::<64>(signature_hex).context("invalid signature.signature")?;

    if let Some(key_path) = expected_key {
        let trusted = load_signing_key(key_path)?;
        if trusted.verifying_key().to_bytes() != document_key {
            return Ok(SignatureStatus::MismatchedKey);
        }
    }

    let verifying_key =
        VerifyingKey::from_bytes(&document_key).context("invalid ed25519 public key")?;
    let parsed_signature = Signature::from_bytes(&signature_bytes);
    let bytes = canonical_bytes(document)?;
    Ok(match verifying_key.verify(&bytes, &parsed_signature) {
        Ok(()) => SignatureStatus::Valid,
        Err(_) => SignatureStatus::Invalid,
    })
}

/// Accept either a checkpoint file or a run workdir containing one.
fn resolve_checkpoint(path: &Path) -> Result<PathBuf> {
    if path.is_dir() {
        for candidate in [
            path.join("checkpoint.json"),
            path.join(".oxo-flow").join("checkpoint.json"),
        ] {
            if candidate.is_file() {
                return Ok(candidate);
            }
        }
        anyhow::bail!(
            "{} is a directory but contains no checkpoint.json or .oxo-flow/checkpoint.json",
            path.display()
        );
    }
    if !path.is_file() {
        anyhow::bail!("checkpoint {} does not exist", path.display());
    }
    Ok(path.to_path_buf())
}

fn decode_hex_array<const N: usize>(text: &str) -> Result<[u8; N]> {
    let bytes = hex::decode(text.trim()).context("value is not hexadecimal")?;
    let len = bytes.len();
    bytes
        .try_into()
        .map_err(|_| anyhow::anyhow!("expected {N} bytes, got {len}"))
}

/// `oxo-flow provenance attest <CHECKPOINT> [--key] [--sign] [-o]`.
pub fn attest_command(
    checkpoint: PathBuf,
    key: Option<PathBuf>,
    sign: bool,
    output: Option<PathBuf>,
    json: bool,
) -> Result<()> {
    let checkpoint = resolve_checkpoint(&checkpoint)?;
    let mut document = build_attestation(&checkpoint)?;
    if sign {
        let key_path = key.as_deref().ok_or_else(|| {
            anyhow::anyhow!(
                "signing needs an explicit ed25519 seed file: pass --key <path> \
                 (64 hex characters or 32 raw bytes)"
            )
        })?;
        sign_document(&mut document, key_path)?;
    }
    let bytes = serde_json::to_vec_pretty(&document)?;
    if let Some(path) = output.as_deref() {
        std::fs::write(path, &bytes)
            .with_context(|| format!("failed to write attestation to {}", path.display()))?;
    }

    if json {
        println!("{}", serde_json::to_string_pretty(&document)?);
        return Ok(());
    }
    let signed = document.get("signature").is_some();
    eprintln!(
        "{} {}",
        "Attestation".bold().cyan(),
        if signed { "signed" } else { "unsigned" }
    );
    eprintln!("  {} {}", "format:".dimmed(), ATTESTATION_FORMAT);
    if let Some(hash) = document
        .pointer("/workflow/content_hash")
        .and_then(Value::as_str)
    {
        eprintln!("  {} {}", "workflow digest:".dimmed(), hash);
    }
    if let Some(path) = output.as_deref() {
        eprintln!("  {} {}", "written:".dimmed(), path.display());
    }
    if !json {
        println!("{}", String::from_utf8_lossy(&bytes));
    }
    Ok(())
}

/// `oxo-flow provenance attest-verify <FILE> [--key]`. Returns the process
/// exit code: 0 valid/unsigned, 2 invalid signature, 3 mismatched key.
pub fn attest_verify_command(file: PathBuf, key: Option<PathBuf>, json: bool) -> Result<i32> {
    let text = std::fs::read_to_string(&file)
        .with_context(|| format!("failed to read attestation {}", file.display()))?;
    let document: Value = match serde_json::from_str(&text) {
        Ok(value) => value,
        Err(e) => {
            emit_verify(json, &file, None, "malformed", Some(&e.to_string()));
            return Ok(EXIT_MALFORMED);
        }
    };
    let format_ok = document.get("format").and_then(Value::as_str) == Some(ATTESTATION_FORMAT);
    if !format_ok {
        emit_verify(
            json,
            &file,
            None,
            "malformed",
            Some("format tag is missing or unknown"),
        );
        return Ok(EXIT_MALFORMED);
    }
    let status = match verify_document(&document, key.as_deref()) {
        Ok(status) => status,
        Err(e) => {
            emit_verify(
                json,
                &file,
                Some(&document),
                "malformed",
                Some(&e.to_string()),
            );
            return Ok(EXIT_MALFORMED);
        }
    };
    emit_verify(json, &file, Some(&document), status.as_str(), None);
    Ok(match status {
        SignatureStatus::Valid | SignatureStatus::Unsigned => EXIT_OK,
        SignatureStatus::Invalid => EXIT_INVALID_SIGNATURE,
        SignatureStatus::MismatchedKey => EXIT_MISMATCHED_KEY,
    })
}

fn emit_verify(
    json: bool,
    file: &Path,
    document: Option<&Value>,
    status: &str,
    error: Option<&str>,
) {
    if json {
        let mut out = Map::new();
        out.insert(
            "command".to_string(),
            Value::String("provenance attest-verify".to_string()),
        );
        out.insert(
            "file".to_string(),
            Value::String(file.display().to_string()),
        );
        out.insert(
            "signature_status".to_string(),
            Value::String(status.to_string()),
        );
        if let Some(doc) = document {
            for pointer in ["/format", "/workflow/content_hash", "/outcome/status"] {
                if let Some(value) = doc.pointer(pointer) {
                    out.insert(
                        pointer.trim_start_matches('/').replace('/', "_"),
                        value.clone(),
                    );
                }
            }
        }
        if let Some(error) = error {
            out.insert("error".to_string(), Value::String(error.to_string()));
        }
        println!(
            "{}",
            serde_json::to_string_pretty(&Value::Object(out)).unwrap_or_else(|_| "{}".to_string())
        );
        return;
    }
    eprintln!(
        "{} {} — {}",
        "Attestation Verify".bold().cyan(),
        file.display(),
        status
    );
    if let Some(error) = error {
        eprintln!("  {} {error}", "error:".red());
    }
}

use colored::Colorize;

#[cfg(test)]
mod tests {
    use super::*;

    fn checkpoint(dir: &Path) -> PathBuf {
        let path = dir.join("checkpoint.json");
        std::fs::write(
            &path,
            serde_json::to_vec_pretty(&json!({
                "checkpoint_format_version": 1,
                "completed_rules": ["align", "call"],
                "failed_rules": [],
                "benchmarks": {},
                "workflow_path": "wf.oxoflow",
                "workflow_git_sha": "deadbeef",
                "workflow_git_dirty": true,
                "workflow_content_hash": "sha256:aaaa",
                "workflow_resolved_hash": "sha256:bbbb",
                "metadata_digest": "sha256:dddd",
                "include_resolutions": { "repo@v1 mods/x.oxoflow": "cafebabe" },
                "config_snapshot": { "samples_list": "S1,S2", "secret_key": "sha256:cccc" },
                "checksums": { "out/a.txt": "sha256:1111" },
                "cleaned_checksums": { "tmp/t.txt": "sha256:2222" },
                "env_lock_identity": { "align": "pixi|abc" }
            }))
            .unwrap(),
        )
        .unwrap();
        path
    }

    fn key_file(dir: &Path) -> PathBuf {
        let path = dir.join("attest.key");
        std::fs::write(&path, "01".repeat(32)).unwrap();
        path
    }

    #[test]
    fn attestation_is_byte_stable_and_key_ordered() {
        let dir = tempfile::tempdir().unwrap();
        let cp = checkpoint(dir.path());
        let first = serde_json::to_string_pretty(&build_attestation(&cp).unwrap()).unwrap();
        let second = serde_json::to_string_pretty(&build_attestation(&cp).unwrap()).unwrap();
        assert_eq!(first, second, "attestation output must be byte-stable");
        let top: Vec<&str> = first
            .lines()
            .filter(|line| line.starts_with("  \""))
            .map(|line| {
                line.trim()
                    .trim_matches(|c| c == '"' || c == ',' || c == ':')
            })
            .collect();
        let mut sorted = top.clone();
        sorted.sort_unstable();
        assert_eq!(top, sorted, "top-level keys must be sorted: {top:?}");
    }

    #[test]
    fn attestation_binds_the_recorded_evidence_and_never_leaks_raw_secrets() {
        let dir = tempfile::tempdir().unwrap();
        let cp = checkpoint(dir.path());
        let doc = build_attestation(&cp).unwrap();
        assert_eq!(doc["format"], ATTESTATION_FORMAT);
        assert_eq!(doc["workflow"]["content_hash"], "sha256:aaaa");
        assert_eq!(doc["workflow"]["resolved_hash"], "sha256:bbbb");
        assert_eq!(doc["workflow"]["metadata_digest"], "sha256:dddd");
        assert_eq!(doc["includes"]["repo@v1 mods/x.oxoflow"], "cafebabe");
        assert_eq!(doc["outputs"]["out/a.txt"], "sha256:1111");
        assert_eq!(doc["cleaned_outputs"]["tmp/t.txt"], "sha256:2222");
        assert_eq!(doc["environment"]["env_lock_identity"]["align"], "pixi|abc");
        assert_eq!(doc["outcome"]["status"], "completed");
        // The sensitive config value is already a digest in the checkpoint;
        // the attestation carries that digest, never a plaintext secret.
        assert_eq!(doc["config"]["secret_key"], "sha256:cccc");
        assert!(!doc.to_string().contains("plaintext-secret"));
    }

    #[test]
    fn signing_round_trips_and_every_mutated_field_breaks_verification() {
        let dir = tempfile::tempdir().unwrap();
        let cp = checkpoint(dir.path());
        let key = key_file(dir.path());
        let mut doc = build_attestation(&cp).unwrap();
        sign_document(&mut doc, &key).unwrap();
        assert_eq!(
            verify_document(&doc, Some(&key)).unwrap(),
            SignatureStatus::Valid
        );

        // Mutate each bound field directly: artifact and include keys
        // contain `/`, which JSON-pointer syntax would split.
        fn set(doc: &mut Value, section: &str, field: &str) {
            doc[section][field] = Value::String("tampered".to_string());
        }
        for (section, field) in [
            ("workflow", "content_hash"),
            ("workflow", "git_sha"),
            ("workflow", "metadata_digest"),
            ("outputs", "out/a.txt"),
            ("config", "secret_key"),
            ("outcome", "status"),
            ("includes", "repo@v1 mods/x.oxoflow"),
        ] {
            let mut tampered = doc.clone();
            set(&mut tampered, section, field);
            assert_ne!(
                tampered[section][field], doc[section][field],
                "the mutation helper must change {section}.{field}"
            );
            assert_eq!(
                verify_document(&tampered, Some(&key)).unwrap(),
                SignatureStatus::Invalid,
                "mutating {section}.{field} must invalidate the signature"
            );
        }
    }

    #[test]
    fn newline_terminated_hex_key_signs_and_verifies() {
        // The documented `openssl rand -hex 32 > attest.key` form ends in a
        // newline; `provenance attest --sign --key` must accept it
        // (task-7c residual).
        let dir = tempfile::tempdir().unwrap();
        let cp = checkpoint(dir.path());
        let key = dir.path().join("attest.key");
        std::fs::write(&key, format!("{}\n", "01".repeat(32))).unwrap();

        let mut doc = build_attestation(&cp).unwrap();
        sign_document(&mut doc, &key).unwrap();
        assert_eq!(
            verify_document(&doc, Some(&key)).unwrap(),
            SignatureStatus::Valid
        );
    }

    #[test]
    fn mismatched_key_is_distinct_from_an_invalid_signature() {
        let dir = tempfile::tempdir().unwrap();
        let cp = checkpoint(dir.path());
        let key = key_file(dir.path());
        let other = dir.path().join("other.key");
        std::fs::write(&other, "02".repeat(32)).unwrap();

        let mut doc = build_attestation(&cp).unwrap();
        sign_document(&mut doc, &key).unwrap();
        assert_eq!(
            verify_document(&doc, Some(&other)).unwrap(),
            SignatureStatus::MismatchedKey
        );

        let mut broken = doc.clone();
        broken["signature"]["signature"] = Value::String("00".repeat(64));
        assert_eq!(
            verify_document(&broken, Some(&key)).unwrap(),
            SignatureStatus::Invalid
        );
    }

    #[test]
    fn verify_command_maps_every_outcome_to_its_exit_code() {
        let dir = tempfile::tempdir().unwrap();
        let cp = checkpoint(dir.path());
        let key = key_file(dir.path());
        let other = dir.path().join("other.key");
        std::fs::write(&other, "02".repeat(32)).unwrap();

        let write = |name: &str, doc: &Value| {
            let path = dir.path().join(name);
            std::fs::write(&path, serde_json::to_vec(doc).unwrap()).unwrap();
            path
        };

        let mut signed = build_attestation(&cp).unwrap();
        sign_document(&mut signed, &key).unwrap();
        assert_eq!(
            attest_verify_command(write("ok.json", &signed), Some(key.clone()), true).unwrap(),
            0
        );
        assert_eq!(
            attest_verify_command(
                write("unsigned.json", &build_attestation(&cp).unwrap()),
                None,
                true
            )
            .unwrap(),
            0
        );

        let mut tampered = signed.clone();
        tampered["outcome"]["status"] = Value::String("failed".into());
        assert_eq!(
            attest_verify_command(write("tampered.json", &tampered), Some(key.clone()), true)
                .unwrap(),
            2
        );
        assert_eq!(
            attest_verify_command(write("same.json", &signed), Some(other), true).unwrap(),
            3
        );

        let malformed = dir.path().join("bad.json");
        std::fs::write(&malformed, "not json").unwrap();
        assert_eq!(attest_verify_command(malformed, None, true).unwrap(), 4);
    }

    #[test]
    fn unsigned_attestation_reports_unsigned() {
        let dir = tempfile::tempdir().unwrap();
        let cp = checkpoint(dir.path());
        let doc = build_attestation(&cp).unwrap();
        assert_eq!(
            verify_document(&doc, None).unwrap(),
            SignatureStatus::Unsigned
        );
    }
}
