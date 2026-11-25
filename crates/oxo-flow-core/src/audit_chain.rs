//! Hash-chained, externally verifiable audit records (issue #1372).
//!
//! The web server's `audit_logs` table is the listing/UI surface. It is a
//! mutable SQL table: anyone who can write the database can rewrite, reorder,
//! delete, or insert a row. This module defines a second, append-only record —
//! `audit_chain` — that links every audit event to its predecessor with
//! SHA-256, plus a canonical export format a holder can verify offline with
//! `oxo-flow audit verify`.
//!
//! # What the chain proves, and what it does not
//!
//! Hash chaining detects *local* edits: a deleted event leaves a sequence gap
//! or a head mismatch; a reordered event breaks the `prev_hash` link; a mutated
//! field no longer hashes to its stored `hash`; an inserted event either
//! duplicates a position or fails verification. It is **not** a signature and
//! **not** external notarization: an attacker with write access to the whole
//! chain (and the head row) can recompute every hash. The value comes from
//! *holding an export*: a previously exported head cannot be rewritten in the
//! copy you already have, so a later wholesale rewrite of the database is
//! detected by comparing the two. The docs state this explicitly; no legal or
//! regulatory compliance is claimed.
//!
//! # Canonical form
//!
//! Events are encoded as a JSON object with keys serialized in sorted order
//! (`serde_json`'s default BTreeMap-backed map — no `preserve_order` feature)
//! and `metadata` normalized to canonical JSON (keys sorted, no whitespace).
//! Timestamps written by the server use a fixed UTC format with 6 fractional
//! digits and a `Z` suffix. The hash input additionally carries the format tag
//! so a future chain format cannot be confused with this one.

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use std::collections::BTreeMap;

/// Version tag of the exported audit-chain document.
pub const AUDIT_CHAIN_FORMAT: &str = "oxo-flow.audit-chain/v1";

/// Hash stored as `prev_hash` by the first event and as the head of an empty
/// chain: 64 hexadecimal zeros (SHA-256 output width).
pub const GENESIS_HASH: &str = "0000000000000000000000000000000000000000000000000000000000000000";

/// A write-ahead intent that has not reached an outcome yet.
pub const PHASE_PENDING: &str = "pending";
/// The audited action completed successfully.
pub const PHASE_SUCCESS: &str = "success";
/// The audited action failed.
pub const PHASE_FAILURE: &str = "failure";

/// Finding code: the first event does not link to the genesis hash.
pub const FINDING_GENESIS_MISMATCH: &str = "GENESIS_MISMATCH";
/// Finding code: an event's `seq` is not the expected 1-based position.
pub const FINDING_SEQ_MISMATCH: &str = "SEQ_MISMATCH";
/// Finding code: an event's `prev_hash` is not its predecessor's `hash`.
pub const FINDING_PREV_HASH_MISMATCH: &str = "PREV_HASH_MISMATCH";
/// Finding code: an event's stored `hash` is not the hash of its fields.
pub const FINDING_HASH_MISMATCH: &str = "HASH_MISMATCH";
/// Finding code: one `(audit_id, phase)` pair appears more than once.
pub const FINDING_DUPLICATE_PHASE: &str = "DUPLICATE_PHASE";
/// Finding code: an event carries an unknown `phase`.
pub const FINDING_INVALID_PHASE: &str = "INVALID_PHASE";
/// Finding code: one audit id carries both a success and a failure event.
pub const FINDING_CONFLICTING_TERMINAL: &str = "CONFLICTING_TERMINAL";
/// Finding code: the head does not match the events (truncation/replacement).
pub const FINDING_HEAD_MISMATCH: &str = "HEAD_MISMATCH";
/// Finding code: a chained audit id has no row in the `audit_logs` snapshot.
pub const FINDING_MISSING_LOG_ROW: &str = "MISSING_LOG_ROW";
/// Finding code: an `audit_logs` row disagrees with its chain record.
pub const FINDING_LOG_MISMATCH: &str = "LOG_MISMATCH";
/// Finding code: an `audit_logs` row is newer than the chain start but has no
/// chain event.
pub const FINDING_UNCHAINED_LOG_ROW: &str = "UNCHAINED_LOG_ROW";
/// Finding code: the `audit_logs` snapshot contains the same id twice.
pub const FINDING_DUPLICATE_LOG_ID: &str = "DUPLICATE_LOG_ID";

/// Fixed-format UTC timestamp used for audit rows and chain events:
/// `YYYY-MM-DDTHH:MM:SS.ffffffZ` (always exactly 6 fractional digits).
///
/// A fixed width keeps exported bytes and lexicographic ordering stable; the
/// pre-chain `chrono::to_rfc3339()` format (`+00:00`, variable fraction) is
/// still accepted in historical rows and is never rewritten.
#[must_use]
pub fn canonical_timestamp(timestamp: chrono::DateTime<chrono::Utc>) -> String {
    timestamp.format("%Y-%m-%dT%H:%M:%S%.6fZ").to_string()
}

/// [`canonical_timestamp`] for the current instant.
#[must_use]
pub fn now_canonical_timestamp() -> String {
    canonical_timestamp(chrono::Utc::now())
}

/// Normalize a metadata payload to canonical JSON text.
///
/// `None` stays `None` (SQL `NULL`). Text that parses as JSON is re-serialized
/// with sorted keys and no whitespace. Text that is not JSON is preserved as an
/// opaque JSON string value, so the function is total and a malformed payload
/// can never make an audit write fail.
#[must_use]
pub fn canonical_metadata_text(metadata: Option<&str>) -> Option<String> {
    metadata.map(|text| {
        serde_json::to_string(&metadata_value(Some(text)))
            .expect("canonical JSON serialization cannot fail")
    })
}

/// The JSON value a metadata column contributes to a chain event and to the
/// export. Valid JSON text is parsed; `None` is JSON `null`; invalid text is
/// wrapped as a JSON string.
#[must_use]
pub fn metadata_value(metadata: Option<&str>) -> Value {
    match metadata {
        None => Value::Null,
        Some(text) => serde_json::from_str(text).unwrap_or_else(|_| Value::String(text.into())),
    }
}

/// One immutable link in the audit chain.
///
/// `metadata` is held as parsed JSON so the export embeds it as JSON rather
/// than as an escaped string; the database stores its canonical text form.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AuditChainEvent {
    /// 1-based position in the chain.
    pub seq: u64,
    /// The `audit_logs.id` this event belongs to.
    pub audit_id: String,
    /// `pending` | `success` | `failure`.
    pub phase: String,
    pub user_id: String,
    pub action: String,
    pub target: String,
    /// The audit row's result at the time of the event.
    pub result: String,
    /// Canonical metadata payload; `null` when absent.
    pub metadata: Option<Value>,
    /// The audit row's timestamp (fixed canonical format for new rows).
    pub timestamp: String,
    /// The predecessor's `hash` (genesis for the first event).
    pub prev_hash: String,
    /// SHA-256 of the canonical event fields.
    pub hash: String,
}

impl AuditChainEvent {
    /// The canonical JSON value hashed by [`compute_event_hash`]. Keys are
    /// emitted in sorted order by the BTreeMap-backed `serde_json::Map`; the
    /// format tag is part of the hash domain and `hash` itself is excluded (it
    /// is the digest of this value).
    #[must_use]
    pub fn canonical_value(&self) -> Value {
        let mut map = Map::new();
        map.insert("format".into(), Value::String(AUDIT_CHAIN_FORMAT.into()));
        map.insert("seq".into(), Value::from(self.seq));
        map.insert("audit_id".into(), Value::String(self.audit_id.clone()));
        map.insert("phase".into(), Value::String(self.phase.clone()));
        map.insert("user_id".into(), Value::String(self.user_id.clone()));
        map.insert("action".into(), Value::String(self.action.clone()));
        map.insert("target".into(), Value::String(self.target.clone()));
        map.insert("result".into(), Value::String(self.result.clone()));
        map.insert(
            "metadata".into(),
            self.metadata.clone().unwrap_or(Value::Null),
        );
        map.insert("timestamp".into(), Value::String(self.timestamp.clone()));
        map.insert("prev_hash".into(), Value::String(self.prev_hash.clone()));
        Value::Object(map)
    }

    /// The canonical metadata text stored in the `metadata` column.
    #[must_use]
    pub fn metadata_text(&self) -> Option<String> {
        self.metadata.as_ref().map(|value| {
            serde_json::to_string(value).expect("canonical metadata serialization cannot fail")
        })
    }

    /// SHA-256 (lowercase hex) of the event's canonical fields.
    #[must_use]
    pub fn computed_hash(&self) -> String {
        compute_event_hash(self)
    }

    /// The exported representation: the event fields plus `hash`. The format
    /// tag is part of the hash domain but is not repeated on every event — the
    /// document's top-level `version` names the format once.
    #[must_use]
    pub fn export_value(&self) -> Value {
        let mut value = self.canonical_value();
        if let Some(map) = value.as_object_mut() {
            map.remove("format");
            map.insert("hash".into(), Value::String(self.hash.clone()));
        }
        value
    }
}

/// SHA-256 (lowercase hex) of the event's canonical fields.
#[must_use]
pub fn compute_event_hash(event: &AuditChainEvent) -> String {
    use sha2::{Digest, Sha256};
    let bytes = serde_json::to_vec(&event.canonical_value())
        .expect("canonical audit-chain event serialization cannot fail");
    hex::encode(Sha256::digest(bytes))
}

/// The fields needed to append a new event. Borrowed to keep call sites small.
#[derive(Debug, Clone, Copy)]
pub struct AuditChainAppend<'a> {
    pub seq: u64,
    pub prev_hash: &'a str,
    pub audit_id: &'a str,
    pub phase: &'a str,
    pub user_id: &'a str,
    pub action: &'a str,
    pub target: &'a str,
    pub result: &'a str,
    pub metadata: Option<&'a str>,
    pub timestamp: &'a str,
}

/// Build (and hash) an event for `seq`/`prev_hash`.
#[must_use]
pub fn build_event(append: AuditChainAppend<'_>) -> AuditChainEvent {
    let mut event = AuditChainEvent {
        seq: append.seq,
        audit_id: append.audit_id.to_string(),
        phase: append.phase.to_string(),
        user_id: append.user_id.to_string(),
        action: append.action.to_string(),
        target: append.target.to_string(),
        result: append.result.to_string(),
        metadata: Some(metadata_value(append.metadata)).filter(|v| !v.is_null()),
        timestamp: append.timestamp.to_string(),
        prev_hash: append.prev_hash.to_string(),
        hash: String::new(),
    };
    event.hash = compute_event_hash(&event);
    event
}

/// Cursor at the end of the chain. `seq = 0` / [`GENESIS_HASH`] for an empty
/// chain.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AuditChainHead {
    pub seq: u64,
    pub hash: String,
}

impl AuditChainHead {
    /// The empty-chain head.
    #[must_use]
    pub fn genesis() -> Self {
        Self {
            seq: 0,
            hash: GENESIS_HASH.to_string(),
        }
    }

    /// Exported representation (sorted keys).
    #[must_use]
    pub fn export_value(&self) -> Value {
        serde_json::json!({ "seq": self.seq, "hash": self.hash })
    }
}

/// One `audit_logs` row as it appears in an export. The snapshot is compared
/// against the chain terminals, so deleting or mutating a row after the fact is
/// a finding.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AuditChainLogRecord {
    pub id: String,
    pub user_id: String,
    pub action: String,
    pub target: String,
    pub result: String,
    pub metadata: Option<Value>,
    pub timestamp: String,
}

impl AuditChainLogRecord {
    /// Build a record from the stored `audit_logs` columns.
    #[must_use]
    pub fn from_columns(
        id: &str,
        user_id: &str,
        action: &str,
        target: &str,
        result: &str,
        metadata: Option<&str>,
        timestamp: &str,
    ) -> Self {
        Self {
            id: id.to_string(),
            user_id: user_id.to_string(),
            action: action.to_string(),
            target: target.to_string(),
            result: result.to_string(),
            metadata: Some(metadata_value(metadata)).filter(|v| !v.is_null()),
            timestamp: timestamp.to_string(),
        }
    }

    /// Exported representation (sorted keys).
    #[must_use]
    pub fn export_value(&self) -> Value {
        serde_json::json!({
            "id": self.id,
            "user_id": self.user_id,
            "action": self.action,
            "target": self.target,
            "result": self.result,
            "metadata": self.metadata.clone().unwrap_or(Value::Null),
            "timestamp": self.timestamp,
        })
    }
}

/// The portable audit-chain document (`oxo-flow.audit-chain/v1`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AuditChainExport {
    pub version: String,
    pub head: AuditChainHead,
    pub events: Vec<AuditChainEvent>,
    /// The full `audit_logs` snapshot (all rows, not only chained ones).
    pub audit_logs: Vec<AuditChainLogRecord>,
}

impl AuditChainExport {
    /// Assemble an export from a snapshot. The caller supplies events in
    /// ascending `seq` order and log rows sorted by `id`; this function does
    /// not reorder (an out-of-order input is exactly what verification must
    /// see).
    #[must_use]
    pub fn new(
        head: AuditChainHead,
        events: Vec<AuditChainEvent>,
        audit_logs: Vec<AuditChainLogRecord>,
    ) -> Self {
        Self {
            version: AUDIT_CHAIN_FORMAT.to_string(),
            head,
            events,
            audit_logs,
        }
    }

    /// Deterministic JSON bytes. Every object is key-sorted and every event is
    /// encoded through [`AuditChainEvent::export_value`], so two exports of the
    /// same database state are byte-identical.
    #[must_use]
    pub fn to_canonical_bytes(&self) -> Vec<u8> {
        serde_json::to_vec(&self.to_canonical_value())
            .expect("canonical audit-chain export serialization cannot fail")
    }

    /// Deterministic JSON value with sorted keys.
    #[must_use]
    pub fn to_canonical_value(&self) -> Value {
        serde_json::json!({
            "version": self.version,
            "head": self.head.export_value(),
            "events": self.events.iter().map(AuditChainEvent::export_value).collect::<Vec<_>>(),
            "audit_logs": self.audit_logs.iter().map(AuditChainLogRecord::export_value).collect::<Vec<_>>(),
        })
    }
}

/// One verification finding. `seq` names the chain position when the finding
/// is tied to one event; it is omitted for document-level findings.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AuditChainFinding {
    pub code: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub seq: Option<u64>,
    pub message: String,
}

impl AuditChainFinding {
    fn new(code: &str, seq: Option<u64>, message: impl Into<String>) -> Self {
        Self {
            code: code.to_string(),
            seq,
            message: message.into(),
        }
    }

    /// Exported representation (sorted keys).
    #[must_use]
    pub fn export_value(&self) -> Value {
        match self.seq {
            Some(seq) => serde_json::json!({
                "code": self.code,
                "seq": seq,
                "message": self.message,
            }),
            None => serde_json::json!({
                "code": self.code,
                "message": self.message,
            }),
        }
    }
}

/// Outcome of verifying an export. `ok` is true exactly when `findings` is
/// empty; tampering is a finding, never an error.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AuditChainVerification {
    pub ok: bool,
    pub events: usize,
    pub head: AuditChainHead,
    pub findings: Vec<AuditChainFinding>,
}

impl AuditChainVerification {
    /// Deterministic JSON value matching the web `GET /api/audit/chain/verify`
    /// and CLI `audit verify --json` body.
    #[must_use]
    pub fn to_canonical_value(&self) -> Value {
        serde_json::json!({
            "ok": self.ok,
            "events": self.events,
            "head": self.head.export_value(),
            "findings": self.findings.iter().map(AuditChainFinding::export_value).collect::<Vec<_>>(),
        })
    }
}

/// Verify a chain export end to end.
///
/// Checks, in order:
///
/// 1. **Chain integrity** — every event occupies the expected 1-based `seq`,
///    links to its predecessor's `hash` (genesis first), hashes to its stored
///    `hash`, carries a known phase, and no `(audit_id, phase)` repeats.
/// 2. **Head** — matches the last event (or genesis for an empty chain), which
///    detects truncation and whole-chain replacement with a stale head.
/// 3. **`audit_logs` agreement** — every chained audit id has a row whose
///    user/action/target/result/metadata/timestamp equal the event's record
///    (deletion and mutation detection).
/// 4. **Unchained rows** — a log row newer than the chain start with no chain
///    event is reported (insertion detection). Pre-chain historical rows (older
///    than the first event) are accepted: the chain does not cover them.
///
/// The check is deliberately honest about its limit: an actor who can rewrite
/// the events AND the head consistently passes this verification. Only
/// comparing against a previously held export detects that.
#[must_use]
pub fn verify_export(export: &AuditChainExport) -> AuditChainVerification {
    let mut findings = Vec::new();
    verify_chain_integrity(&export.events, &mut findings);
    verify_genesis_and_head(&export.events, &export.head, &mut findings);
    verify_log_agreement(&export.events, &export.audit_logs, &mut findings);
    AuditChainVerification {
        ok: findings.is_empty(),
        events: export.events.len(),
        head: export.head.clone(),
        findings,
    }
}

/// Pass 1: the 1-based position sequence, every `prev_hash` link, the known
/// phase set, one `(audit_id, phase)` per chain, and the recomputed hash.
fn verify_chain_integrity(events: &[AuditChainEvent], findings: &mut Vec<AuditChainFinding>) {
    let mut expected_prev = GENESIS_HASH.to_string();
    let mut seen_phases: BTreeMap<(String, String), u64> = BTreeMap::new();
    let mut terminal_phases: BTreeMap<&str, Vec<&str>> = BTreeMap::new();
    for (index, event) in events.iter().enumerate() {
        verify_event_integrity(
            event,
            (index as u64) + 1,
            &expected_prev,
            &mut seen_phases,
            &mut terminal_phases,
            findings,
        );
        expected_prev = event.hash.clone();
    }
    report_conflicting_terminals(terminal_phases, findings);
}

/// One event's position, link, phase, uniqueness and hash checks.
fn verify_event_integrity<'a>(
    event: &'a AuditChainEvent,
    expected_seq: u64,
    expected_prev: &str,
    seen_phases: &mut BTreeMap<(String, String), u64>,
    terminal_phases: &mut BTreeMap<&'a str, Vec<&'a str>>,
    findings: &mut Vec<AuditChainFinding>,
) {
    verify_event_fields(event, expected_seq, expected_prev, findings);
    note_event_phase(event, seen_phases, terminal_phases, findings);
    verify_event_hash(event, findings);
}

/// Position, predecessor link, and known-phase checks for one event.
fn verify_event_fields(
    event: &AuditChainEvent,
    expected_seq: u64,
    expected_prev: &str,
    findings: &mut Vec<AuditChainFinding>,
) {
    if event.seq != expected_seq {
        findings.push(AuditChainFinding::new(
            FINDING_SEQ_MISMATCH,
            Some(event.seq),
            format!(
                "event at position {expected_seq} carries seq {} (gap, duplicate, or reorder)",
                event.seq
            ),
        ));
    }
    if event.prev_hash != expected_prev {
        findings.push(AuditChainFinding::new(
            FINDING_PREV_HASH_MISMATCH,
            Some(event.seq),
            format!(
                "prev_hash {} does not match the predecessor hash {}",
                event.prev_hash, expected_prev
            ),
        ));
    }
    if !matches!(
        event.phase.as_str(),
        PHASE_PENDING | PHASE_SUCCESS | PHASE_FAILURE
    ) {
        findings.push(AuditChainFinding::new(
            FINDING_INVALID_PHASE,
            Some(event.seq),
            format!("unknown phase '{}'", event.phase),
        ));
    }
}

/// The `(audit_id, phase)` uniqueness finding and the terminal-phase
/// collection consumed by [`report_conflicting_terminals`].
fn note_event_phase<'a>(
    event: &'a AuditChainEvent,
    seen_phases: &mut BTreeMap<(String, String), u64>,
    terminal_phases: &mut BTreeMap<&'a str, Vec<&'a str>>,
    findings: &mut Vec<AuditChainFinding>,
) {
    if seen_phases
        .insert((event.audit_id.clone(), event.phase.clone()), event.seq)
        .is_some()
    {
        findings.push(AuditChainFinding::new(
            FINDING_DUPLICATE_PHASE,
            Some(event.seq),
            format!(
                "audit id '{}' carries more than one '{}' event",
                event.audit_id, event.phase
            ),
        ));
    }
    if event.phase != PHASE_PENDING {
        terminal_phases
            .entry(event.audit_id.as_str())
            .or_default()
            .push(event.phase.as_str());
    }
}

/// Recompute the hash and compare it with the stored value.
fn verify_event_hash(event: &AuditChainEvent, findings: &mut Vec<AuditChainFinding>) {
    let computed = compute_event_hash(event);
    if computed != event.hash {
        findings.push(AuditChainFinding::new(
            FINDING_HASH_MISMATCH,
            Some(event.seq),
            format!(
                "stored hash {} does not match the recomputed hash {computed}",
                event.hash
            ),
        ));
    }
}

/// More than one terminal phase for one audit id is a conflicting outcome.
fn report_conflicting_terminals(
    terminal_phases: BTreeMap<&str, Vec<&str>>,
    findings: &mut Vec<AuditChainFinding>,
) {
    for (audit_id, phases) in terminal_phases {
        if phases.len() > 1 {
            findings.push(AuditChainFinding::new(
                FINDING_CONFLICTING_TERMINAL,
                None,
                format!(
                    "audit id '{audit_id}' carries multiple terminal events ({})",
                    phases.join(", ")
                ),
            ));
        }
    }
}

/// Pass 2: the empty-chain genesis head, the first event's genesis link, and
/// the head/last-event agreement (truncation/replacement).
fn verify_genesis_and_head(
    events: &[AuditChainEvent],
    head: &AuditChainHead,
    findings: &mut Vec<AuditChainFinding>,
) {
    let expected_head = events
        .last()
        .map_or_else(AuditChainHead::genesis, |last| AuditChainHead {
            seq: last.seq,
            hash: last.hash.clone(),
        });
    if events.is_empty() {
        if head != &AuditChainHead::genesis() {
            findings.push(AuditChainFinding::new(
                FINDING_GENESIS_MISMATCH,
                None,
                format!(
                    "an empty chain must carry the genesis head, got seq {} hash {}",
                    head.seq, head.hash
                ),
            ));
        }
    } else if events[0].prev_hash != GENESIS_HASH {
        findings.push(AuditChainFinding::new(
            FINDING_GENESIS_MISMATCH,
            Some(events[0].seq),
            format!(
                "the first event does not link to genesis (prev_hash {})",
                events[0].prev_hash
            ),
        ));
    }
    if head != &expected_head && !events.is_empty() {
        findings.push(AuditChainFinding::new(
            FINDING_HEAD_MISMATCH,
            Some(head.seq),
            format!(
                "head (seq {}, hash {}) does not match the last event (seq {}, hash {}) — \
                 truncated or replaced chain",
                head.seq, head.hash, expected_head.seq, expected_head.hash
            ),
        ));
    }
}

/// Pass 3: the `audit_logs` snapshot must agree with each audit id's final
/// chain record, and no post-chain-start row may be missing from the chain.
fn verify_log_agreement(
    events: &[AuditChainEvent],
    logs: &[AuditChainLogRecord],
    findings: &mut Vec<AuditChainFinding>,
) {
    let logs_by_id = index_audit_logs(logs, findings);
    let last_event_by_id = last_event_per_audit(events);
    verify_log_rows(&last_event_by_id, &logs_by_id, findings);
    verify_unchained_rows(events, logs, &last_event_by_id, findings);
}

/// Index the snapshot by audit id, reporting a duplicated id.
fn index_audit_logs<'a>(
    logs: &'a [AuditChainLogRecord],
    findings: &mut Vec<AuditChainFinding>,
) -> BTreeMap<&'a str, &'a AuditChainLogRecord> {
    let mut logs_by_id = BTreeMap::new();
    for log in logs {
        if logs_by_id.insert(log.id.as_str(), log).is_some() {
            findings.push(AuditChainFinding::new(
                FINDING_DUPLICATE_LOG_ID,
                None,
                format!("audit_logs snapshot contains id '{}' twice", log.id),
            ));
        }
    }
    logs_by_id
}

/// The final chain record for each audit id — the one the log row must match.
fn last_event_per_audit(events: &[AuditChainEvent]) -> BTreeMap<&str, &AuditChainEvent> {
    let mut last_event_by_id = BTreeMap::new();
    for event in events {
        last_event_by_id.insert(event.audit_id.as_str(), event);
    }
    last_event_by_id
}

/// Deletion and mutation detection: every chained id has a row, and the row
/// still equals its final chain record.
fn verify_log_rows(
    last_event_by_id: &BTreeMap<&str, &AuditChainEvent>,
    logs_by_id: &BTreeMap<&str, &AuditChainLogRecord>,
    findings: &mut Vec<AuditChainFinding>,
) {
    for (audit_id, event) in last_event_by_id {
        match logs_by_id.get(audit_id) {
            None => findings.push(AuditChainFinding::new(
                FINDING_MISSING_LOG_ROW,
                Some(event.seq),
                format!("audit_logs row '{audit_id}' is missing (deleted after chaining)"),
            )),
            Some(log) if !log_matches_event(log, event) => {
                findings.push(AuditChainFinding::new(
                    FINDING_LOG_MISMATCH,
                    Some(event.seq),
                    format!(
                        "audit_logs row '{audit_id}' no longer matches its chain record \
                         (deleted, mutated, or finalized outside the chain)"
                    ),
                ));
            }
            Some(_) => {}
        }
    }
}

/// Rows written after the chain started must have a chain event. Rows older
/// than the first event predate the chain (the migration did not backfill
/// history) and are accepted.
fn verify_unchained_rows(
    events: &[AuditChainEvent],
    logs: &[AuditChainLogRecord],
    last_event_by_id: &BTreeMap<&str, &AuditChainEvent>,
    findings: &mut Vec<AuditChainFinding>,
) {
    let Some(first) = events.first() else {
        return;
    };
    for log in logs {
        if !last_event_by_id.contains_key(log.id.as_str())
            && log.timestamp.as_str() >= first.timestamp.as_str()
        {
            findings.push(AuditChainFinding::new(
                FINDING_UNCHAINED_LOG_ROW,
                None,
                format!(
                    "audit_logs row '{}' ({}) has no chain event",
                    log.id, log.timestamp
                ),
            ));
        }
    }
}

/// True when the current `audit_logs` row is exactly the record its final
/// chain event captured.
fn log_matches_event(log: &AuditChainLogRecord, event: &AuditChainEvent) -> bool {
    log.user_id == event.user_id
        && log.action == event.action
        && log.target == event.target
        && log.result == event.result
        && log.timestamp == event.timestamp
        && log.metadata == event.metadata
}

#[cfg(test)]
mod tests;
