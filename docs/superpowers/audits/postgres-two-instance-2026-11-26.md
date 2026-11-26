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

## Partition probe (single host)

A second probe blocked all outbound PostgreSQL traffic (`iptables -A OUTPUT
-p tcp --dport 5432 -j DROP`) against one running instance:

- 10 concurrent `GET /api/runs` requests all hit a 5 s client cap (`000`),
  i.e. the server did not answer within 5 s while the database was
  unreachable.
- `GET /api/health` with a 35 s budget answered after **30 s** with HTTP 200
  `status: "degraded"`, `components.database.status: "error"`,
  `latency_ms: null`.
- Removing the rule restored `health` and `runs` to 200 within 2 s; no
  restart was needed.

Observation: the health probe does not fail fast under a database partition
— it waits for the pool/DB timeout (~30 s) before reporting degraded. That
is a measurable availability residual for load balancers with short health
timeouts; it is not a correctness failure. A health-specific DB probe
timeout is a candidate follow-up, not implemented here.

## Database restart probe (single host)

One running instance was exercised across a clean PostgreSQL restart
(`systemctl restart postgresql`):

- Before: `health` 200, `runs` 200.
- After `pg_isready` reported ready: `health` 200 `status: "ok"` in 0 s with
  `database.status: "ok"`; `runs` 200; the audit chain verified `ok: true`
  with 40 events. The web process was not restarted — the pool reconnected.

Limits: a clean service restart, not a crash/kill -9 or corruption, and no
in-flight write was deliberately overlapped.

## Connection exhaustion probe (single host)

With `max_connections = 10` (superuser-reserved 3) and six held
`pg_sleep(60)` sessions as the `oxo` role, 10 concurrent `GET /api/runs`
requests with a 10 s client cap produced **8 timeouts (`000`) and 2 HTTP 200**
responses; `/api/health` still answered 200 through an existing connection.
Releasing the held sessions restored `health`/`runs` to 200 and the 40-event
chain verified. Limits: reserved superuser connections make the exact
threshold fuzzy, and the 10 s cap is a client-side observation; no 5xx
classification is claimed.

## Crash probe: kill -9 the web process

A `sleep 30` run reached `running`; the web server was killed with `kill -9`
and restarted. Health returned 200, but the run row remained **`running`**
after restart and a further wait — a ghost row: the single-instance restart
does not reconcile in-flight runs whose executor died. The audit chain still
verified `ok: true` with 42 events. The orphaned rule process was terminated
and the test row marked failed manually for cleanup.

This is a measured crash-recovery gap, not a correctness failure of the
chain. The restarted instance can still cancel the ghost through
`POST /api/runs/{id}/cancel` (200, status `cancelled` in a follow-up probe),
so it is recoverable manually. Automatic reconciliation is what is missing:
fixing it needs an owner lease/heartbeat (or an explicit single-instance
startup reconciliation) so a second live instance's runs are not
misclassified; that is a design follow-up, not implemented here.

## Multi-user team mode probe (single host)

Team mode against PostgreSQL (`oxo_users`), fixed admin/user/viewer
accounts:

- user created 3 runs (all 200), admin created 1 (200), viewer's create was
  denied (403).
- The user's run list showed its own 3; the admin's list showed all 4.
- Audit chain verified `ok: true`, 0 findings, 13 events; the export's
  distinct `user_id` values were the admin UUID, `user`, and `viewer` (the
  denied viewer request is audited).

Limits: three fixed accounts, one host, no per-user quota/concurrent-login
campaign, and no OAuth/API-key path in this probe.

## Limits

- One host, two processes, one database; no separate hosts or network
  namespaces. The partition probe is a full outbound block on one host
  (not asymmetric/partial); the restart probe is a clean service restart;
  the crash probe kills only the web process. Network-namespace isolation,
  asymmetric partitions, database corruption, and kill-mid-transaction
  remain unmeasured. The ghost-row reconciliation gap is documented above.
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
