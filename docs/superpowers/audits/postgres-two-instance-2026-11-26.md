# PostgreSQL multi-instance, crash-recovery, and pool-exhaustion campaign (issue #1402)

Campaign evidence for issue #1402. It measures the PostgreSQL run ledger with
two web instances on one host, two instances in separate Linux network
namespaces, an asymmetric partition, connection exhaustion, a per-user quota
isolation probe, a `kill -9` web-process crash (including the owner-lease
reconciliation fix), and a `SIGKILL` PostgreSQL crash during an uncommitted
transaction. Three defects the campaign proved are fixed: the health bound
and the lease reconciliation have committed regression tests, and the
pool-acquire bound is verified by the live pool-exhaustion probe (pool
construction is exercised by every web test). The residual list below is part
of the result: a separate
physical host, production scale, packet-loss/latency injection, per-query
network timeouts, and database media corruption remain unmeasured, so **no
production-parity claim is made**.

Raw output for the five new probes is committed at
[`data/postgres-1402-campaign-2026-11-26.log`](data/postgres-1402-campaign-2026-11-26.log).
The earlier probes' raw output stayed on the campaign host; the page records
their measured values.

## Environment

| Field | Value |
| --- | --- |
| Host | `VM-4-4-ubuntu`, Linux 6.8.0-137, 4 vCPU / 3.6 GiB |
| PostgreSQL | 16.15, local cluster `postgresql@16-main`, database `oxo_1402` per probe |
| Web binary | `0.32.0`, release, `--features postgres`, built from the campaign branch (this PR) |
| CLI binary | `0.32.0`, release, unchanged by this PR (it executes workflow rules; the web owns the ledger) |
| Instances | two processes (`127.0.0.1:18110`/`:18111`), two network namespaces (`10.200.0.2:18121`/`10.200.0.3:18122`), and single instances on ports `18130`-`18135` for the focused probes |
| Shared state | one `DATABASE_URL`; same `OXO_FLOW_WORKSPACE` within a probe |
| Mode | personal (no auth) except the quota and multi-user probes (team mode, fixed test accounts) |

## Scenario matrix

| Probe | Measured outcome | Section |
| --- | --- | --- |
| Two web instances, one database, 20 concurrent creates | 10 admitted, 10 `QUOTA_EXCEEDED` (the shared `default` identity's 10-run quota); both instances list the runs; 40-event chain verifies byte-identically | [Two-instance pair](#two-instance-pair-one-host) |
| Two instances in separate network namespaces | 10/10 creates succeed; 22-run shared view; 70-event chain byte-identical; asymmetric in-namespace `DROP` affects only that instance | [Network-namespace pair](#network-namespace-pair-and-asymmetric-partition) |
| Per-user quota isolation (team mode) | user at 1/1 gets `429`; admin's create is `200`; each identity's `/api/quota` usage is its own; release restores both | [Per-user quota isolation](#per-user-quota-isolation-team-mode) |
| Full outbound partition (`iptables DROP 5432`) | `/api/health` **4013 ms** (pre-fix 30 s) `200 degraded`; `GET /api/runs` `500 DB_ERROR` in **5011 ms**; recovery in 22 ms | [Health fail-fast](#health-fail-fast-and-bounded-pool-acquire) |
| Connection exhaustion (`max_connections=10`, six held sessions) | 8 requests `500` at **max 5031 ms**, 2 `200` (pre-fix: 8/10 hit a 10 s client cap); health stays `200 ok` | [Pool exhaustion](#pool-exhaustion-re-check) |
| `kill -9` web process with two running orphans | after restart and **91 s**, still-running orphan → `failed`; finished orphan (exit record 0) → `completed`; quota released; chain `ok` | [Ghost-run reconciliation](#ghost-run-reconciliation-after-a-web-process-crash) |
| `SIGKILL` PostgreSQL mid-transaction | uncommitted run and audit rows roll back (`0`/`0`); WAL recovery logged; same web process reconnects; live run completes; chain `ok` | [Database crash](#database-crash-during-an-uncommitted-transaction) |
| Clean database restart | health/runs recover, 40-event chain verifies without a web restart | [Earlier probes](#earlier-probes) |
| Multi-user team mode | user 3 runs, admin 1, viewer denied `403`; views scoped; 13-event chain with distinct ids | [Earlier probes](#earlier-probes) |

## Two-instance pair (one host)

Two web processes (`127.0.0.1:18110`, `127.0.0.1:18111`) shared one
PostgreSQL database and one workspace; personal mode, AI disabled,
run-creation rate limit disabled.

- 20 concurrent `POST /api/runs` calls (10 per instance) with the same
  single-rule workflow.
- **10 runs admitted**, 10 rejected with the structured
  `QUOTA_EXCEEDED` response (`Concurrent run limit reached: 10/10`). Both
  processes act as the same `default` identity in personal mode, so this is
  the **per-user** quota shared across instances (RB-09), not a per-process
  counter; the team-mode probe below shows that a different identity has its
  own allowance.
- All 10 admitted runs completed; both instances list `completed: 10`.
- Audit chain: both instances verify `ok: true`, `0` findings, **40
  events**; the two exports are byte-identical
  (`sha256:bf78f91aa06e54ce325bfc26a6f81f3ee10605641179e8100e4eb0e202d6cfb8`).
- Restarting one instance (health ok) leaves the chain verifiable with the
  same 40 events.

## Network-namespace pair and asymmetric partition

Two web instances ran in **separate Linux network namespaces**
(`10.200.0.2:18121` and `10.200.0.3:18122`, team mode), connected to the
same PostgreSQL database through a host TCP forwarder
(`10.200.0.1:15432 -> 127.0.0.1:5432`) and sharing one workspace:

- 10 concurrent create-run requests (5 per instance) all returned 200.
- Both admin views listed the same 22 runs (shared database; includes
  earlier probes); both audit chains verified `ok: true`, 0 findings with
  **70 events**, and the two exports were byte-identical
  (`sha256:2d5e1ca061888bcea93879297f55358d539b29696e58b9230c9fa7a13ff67213`).
- **Asymmetric partition**: an `OUTPUT` DROP to the database inside the
  first namespace only made that instance's run-list request hit the 5 s
  client cap (`000`) while the second instance answered 200; removing the
  rule restored the first instance to 200 within 2 s.

Limits: both namespaces are on the same physical host and the database is
reached through a TCP forwarder, not a real inter-host network; there is no
packet loss/latency injection, and the partition is an outbound DROP rather
than a routing partition. This is the network-namespace step of #1402, not
a multi-host claim.

## Per-user quota isolation (team mode)

Three fixed accounts (admin/user/viewer) against PostgreSQL; the admin set
`max_concurrent_runs = 1` for every identity through `PUT /api/quota`:

- user's first create → `200`; once the run reached `running`, the user's
  second create → `429 {"code":"QUOTA_EXCEEDED","message":"… Concurrent run
  limit reached: 1/1 …"}`.
- admin's create in the same window → `200` — the user's consumed slot is
  not charged to the admin identity.
- `/api/quota` reported `active_runs: 1` for **each** identity (its own
  usage), and after cancelling both runs both returned `active_runs: 0`;
  `runs_today` stayed `1` (completion never erases the daily count).
- Audit chain verified `ok: true`, 0 findings, 14 events. This fixes the
  per-user quota question left open by the earlier multi-user probe: the
  durable counters are keyed by `users.id`, so one user at its cap does not
  consume another user's allowance.

## Health fail-fast and bounded pool acquire

The earlier partition probe measured `/api/health` taking **30 s** (the sqlx
pool acquire timeout) before returning `200 degraded` — a load balancer
with a shorter timeout would treat a live server as dead. The fix bounds
the health database probe to 4 s (`HEALTH_DB_PROBE_TIMEOUT`) and every
PostgreSQL pool acquire to 5 s (`DB_ACQUIRE_TIMEOUT`; SQLite keeps sqlx's
default because its local pool contention under parallel test load is
legitimate, and the health probe bounds both backends).

Re-run with a full outbound PostgreSQL partition (`iptables -A OUTPUT -p tcp
--dport 5432 -j DROP`):

- `/api/health` answered **4013 ms**, HTTP 200,
  `{"status":"degraded","components":{"database":{"status":"error"}}}`.
- `GET /api/runs` answered **5011 ms** with a structured
  `500 {"code":"DB_ERROR","message":"Internal database error"}` instead of
  hanging to the client cap.
- Removing the rule restored health in **22 ms** and runs to 200 without a
  restart; the chain verified.
- A `POST /api/runs` during the same partition still hit the 15 s client
  cap (`000`): a request that already holds a connection to a black-holed
  server can block in TCP retransmission, which a pool *acquire* timeout
  cannot bound. That is a documented residual, not a regression — the
  health endpoint and pool waits are bounded, per-query network waits are
  not.

## Pool exhaustion re-check

With `max_connections = 10` (three reserved for superusers) and six held
`pg_sleep(60)` sessions, ten concurrent `GET /api/runs` requests produced:

- **2 × `200`, 8 × `500`, maximum elapsed 5031 ms** — the pre-fix
  measurement was 8/10 client timeouts at a 10 s cap (`000`). The bounded
  acquire converts a pool wait into a fast structured error.
- `/api/health` stayed `200 ok` in 10 ms through an existing connection.
- Releasing the sessions restored health/runs; the chain verified.
- Limits: reserved superuser connections make the exact threshold fuzzy,
  and the 5 s bound is connection acquisition, not query execution.

## Ghost-run reconciliation after a web-process crash

The earlier crash probe left a `running` row after the web process was
killed, recoverable only by manual cancel. The fix adds an owner lease
(issue #1402): every run row records the inserting instance and a
`heartbeat_at`; a background task renews it every 10 s while the process
lives, and a reaper finalizes live rows whose owner has been silent for
90 s. When the orphan's workdir is reachable and carries the wrapper's
`.exit-code` record, the real outcome is used; otherwise the run is
`failed`. A process never reaps its own rows; terminal rows are never
selected.

Measured with two orphaned runs (a `sleep 600` still running and a
`sleep 20` that finished after the web died):

- `STATUS_BEFORE_KILL`: both `running`; web killed with `kill -9`.
- After 25 s the finished orphan's workdir held `.exit-code = 0`; the
  long-run workdir had no exit record.
- The restarted instance reported both still `running` immediately after
  restart (lease not expired), then **91 s after restart**:
  long-run → `failed`, finished orphan → `completed`.
- Two reaper log lines named the old owner
  (`unknown:984025:<uuid>`; the hostname field is `unknown` when the service
  environment does not export `HOSTNAME`), the last heartbeat, and the exit
  record (`None` / `Some(0)`).
- `/api/quota` after reconciliation: `active_runs: 0` (both durable
  reservations released); audit chain `ok: true`, 0 findings.

Limits: reconciliation takes about 90-120 s (the lease TTL plus the 30 s
sweep), not instant; the orphaned CLI is not signalled (it may keep running
after its ledger entry is finalized); a live owner partitioned from the
database for longer than the lease TTL can have its runs finalized by
another instance, and a later completion by the original process cannot
overwrite the first terminal write.

## Database crash during an uncommitted transaction

A PostgreSQL crash was induced with `systemctl kill -s SIGKILL
postgresql@16-main` while a transaction held an uncommitted `runs` row and
an uncommitted `audit_logs` row (`SELECT pg_sleep(30)` before `COMMIT`):

- `pg_stat_activity` showed the transaction `active`; both rows invisible
  before the crash (`TX_ROWS_VISIBLE_BEFORE_CRASH=0`).
- During the outage `/api/health` answered **4011 ms** `200 degraded`
  (bounded), and the unit was `failed`.
- On restart PostgreSQL logged
  `database system was not properly shut down; automatic recovery in
  progress`; the first health request after recovery was `200`.
- The web process was **not restarted** (same PID) — the pool reconnected.
- Both half-written rows were absent after recovery (`0` and `0`), so a
  crash cannot leave a ghost run row or an audit row without its chain
  event.
- The live run whose owner never died reached `completed`; the chain
  verified `ok: true`, 0 findings; the export hash is recorded in the raw
  log.

This is the kill-mid-transaction probe. Database **media corruption**
(truncated/edited data files or WAL) was not attempted and is residual: it
would test PostgreSQL's corruption handling rather than an oxo-flow code
path, and it risks the host's other databases.

## Earlier probes

These were recorded before the fixes and remain valid evidence for the
campaign matrix:

- **Full partition**: one instance behind `iptables OUTPUT DROP 5432`
  returned nothing for 10 concurrent run-list requests within 5 s, and
  `/api/health` answered after 30 s (the defect fixed above); removing the
  rule restored health/runs in 2 s.
- **Clean database restart**: `systemctl restart postgresql`; health and
  runs stayed 200, the 40-event chain verified, and the web process was not
  restarted (the pool reconnected). Limits: clean restart, not a crash.
- **Connection exhaustion**: with `max_connections=10` and six held
  sessions, 8/10 concurrent run-list requests hit a 10 s client cap and 2
  succeeded; health stayed 200. Re-measured after the fix above.
- **Multi-user team mode**: user created 3 runs, admin 1, viewer denied
  `403`; the user's list showed its own 3, the admin's all 4; the 13-event
  chain verified with distinct admin/user/viewer ids (the denied viewer
  create is audited).
- **Manual ghost recovery**: the restarted instance could cancel the
  ghost run with `POST /api/runs/{id}/cancel` (200, `cancelled`), which is
  what made the missing automatic reconciliation a defect rather than an
  outage.

## Defects fixed by this campaign

| Defect | Fix | Regression test |
| --- | --- | --- |
| `/api/health` took ~30 s under a database outage | 4 s bound on the health DB probe; the structured 200 `degraded` body is unchanged | `domains::observability::service::tests::database_probe_times_out_within_its_bound` |
| Pool waits could hold API requests 10-30 s | 5 s `acquire_timeout` on the PostgreSQL pool; requests answer `500 DB_ERROR` instead of hanging (SQLite keeps sqlx's default; the health probe bounds both) | Live probe 4 (pool options have no unit seam); pool construction is exercised by every web test |
| `kill -9` left a `running` ghost row forever | Owner lease (`owner_instance`, `heartbeat_at`), 10 s heartbeat, 90 s TTL, 30 s reaper; `.exit-code` attribution; quota release through the shared finalizer | `tests/run_lease_reaper.rs`; storage-contract assertions in `tests/postgres_storage_contract.rs`; unit tests in `infra::run_lease` |

The lease schema is added by both bootstraps (SQLite `ALTER TABLE`/rebuild
path and PostgreSQL `ADD COLUMN IF NOT EXISTS`) with a partial sweep index;
pre-lease rows have a NULL owner and are deliberately not reaped, so a
rolling upgrade cannot misclassify an old instance's live run.

## Residuals

- **Separate physical hosts were not available.** The only host is
  `tx-ubuntu`; multi-instance evidence is two processes on one host and two
  Linux network namespaces on that host with a TCP forwarder. A true
  multi-host fleet, its network, and its clock skew are unmeasured.
- **Network impairment is coarse.** Partitions are outbound `DROP` rules
  (full or per-namespace), not routing partitions, packet loss, latency
  injection, or a broken TCP proxy. The database is local.
- **Per-query network waits are still unbounded.** A request that already
  holds a connection to a black-holed server can block in TCP
  retransmission (`CREATE_PARTITION` hit a 15 s client cap). Health and
  pool acquisition are bounded; a client-side per-query timeout or
  `TCP_USER_TIMEOUT` tuning remains a follow-up.
- **Database media corruption was not attempted** (see above). The clean
  restart and the `SIGKILL` crash/WAL-recovery path are covered.
- **Concurrent schema bootstrap during a rolling upgrade was not probed.**
  Running the PostgreSQL smoke tests in parallel reproduced a lock-order
  deadlock: the init transaction's `CREATE INDEX ON audit_logs` waited on a
  runtime audit write holding `audit_logs`, while that write waited on
  `audit_chain` held by init. Production initializes the schema once before
  serving (the in-crate tests now do the same via a process-local
  `OnceCell`), so the measured probes are unaffected; a new instance
  initializing while old instances are writing has no production retry and
  remains unmeasured.
- **No production-scale measurement**: row counts, sustained load, a large
  pool, multi-instance admission on separate hosts, and long-running
  transactions are unmeasured.
- **Lease tradeoffs are deliberate**: a live owner that cannot reach the
  database for longer than the TTL may have its runs finalized by another
  instance (the later completion cannot overwrite the first terminal
  write); an orphaned CLI is not signalled; pre-lease rows (NULL owner) are
  not reaped; reconciliation is not instant (~90-120 s).
- **A terminal write that fails while the database is down is not
  retried.** If a run finishes during an outage and its owning web process
  stays alive, the row can remain `running` after recovery (the owner keeps
  heartbeating, so the reaper does not take it). This was not exercised by
  the final crash probe (the run finished after recovery); it is a known
  residual of the live-owner path.
- **No production-parity claim.** Single-host, single-database, one
  operator, one author; the evidence is a campaign, not a qualification.

## Reproduction

The host-local probe scripts are intentionally not committed (they create
and drop a disposable database, restart PostgreSQL, and install temporary
`iptables` rules). Their exact commands are named in
[`data/postgres-1402-campaign-2026-11-26.log`](data/postgres-1402-campaign-2026-11-26.log).
The checked-in regression tests (`tests/run_lease_reaper.rs`,
`tests/quota_durable.rs`, `tests/postgres_storage_contract.rs`, and the
health/lease unit tests) run without the live host; the PostgreSQL storage
contract gates on `OXO_FLOW_TEST_POSTGRES_URL`.
