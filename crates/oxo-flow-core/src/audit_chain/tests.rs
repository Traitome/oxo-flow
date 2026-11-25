//! Unit and tamper-matrix tests for the audit chain (issue #1372).

use super::*;

fn timestamp(offset: u64) -> String {
    canonical_timestamp(
        chrono::DateTime::from_timestamp(1_700_000_000 + offset as i64, 123_456_000).unwrap(),
    )
}

/// Build a valid chain of `count` events: pending + success pairs.
fn sample_export(count: usize) -> AuditChainExport {
    let mut events = Vec::new();
    let mut prev = GENESIS_HASH.to_string();
    let mut logs = Vec::new();
    for index in 0..count {
        let audit_id = format!("audit-{index}");
        let pending = build_event(AuditChainAppend {
            seq: events.len() as u64 + 1,
            prev_hash: &prev,
            audit_id: &audit_id,
            phase: PHASE_PENDING,
            user_id: "user-1",
            action: "POST /api/runs",
            target: "/api/runs",
            result: PHASE_PENDING,
            metadata: None,
            timestamp: &timestamp(index as u64 * 2),
        });
        prev = pending.hash.clone();
        events.push(pending);
        let success = build_event(AuditChainAppend {
            seq: events.len() as u64 + 1,
            prev_hash: &prev,
            audit_id: &audit_id,
            phase: PHASE_SUCCESS,
            user_id: "user-1",
            action: "POST /api/runs",
            target: "/api/runs",
            result: PHASE_SUCCESS,
            metadata: Some(r#"{"status":200}"#),
            timestamp: &timestamp(index as u64 * 2),
        });
        prev = success.hash.clone();
        events.push(success);
        logs.push(AuditChainLogRecord::from_columns(
            &audit_id,
            "user-1",
            "POST /api/runs",
            "/api/runs",
            PHASE_SUCCESS,
            Some(r#"{"status":200}"#),
            &timestamp(index as u64 * 2),
        ));
    }
    AuditChainExport::new(
        AuditChainHead {
            seq: events.len() as u64,
            hash: prev,
        },
        events,
        logs,
    )
}

fn finding_codes(verification: &AuditChainVerification) -> Vec<&str> {
    verification
        .findings
        .iter()
        .map(|finding| finding.code.as_str())
        .collect()
}

#[test]
fn canonical_timestamp_has_fixed_microsecond_width() {
    let value = canonical_timestamp(chrono::DateTime::from_timestamp(1_700_000_000, 1).unwrap());
    assert_eq!(value, "2023-11-14T22:13:20.000000Z");
    assert_eq!(value.len(), 27);
    assert!(value.ends_with('Z'));
}

#[test]
fn canonical_metadata_is_key_ordered_and_total() {
    assert_eq!(
        canonical_metadata_text(Some(r#"{ "zz": 1, "aa": 2 }"#)).as_deref(),
        Some(r#"{"aa":2,"zz":1}"#)
    );
    assert_eq!(canonical_metadata_text(None), None);
    // Invalid JSON is preserved as an opaque string, never an error.
    assert_eq!(
        canonical_metadata_text(Some("not json")).as_deref(),
        Some("\"not json\"")
    );
}

#[test]
fn event_hash_ignores_metadata_key_order_and_whitespace() {
    let base = AuditChainAppend {
        seq: 1,
        prev_hash: GENESIS_HASH,
        audit_id: "a",
        phase: PHASE_SUCCESS,
        user_id: "u",
        action: "POST /x",
        target: "/x",
        result: PHASE_SUCCESS,
        metadata: Some(r#"{"b":2,"a":1}"#),
        timestamp: "2026-01-01T00:00:00.000000Z",
    };
    let reordered = AuditChainAppend {
        metadata: Some(r#"{ "a": 1, "b": 2 }"#),
        ..base
    };
    assert_eq!(
        build_event(base).hash,
        build_event(reordered).hash,
        "canonical metadata means key order cannot change the hash"
    );
}

#[test]
fn a_valid_export_verifies_clean() {
    let export = sample_export(3);
    let verification = verify_export(&export);
    assert!(verification.ok, "{:?}", verification.findings);
    assert_eq!(verification.events, 6);
    assert_eq!(verification.head, export.head);
    assert!(finding_codes(&verification).is_empty());
}

#[test]
fn an_empty_genesis_export_verifies_clean() {
    let export = AuditChainExport::new(AuditChainHead::genesis(), Vec::new(), Vec::new());
    let verification = verify_export(&export);
    assert!(verification.ok, "{:?}", verification.findings);
    assert_eq!(verification.events, 0);
    assert_eq!(verification.head, AuditChainHead::genesis());
}

#[test]
fn deleting_an_event_is_detected_by_seq_and_prev_hash() {
    let mut export = sample_export(2);
    // Remove the second pair's pending event: the following success event
    // lands at position 3 with seq 4 and no longer links to its
    // predecessor.
    export.events.remove(2);
    let verification = verify_export(&export);
    assert!(!verification.ok);
    let codes = finding_codes(&verification);
    assert!(codes.contains(&FINDING_SEQ_MISMATCH), "{codes:?}");
    assert!(codes.contains(&FINDING_PREV_HASH_MISMATCH), "{codes:?}");
}

#[test]
fn reordering_events_is_detected() {
    let mut export = sample_export(2);
    export.events.swap(1, 2);
    let verification = verify_export(&export);
    assert!(!verification.ok);
    let codes = finding_codes(&verification);
    assert!(codes.contains(&FINDING_SEQ_MISMATCH), "{codes:?}");
    assert!(codes.contains(&FINDING_PREV_HASH_MISMATCH), "{codes:?}");
}

#[test]
fn inserting_an_event_is_detected() {
    let mut export = sample_export(2);
    let mut inserted = export.events[1].clone();
    inserted.seq = 2;
    export.events.insert(1, inserted);
    let verification = verify_export(&export);
    assert!(!verification.ok);
    let codes = finding_codes(&verification);
    assert!(codes.contains(&FINDING_DUPLICATE_PHASE), "{codes:?}");
    assert!(codes.contains(&FINDING_PREV_HASH_MISMATCH), "{codes:?}");
}

#[test]
fn mutating_an_event_is_detected_by_hash() {
    let mut export = sample_export(1);
    export.events[1].target = "/api/somewhere-else".to_string();
    let verification = verify_export(&export);
    assert!(!verification.ok);
    assert!(
        finding_codes(&verification).contains(&FINDING_HASH_MISMATCH),
        "{:?}",
        verification.findings
    );
}

#[test]
fn truncating_the_chain_is_detected_by_head() {
    let mut export = sample_export(2);
    export.events.pop();
    let verification = verify_export(&export);
    assert!(!verification.ok);
    assert!(
        finding_codes(&verification).contains(&FINDING_HEAD_MISMATCH),
        "{:?}",
        verification.findings
    );
}

#[test]
fn a_bad_genesis_link_is_detected() {
    let mut export = sample_export(1);
    export.events[0].prev_hash = "f".repeat(64);
    // Recompute so only the genesis link (not the hash) is wrong.
    export.events[0].hash = compute_event_hash(&export.events[0]);
    export.events[1].prev_hash = export.events[0].hash.clone();
    export.events[1].hash = compute_event_hash(&export.events[1]);
    export.head.hash = export.events[1].hash.clone();
    let verification = verify_export(&export);
    assert!(!verification.ok);
    assert!(
        finding_codes(&verification).contains(&FINDING_GENESIS_MISMATCH),
        "{:?}",
        verification.findings
    );
}

#[test]
fn a_non_genesis_empty_head_is_detected() {
    let export = AuditChainExport::new(
        AuditChainHead {
            seq: 1,
            hash: "a".repeat(64),
        },
        Vec::new(),
        Vec::new(),
    );
    let verification = verify_export(&export);
    assert!(!verification.ok);
    assert_eq!(finding_codes(&verification), vec![FINDING_GENESIS_MISMATCH]);
}

#[test]
fn a_deleted_audit_log_row_is_detected() {
    let mut export = sample_export(2);
    export.audit_logs.remove(1);
    let verification = verify_export(&export);
    assert!(!verification.ok);
    assert!(
        finding_codes(&verification).contains(&FINDING_MISSING_LOG_ROW),
        "{:?}",
        verification.findings
    );
}

#[test]
fn a_mutated_audit_log_row_is_detected() {
    let mut export = sample_export(1);
    export.audit_logs[0].target = "/api/edited".to_string();
    let verification = verify_export(&export);
    assert!(!verification.ok);
    assert!(
        finding_codes(&verification).contains(&FINDING_LOG_MISMATCH),
        "{:?}",
        verification.findings
    );
}

#[test]
fn an_inserted_audit_log_row_is_detected() {
    let mut export = sample_export(1);
    export.audit_logs.push(AuditChainLogRecord::from_columns(
        "forged",
        "user-1",
        "POST /api/users",
        "/api/users",
        PHASE_SUCCESS,
        None,
        &timestamp(99),
    ));
    let verification = verify_export(&export);
    assert!(!verification.ok);
    assert!(
        finding_codes(&verification).contains(&FINDING_UNCHAINED_LOG_ROW),
        "{:?}",
        verification.findings
    );
}

#[test]
fn a_pre_chain_historical_log_row_is_not_a_finding() {
    let mut export = sample_export(1);
    export.audit_logs.push(AuditChainLogRecord::from_columns(
        "legacy",
        "user-1",
        "POST /api/legacy",
        "/api/legacy",
        PHASE_SUCCESS,
        None,
        "2020-01-01T00:00:00+00:00",
    ));
    let verification = verify_export(&export);
    assert!(verification.ok, "{:?}", verification.findings);
}

#[test]
fn conflicting_terminal_outcomes_are_detected() {
    let mut export = sample_export(1);
    // Append a failure event for the same audit id, hashed correctly so
    // only the semantic conflict remains.
    let prev = export.events.last().unwrap().hash.clone();
    let failure = build_event(AuditChainAppend {
        seq: export.events.len() as u64 + 1,
        prev_hash: &prev,
        audit_id: "audit-0",
        phase: PHASE_FAILURE,
        user_id: "user-1",
        action: "POST /api/runs",
        target: "/api/runs",
        result: PHASE_FAILURE,
        metadata: None,
        timestamp: &timestamp(0),
    });
    export.head = AuditChainHead {
        seq: failure.seq,
        hash: failure.hash.clone(),
    };
    export.events.push(failure);
    let verification = verify_export(&export);
    assert!(!verification.ok);
    assert!(
        finding_codes(&verification).contains(&FINDING_CONFLICTING_TERMINAL),
        "{:?}",
        verification.findings
    );
}

#[test]
fn an_unknown_phase_is_detected() {
    let mut export = sample_export(1);
    // Turn the terminal success into an unknown phase and rehash/relink so the
    // phase branch is the only finding.
    export.events[1].phase = "denied".to_string();
    export.events[1].hash = compute_event_hash(&export.events[1]);
    export.head.hash = export.events[1].hash.clone();
    let verification = verify_export(&export);
    assert!(!verification.ok);
    assert_eq!(finding_codes(&verification), vec![FINDING_INVALID_PHASE]);
}

#[test]
fn a_duplicate_log_id_is_detected() {
    let mut export = sample_export(1);
    let duplicate = export.audit_logs[0].clone();
    export.audit_logs.push(duplicate);
    let verification = verify_export(&export);
    assert!(!verification.ok);
    assert_eq!(finding_codes(&verification), vec![FINDING_DUPLICATE_LOG_ID]);
}

#[test]
fn export_bytes_are_deterministic_and_sorted() {
    let export = sample_export(1);
    let first = export.to_canonical_bytes();
    let second = export.to_canonical_bytes();
    assert_eq!(first, second);
    let text = String::from_utf8(first).unwrap();
    assert!(text.contains(r#""audit_id":"audit-0""#));
    assert!(text.contains(r#""metadata":{"status":200}"#));
    // Top-level keys serialize sorted: audit_logs, events, head, version.
    let value: Value = serde_json::from_str(&text).unwrap();
    let top: Vec<&String> = value.as_object().unwrap().keys().collect();
    assert_eq!(top, vec!["audit_logs", "events", "head", "version"]);
}
