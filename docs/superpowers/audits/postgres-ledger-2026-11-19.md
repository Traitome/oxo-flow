# Unified run-ledger storage abstraction — live PostgreSQL measurement (#1251)

> **Status**: measurement evidence for ROADMAP Stage 12.1 row *"Unified run-ledger
> storage abstraction"*. This is the evidence the row's exit criterion names:
> **live PostgreSQL parity, concurrency, crash recovery, and horizontal-instance
> tests**. It measures the guarded run-ledger admission path on a real
> `postgres:16` server. It is **not** a production-scale campaign and makes no
> such claim (see §7).

## 1. What was measured

The run ledger routes every run insert through the unified `StorageBackend`
trait, whose PostgreSQL implementation (`crates/oxo-flow-web/src/infra/db/postgres.rs`)
admits a run atomically with

```sql
INSERT INTO runs (...) SELECT {...}
WHERE NOT EXISTS (SELECT 1 FROM runs
                  WHERE {guard_column} = {guard_value} AND status IN ('queued','running','paused'))
ON CONFLICT DO NOTHING
```

(relying on the partial unique indexes `idx_runs_live_workdir` /
`idx_runs_live_pipeline`), and returns `rows_affected() == 1` as the win flag.
The existing `postgres_storage_contract.rs` and the in-crate smoke test pin
schema bootstrap and CRUD and exercise only a **2-way** race. This audit adds
the three behaviors the exit criterion names, at N-way scale:

- **M1 Concurrency** — N concurrent guarded inserts on the **same** workdir key
  (`create_run_if_workdir_idle`) must admit **exactly one**, never error, and
  leave exactly one row on disk (no ghost/orphan); the pool must remain usable
  afterwards (no deadlock/hang).
- **M2 Crash recovery** — a writer opens a transaction, inserts a queued run,
  and is killed mid-transaction (`pg_terminate_backend`, the in-process analog
  of a SIGKILL / dropped socket). The uncommitted row must **not** survive; a
  fresh independent connection must read consistent state. Durable counterpart:
  a **committed** admission IS visible to a fresh connection.
- **M3 Horizontal instances** — two independent `PostgresBackend` values
  (separate pools, one database) must not double-claim the guarded key, and
  must agree on pagination totals and page contents.

Plus a **non-vacuity control (C0)**: with N *distinct* guard keys the same
apparatus must observe N winners — if it could only ever print `1`, M1's
"exactly one" would be a constant, not a measurement.

## 2. Host and version identity (ledger §8)

| Field | Value |
| :--- | :--- |
| hostname | `VM-4-4-ubuntu` |
| `uname -a` | `Linux VM-4-4-ubuntu 6.8.0-137-generic #137-Ubuntu SMP PREEMPT_DYNAMIC … x86_64 GNU/Linux` |
| arch | `x86_64` |
| CPUs | 4 (`nproc`) |
| RAM | 3.6 GiB total |
| PostgreSQL server | `PostgreSQL 16.16 (Debian) on x86_64-pc-linux-gnu` (container `postgres:16`) |
| baseline commit | `1face885104d24af95b213d52b90ed68373ab957` (`origin/main` at branch point) |
| measured head | `923ea6a4606087cbbc91f08abd74e288a5f01a8f` (this PR's head — harness/test/script live here) |
| branch | `research/1251-postgres-ledger` |
| oxo-flow | `0.32.0` |
| rustc / cargo | `1.98.0` / `1.98.0` |
| date (UTC) | 2026-11-19 |

## 3. Exact command

```bash
# stands up a disposable postgres:16, points the tests at it, prints raw numbers
scripts/postgres-ledger-campaign.sh --concurrency 16
scripts/postgres-ledger-campaign.sh --concurrency 32
```

The harness is **report-only by default (always exits 0)**; `--strict` makes it
exit non-zero on failure. It captures the test status as `rc=0; … || rc=$?` so
`set -euo pipefail` cannot abort before the `--strict` check (mirrors
`scripts/import-equivalence.sh`). It cannot run vacuously: it resolves either a
reachable `OXO_FLOW_TEST_POSTGRES_URL` or a working container runtime to create
one, and **fails hard if neither exists**. The tests print one
`measurement=…` line per run for collection. With `OXO_FLOW_TEST_POSTGRES_URL`
unset the test file itself skips cleanly (same contract as the existing
contract test).

## 4. Pre-registered acceptance criterion

**Registered before the runs:**

> **M1** admits exactly `1`, with `0` errors and exactly `1` row on disk, and
> the pool is usable afterwards. **C0** admits exactly `N`. **M2** leaves `0`
> ghost rows and shows `1` committed row visible to a fresh connection.
> **M3** admits exactly `1` workdir winner and exactly `1` pipeline claim across
> the two instances, and both instances return the same run total and page set.
> A single violation is a **FAIL**. The measurements are **non-vacuous** iff an
> injected fault inverts the verdict (see §5).

## 5. Results

### 5.1 Measurements (raw `measurement=` lines)

| Run | line |
| :--- | :--- |
| N=16 | `measurement=concurrency n=16 admitted=1 live_rows=1 errors=0 result=PASS` |
| N=16 | `measurement=crash_recovery killed_backend=true ghost_rows=0 committed_visible=1 result=PASS` |
| N=16 | `measurement=negative_control n=16 admitted=16 result=PASS` |
| N=16 | `measurement=horizontal_instances n=16 workdir_admitted=1 pipeline_claims=1 page_total=9 instances=2 result=PASS` |
| N=32 | `measurement=concurrency n=32 admitted=1 live_rows=1 errors=0 result=PASS` |
| N=32 | `measurement=crash_recovery killed_backend=true ghost_rows=0 committed_visible=1 result=PASS` |
| N=32 | `measurement=negative_control n=32 admitted=32 result=PASS` |
| N=32 | `measurement=horizontal_instances n=32 workdir_admitted=1 pipeline_claims=1 page_total=9 instances=2 result=PASS` |

Both runs: `test result: ok. 4 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out` (exit 0).
**Headline: M1 exactly-one (16 and 32), M2 zero ghost rows, M3 single shared
claim — every result PASS.** No defect (double-claim, ghost row, deadlock) was
observed.

### 5.2 Non-vacuity (fail-first)

Each measurement was shown to be able to **fail** by injecting the fault it is
meant to catch, in a throwaway probe (deleted after use; not shipped):

| Probe | Injected fault | Observed | Verdict |
| :--- | :--- | :--- | :--- |
| crash-recovery | writer `COMMIT`s **before** the kill | `ghost_row_present=true`, assertion fires, test `FAILED` (exit 101) | M2 is non-vacuous — a durable row is detected |
| concurrency | `DROP INDEX idx_runs_live_workdir` before the race | `admitted=2` (>1), and restoring the index then fails with `could not create unique index … is duplicated` | M1 is non-vacuous — removing atomicity admits multiple winners |

The negative control (C0) is the shipped, always-on form of the concurrency
non-vacuity: it proves the apparatus observes `N > 1` winners when keys differ.

## 6. Off-criterion observations

- **N-way scale.** N=16 and N=32 were run; the insert is a single statement
  against one row's guard, so no larger N exercises new code paths on
  PostgreSQL (contention is resolved by the index, not by a loop). N is a
  smoke-scale value, not a production-concurrency figure (§7).
- **Env-var naming.** The new test gates on `OXO_FLOW_TEST_POSTGRES_URL`
  (matching the existing contract test). The CI smoke test
  (`.github/workflows/ci.yml`) and the in-crate `postgres.rs` unit tests gate on
  `OXO_PG_TEST_URL`. The two names are independent; this file does not change
  that.
- **Restoring the guard index** after the fail-first probe required marking the
  injected duplicates terminal first — the partial index is `WHERE status IN
  ('queued','running','paused')`, exactly as designed. This is expected
  behavior, not a finding.

## 7. Honesty boundary

**What this is.** A live-PostgreSQL measurement of the guarded run-ledger
admission path: N-way concurrency, crash recovery (writer killed
mid-transaction), and two-instance sharing, on one `postgres:16` server.

**What this is NOT, stated loudly.**

- **Single-host, loopback confound.** The PostgreSQL server is a container on
  the **same host** as the test process, reached over `127.0.0.1`. Every
  measurement is loopback IPC. **Network partition, real latency, and a
  genuinely remote server are NOT exercised.**
- **Not production-scale.** N=16/32 on a 4-core host with a handful of rows.
  **Production row counts, large tables, and index degradation are NOT
  measured.** The `page_total=9` pagination check is correctness, not scale.
- **This run measures PostgreSQL only.** The behavioral parity of the SQLite
  backend's equivalent guard was **not** re-measured here. "Parity" in the
  ROADMAP row refers to the PostgreSQL path matching the documented
  `StorageBackend` contract (already pinned by
  `postgres_storage_contract.rs`), not to a fresh SQLite-vs-PostgreSQL
  differential on this ledger. **Follow-up (issue #1312,
  `tests/admission_backend_parity.rs`):** the SQLite-vs-PostgreSQL differential
  this section names now exists — one shared scenario driver (workdir/pipeline
  serialization, concurrent double-claim, terminal release, `Ok(false)`
  rejection) is run through the production `claim_run_row` helper against both
  backends and each backend's verdict vector is compared to one canonical
  contract. It is a same-host, single-process verdict comparison; it does not
  supersede the N-way concurrency, crash-recovery, or two-instance measurements
  above, and it does **not** close the pool-exhaustion / network-partition /
  multi-instance gaps below.
- **Pool exhaustion NOT exercised.** The concurrency run stays within the pool's
  connection budget; saturation/timeout behavior under a tiny pool is **NOT**
  measured. (Only "the pool still hands out a connection afterwards" is
  checked.)
- **Crash model is in-process.** The writer is killed with
  `pg_terminate_backend` / a dropped socket, **not** a SIGKILL of the whole
  process, and **not** a power cut. It reproduces the server-side abort of an
  uncommitted transaction; it does **not** model host loss.
- **No code change.** This PR adds measurement (a test + harness + this doc).
  It does not restructure production code and closes no defect — none was
  found.

**What would move it forward**: a remote PostgreSQL, a large seeded table for
index/plan evidence, pool-saturation and network-partition injection. (The
SQLite-vs-PostgreSQL differential on the same guard this list once named now
exists — `tests/admission_backend_parity.rs`, issue #1312 — but it is a
same-host verdict comparison, not a remote/scale measurement.)

## 8. Raw-data lineage

| Artifact | Path |
| :--- | :--- |
| Measurement test | `crates/oxo-flow-web/tests/postgres_concurrency.rs` |
| Harness (bash) | `scripts/postgres-ledger-campaign.sh` |
| Guard implementation | `crates/oxo-flow-web/src/infra/db/postgres.rs` (`insert_run_guarded`, partial indexes) |
| Contract baseline | `crates/oxo-flow-web/tests/postgres_storage_contract.rs` |
| Backend-parity differential | `crates/oxo-flow-web/tests/admission_backend_parity.rs` (issue #1312) |
| baseline commit | `1face885104d24af95b213d52b90ed68373ab957` (`origin/main` at branch point) |
| Repo commit measured | `923ea6a4606087cbbc91f08abd74e288a5f01a8f` (this PR's head — harness/test/script live here) |
| Host | `VM-4-4-ubuntu`, 4 CPU, 3.6 GiB RAM, `postgres:16` (16.16) container |
