# oxo-flow audit

Verify an exported audit chain offline. No database access, no network
access — the command re-checks the bytes you already hold.

## Usage

```
oxo-flow audit verify [OPTIONS] <EXPORT>
```

## Description

`oxo-flow audit verify` verifies an `oxo-flow.audit-chain/v1` document
produced by the web API's `GET /api/audit/chain/export` (see the
[Web API reference](../reference/web-api.md)).
The web server maintains an append-only, SHA-256 hash-chained copy of its
audit trail; every event links to its predecessor, and a one-row head records
the chain's current end. The export embeds the head, every chain event, and
the current `audit_logs` snapshot.

The verifier recomputes every hash and checks four things:

1. **Chain integrity** — each event carries the expected 1-based `seq`,
   links to its predecessor's `hash` (the first links to the genesis hash of
   64 zeros), and hashes to its stored `hash`.
2. **Head** — the head equals the last event (or genesis for an empty
   chain), which catches truncation and a replaced/rewritten chain.
3. **Audit-row agreement** — each chain record matches the `audit_logs` row
   with the same id (user, action, target, result, metadata, timestamp),
   which catches deleted or mutated rows.
4. **Unchained rows** — an `audit_logs` row newer than the chain start with
   no chain event is reported as inserted. Rows older than the first chain
   event predate the chain (the migration does not backfill history) and are
   accepted.

Tampering is a **finding**, not a crash: the command prints each finding and
exits non-zero. The `--json` document is suitable for automated gates.

## Exit codes

| Code | Meaning |
|------|---------|
| `0` | The export verifies: all hashes, links, the head, and the audit rows agree |
| `2` | The export is structurally valid but **invalid** — one or more findings (deleted, reordered, inserted, or mutated event/row, duplicate phase, bad genesis, truncated chain) |
| `4` | The input is malformed, unreadable, or not an `oxo-flow.audit-chain/v1` document; the error code is `AUDIT_EXPORT_INVALID` |

## Options

| Option | Description |
|--------|-------------|
| `-v, --verbose` | Enable verbose (debug-level) logging |
| `--quiet` | Suppress the version banner |
| `--no-color` | Disable colored output |
| `--json` | Output the machine-readable verification document to stdout |

## Examples

```bash
# Export the chain from a running server (admin credentials in team/HPC mode)
curl -fsS -H "Authorization: Bearer $TOKEN" \
  http://127.0.0.1:8080/api/audit/chain/export > audit-chain.json

# Verify it offline — deterministic, no database required
oxo-flow audit verify audit-chain.json

# Machine-readable verdict for a CI gate
oxo-flow audit verify --json audit-chain.json
```

## Output

Valid export:

```
Audit chain valid: 128 event(s), head seq 128 — all hashes and audit rows agree
```

Invalid export (exit 2):

```
Audit chain INVALID: 1 finding(s) across 128 event(s), head seq 128
  [MISSING_LOG_ROW] seq 96: audit_logs row '5b7d0c81-…' is missing (deleted after chaining)
```

`--json` (valid or invalid) emits one document:

```json
{
  "command": "audit verify",
  "file": "audit-chain.json",
  "format": "oxo-flow.audit-chain/v1",
  "ok": false,
  "events": 128,
  "head": {"hash": "…", "seq": 128},
  "findings": [
    {"code": "MISSING_LOG_ROW", "seq": 96, "message": "audit_logs row '5b7d0c81-…' is missing (deleted after chaining)"}
  ]
}
```

A malformed input emits (exit 4):

```json
{
  "command": "audit verify",
  "file": "bad.json",
  "ok": false,
  "error": {"code": "AUDIT_EXPORT_INVALID", "message": "not a valid audit-chain export: …"}
}
```

## Finding codes

| Code | Detects |
|------|---------|
| `SEQ_MISMATCH` | A deleted, duplicated, or reordered event (position ≠ seq) |
| `PREV_HASH_MISMATCH` | A broken link (reorder, insertion, deletion) |
| `HASH_MISMATCH` | A mutated event whose fields no longer hash to its stored hash |
| `DUPLICATE_PHASE` | The same `(audit_id, phase)` appears twice |
| `INVALID_PHASE` | An event phase other than `pending`/`success`/`failure` |
| `CONFLICTING_TERMINAL` | One audit id carries both `success` and `failure` |
| `HEAD_MISMATCH` | Truncation or a replaced chain (head ≠ last event) |
| `GENESIS_MISMATCH` | The first event does not link to genesis, or an empty chain has a non-genesis head |
| `MISSING_LOG_ROW` | A chained audit id has no `audit_logs` row (deleted) |
| `LOG_MISMATCH` | An `audit_logs` row no longer matches its chain record |
| `UNCHAINED_LOG_ROW` | A log row written after the chain start has no chain event |
| `DUPLICATE_LOG_ID` | The snapshot contains the same audit id twice |

## What verification does not prove

The chain is a **local integrity** mechanism, not a signature. An actor with
write access to the whole database can rewrite every event and the head
consistently; such a rewrite still passes this verifier. The chain's value
comes from comparing a live database against an export you already hold
(archived, copied off-host, or attached to a change record): a wholesale
rewrite cannot retroactively change the copy. No external notarization, legal
compliance, or confidentiality is claimed.

The verifier can only check documents that carry a head. A server-side
missing or deleted `audit_chain_head` row is a structural failure, not an
offline finding: the web verify endpoint answers fail-closed `500 DB_ERROR`
(and audited writes also fail), while an export document without a `head`
field is malformed input (`AUDIT_EXPORT_INVALID`, exit 4).

Audit rows are deleted when their user account is deleted (`ON DELETE
CASCADE`), while chain events are immutable. Verification then reports
`MISSING_LOG_ROW` for those rows — an intentional, reviewable signal that the
mutable trail shrank; correlate it with the account-deletion audit record.
