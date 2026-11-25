//! `oxo-flow audit verify <EXPORT>` — offline verification of an exported
//! audit chain (issue #1372).
//!
//! The command never touches a database: it reads the portable
//! `oxo-flow.audit-chain/v1` document produced by
//! `GET /api/audit/chain/export`, recomputes every hash, checks the links and
//! the head, and compares the embedded `audit_logs` snapshot against the chain
//! records. Tampering is a finding (`ok:false`, exit 2), not a crash.
//!
//! Exit codes: 0 valid, 2 invalid/tampered, 4 malformed or unreadable export.

use anyhow::Result;
use colored::Colorize;
use serde_json::Value;
use std::path::{Path, PathBuf};

use oxo_flow_core::audit_chain::{
    AUDIT_CHAIN_FORMAT, AuditChainExport, AuditChainVerification, verify_export,
};

const EXIT_VALID: i32 = 0;
const EXIT_INVALID: i32 = 2;
const EXIT_MALFORMED: i32 = 4;

/// Named error code for an unreadable or structurally invalid export file.
pub const AUDIT_EXPORT_INVALID: &str = "AUDIT_EXPORT_INVALID";

/// `oxo-flow audit verify <EXPORT> [--json]`. Returns the process exit code.
pub fn audit_verify_command(file: PathBuf, json: bool) -> Result<i32> {
    let text = match std::fs::read_to_string(&file) {
        Ok(text) => text,
        Err(error) => {
            emit_malformed(json, &file, &format!("cannot read export: {error}"));
            return Ok(EXIT_MALFORMED);
        }
    };
    let export: AuditChainExport = match serde_json::from_str(&text) {
        Ok(export) => export,
        Err(error) => {
            emit_malformed(
                json,
                &file,
                &format!("not a valid audit-chain export: {error}"),
            );
            return Ok(EXIT_MALFORMED);
        }
    };
    if export.version != AUDIT_CHAIN_FORMAT {
        emit_malformed(
            json,
            &file,
            &format!(
                "unsupported format '{}' (expected '{AUDIT_CHAIN_FORMAT}')",
                export.version
            ),
        );
        return Ok(EXIT_MALFORMED);
    }

    let verification = verify_export(&export);
    emit_verification(json, &file, &verification);
    Ok(if verification.ok {
        EXIT_VALID
    } else {
        EXIT_INVALID
    })
}

/// Emit the malformed-input document (JSON) or message (human).
fn emit_malformed(json: bool, file: &Path, message: &str) {
    if json {
        let document = serde_json::json!({
            "command": "audit verify",
            "file": file.display().to_string(),
            "ok": false,
            "error": { "code": AUDIT_EXPORT_INVALID, "message": message },
        });
        println!(
            "{}",
            serde_json::to_string_pretty(&document).expect("audit verify JSON document")
        );
        return;
    }
    eprintln!(
        "{} {message}",
        format!("audit export error ({AUDIT_EXPORT_INVALID}):")
            .red()
            .bold()
    );
}

/// Emit the verification document (JSON) or human report.
fn emit_verification(json: bool, file: &Path, verification: &AuditChainVerification) {
    if json {
        let mut document = verification.to_canonical_value();
        if let Some(map) = document.as_object_mut() {
            map.insert("command".into(), Value::String("audit verify".to_string()));
            map.insert("file".into(), Value::String(file.display().to_string()));
            map.insert(
                "format".into(),
                Value::String(AUDIT_CHAIN_FORMAT.to_string()),
            );
        }
        println!(
            "{}",
            serde_json::to_string_pretty(&document).expect("audit verify JSON document")
        );
        return;
    }

    if verification.ok {
        println!(
            "{} {} event(s), head seq {} — all hashes and audit rows agree",
            "Audit chain valid:".green().bold(),
            verification.events,
            verification.head.seq
        );
        return;
    }

    println!(
        "{} {} finding(s) across {} event(s), head seq {}",
        "Audit chain INVALID:".red().bold(),
        verification.findings.len(),
        verification.events,
        verification.head.seq
    );
    for finding in &verification.findings {
        match finding.seq {
            Some(seq) => println!(
                "  {} {} {}",
                format!("[{}]", finding.code).red(),
                format!("seq {seq}:").dimmed(),
                finding.message
            ),
            None => println!(
                "  {} {}",
                format!("[{}]", finding.code).red(),
                finding.message
            ),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use oxo_flow_core::audit_chain::{
        AuditChainAppend, AuditChainHead, AuditChainLogRecord, GENESIS_HASH, PHASE_PENDING,
        PHASE_SUCCESS, build_event,
    };

    /// One pending + one success event plus the matching log row.
    fn valid_export() -> AuditChainExport {
        let timestamp = "2026-01-01T00:00:00.000000Z";
        let pending = build_event(AuditChainAppend {
            seq: 1,
            prev_hash: GENESIS_HASH,
            audit_id: "audit-1",
            phase: PHASE_PENDING,
            user_id: "u1",
            action: "POST /api/runs",
            target: "/api/runs",
            result: PHASE_PENDING,
            metadata: None,
            timestamp,
        });
        let success = build_event(AuditChainAppend {
            seq: 2,
            prev_hash: &pending.hash,
            audit_id: "audit-1",
            phase: PHASE_SUCCESS,
            user_id: "u1",
            action: "POST /api/runs",
            target: "/api/runs",
            result: PHASE_SUCCESS,
            metadata: Some(r#"{"status":200}"#),
            timestamp,
        });
        AuditChainExport::new(
            AuditChainHead {
                seq: 2,
                hash: success.hash.clone(),
            },
            vec![pending, success],
            vec![AuditChainLogRecord::from_columns(
                "audit-1",
                "u1",
                "POST /api/runs",
                "/api/runs",
                PHASE_SUCCESS,
                Some(r#"{"status":200}"#),
                timestamp,
            )],
        )
    }

    fn write(dir: &Path, name: &str, bytes: &[u8]) -> PathBuf {
        let path = dir.join(name);
        std::fs::write(&path, bytes).unwrap();
        path
    }

    #[test]
    fn valid_export_exits_zero() {
        let dir = tempfile::tempdir().unwrap();
        let path = write(
            dir.path(),
            "valid.json",
            &valid_export().to_canonical_bytes(),
        );
        assert_eq!(audit_verify_command(path, true).unwrap(), EXIT_VALID);
    }

    #[test]
    fn tampered_export_exits_two() {
        let dir = tempfile::tempdir().unwrap();
        let mut export = valid_export();
        export.events[1].target = "/api/elsewhere".into();
        let path = write(dir.path(), "tampered.json", &export.to_canonical_bytes());
        assert_eq!(audit_verify_command(path, true).unwrap(), EXIT_INVALID);
    }

    #[test]
    fn malformed_export_exits_four() {
        let dir = tempfile::tempdir().unwrap();
        let path = write(dir.path(), "malformed.json", b"{not json");
        assert_eq!(audit_verify_command(path, true).unwrap(), EXIT_MALFORMED);

        let mut export = valid_export();
        export.version = "oxo-flow.audit-chain/v999".into();
        let path = write(dir.path(), "future.json", &export.to_canonical_bytes());
        assert_eq!(audit_verify_command(path, true).unwrap(), EXIT_MALFORMED);
    }

    #[test]
    fn unreadable_export_exits_four() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("missing.json");
        assert_eq!(audit_verify_command(path, true).unwrap(), EXIT_MALFORMED);
    }
}
