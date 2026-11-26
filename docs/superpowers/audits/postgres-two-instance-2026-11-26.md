# PostgreSQL two-instance admission and audit-chain smoke — 2026-11-26

Partial evidence for issue #1402 (multi-instance PostgreSQL campaign). This
is **not** a multi-host or production-scale campaign: two web processes ran
on one host against one PostgreSQL database.

## Environment

| Field | Value |
| --- | --- |
| Host | `VM-4-4-ubuntu`, Linux 6.8.0-137, 4 vCPU / 3.6 GiB |
| PostgreSQL | 16.15, local, database `oxo_multi` owned by role `oxo` |
| Web binary | `0.32.0`, main `93fa06c3`, release, `postgres` feature |
| Instances | two processes, `127.0.0.1:18110` and `127.0.0.1:18111` |
| Shared state | same `DATABASE_URL`, same `OXO_FLOW_WORKSPACE` |
| Mode | personal (no auth), AI disabled, run-creation rate limit disabled |

## Scenario and results

- 20 concurrent `POST /api/runs` calls (10 per instance) with the same
  single-rule workflow.
- **10 runs admitted**, 10 rejected with the structured
  `QUOTA_EXCEEDED` response (`Concurrent run limit reached: 10/10`) — the
  quota is global across the two instances, not per process.
- All 10 admitted runs completed; both instances list `completed: 10`.
- Audit chain: both instances verify `ok: true`, `0` findings, **40
  events**; the two exports are byte-identical
  (`sha256:bf78f91aa06e54ce325bfc26a6f81f3ee10605641179e8100e4eb0e202d6cfb8`).
- Restarting one instance (health ok) leaves the chain verifiable with the
  same 40 events.

## Limits

- One host, two processes, one database; no separate hosts or network
  namespaces, no network partition, no pool exhaustion, no database
  restart, and no kill-mid-transaction.
- Personal mode only; no multi-user RBAC or per-user quota interaction.
- The 10-run cap is the configured concurrent-run quota, not a defect.
- No production-scale or crash-recovery claim is made. The full #1402
  matrix (separate hosts, partitions, pool exhaustion, crash recovery)
  remains open.

## Reproduction

The host-local script that produced this evidence is intentionally not
committed; it starts both instances with the same `DATABASE_URL` /
workspace, fires the 20 requests, polls run status, verifies/exports the
chain on both instances, restarts one, and re-verifies. Raw output is in
the campaign ledger (`pg-two-instance-2026-11-26.log`).
