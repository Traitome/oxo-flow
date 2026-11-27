# Web System Architecture

oxo-flow-web is the web control plane for oxo-flow — not a thin wrapper around the CLI.

---

## Design Principles

### 1. AI-Native API Design

All endpoints are designed to be consumable by both AI agents and browsers:

- Unified structured responses
- Long-running operations streamed via SSE
- Self-discoverable via `GET /api/openapi.json`
- Errors carry `code` + `message`, plus optional `detail` and `suggestion` fields

### 2. Intent-First Pipeline Authoring

Users describe *what* they want ("run standard RNA-seq differential expression on this cohort"), and the system generates a validated pipeline. The TOML format becomes the compilation target, not the authoring surface.

### 3. DAG as the Primary Interaction Surface

An interactive visual graph where users can drag nodes, connect inputs to outputs, and inspect environment/resource configuration. The graph *is* the workspace.

### 4. Results as Queryable Data

Pipeline outputs are not just files — they are structured data that can be queried, compared, and visualized via the API.

### 5. Shareable, Forkable, Citable Pipelines

A pipeline is a URL, not a file. Share it, fork it. Each run has a permanent provenance URL.

### 6. HPC/Cloud as a Detail

The execution backend is a dropdown — local, HPC queue, Docker, cloud batch. Everything else (workspace setup, file staging, environment resolution) happens transparently.

---

## Architecture Overview

The web crate follows a **domain-driven modular monolith** pattern. Each domain has:

- `types.rs` — request/response structs
- `service.rs` — pure logic (zero HTTP dependency)
- `handlers.rs` — HTTP → service adapters

```
+-----------------------------------------------+
|              Web Client (SPA)                 |
|     Interactive DAG Editor / Dashboard        |
+------------------+----------------------------+
                   | HTTP/SSE
+------------------v----------------------------+
|          oxo-flow-web (Axum Server)           |
|                                                |
|  +---------+ +---------+ +----------------+  |
|  | Auth    | |License  | | Observability  |  |
|  | OAuth2  | |Headers  | | Health/Metrics |  |
|  +---------+ +---------+ +----------------+  |
|  +---------+ +---------+ +----------------+  |
|  |Workflow | |Execution| | AI/Translate   |  |
|  |Pipeline | |Diagnose | | Explain/Interp |  |
|  +---------+ +---------+ +----------------+  |
|  +---------+ +---------+ +----------------+  |
|  |Collabor-| |  Data   | | HPC            |  |
|  |ation    | | Discovery| | Scheduler      |  |
|  +---------+ +---------+ +----------------+  |
|  +-------------------------------------------+|
|  |           Middleware Stack                 | |
|  |  (outermost first) Compression -> Trace ->| |
|  |  CORS -> RateLimit -> SecurityHeaders ->  | |
|  |  LicenseHeader                            | |
|  +-------------------------------------------+|
|  +-------------------------------------------+|
|  | StorageBackend trait (SQLite + PostgreSQL)| |
|  +-------------------------------------------+|
+------------------+----------------------------+
                   | API calls
+------------------v----------------------------+
|          oxo-flow-core (Engine)               |
|  DAG . Executor . Environment . Wildcards    |
+-----------------------------------------------+
```

### Middleware & Runtime Behavior

Applied outermost-first in `server.rs::build_router`:

- **Compression** — tower-http `CompressionLayer` (brotli + gzip). The
  default predicate skips `text/event-stream`, so the SSE endpoints
  (`/api/events`, `/api/ai/translate/stream`) stream uncompressed.
- **Tracing** — `TraceLayer` emits an INFO `http.request` span plus a
  completion log (status, latency) per request; `/api/health` runs at DEBUG
  because the dashboard polls it every 30 seconds (`STATUS_POLL_MS` in
  `Layout.tsx`).
- **CORS → Rate limit → Security headers → License header** — the rate
  limiter sits inside the `Extension` layer so it can see the shared
  limiter state.

Static serving and caching:

- `/assets/*` holds Vite content-hashed files, served with
  `Cache-Control: public, max-age=31536000, immutable`; `index.html` stays
  `no-cache` so new deploys are picked up immediately.
- The SPA `index.html` (with `<base>`/base-path injection) is computed once
  per process and cached in memory, as is the serialized OpenAPI document
  behind `GET /api/openapi.json`.

Async-runtime hygiene:

- Run-log reads (logs, diagnostics, preview, finalize) and the log tailer
  use `tokio::fs`; `metrics.jsonl` appends are O(1) with a truncate-rewrite
  only past the 2000-line cap.
- bcrypt password hash/verify runs via `tokio::task::block_in_place` so it
  does not block the async worker.
- `GET /api/metrics` reads a shared persistent `sysinfo::System` with
  targeted CPU/memory refreshes (real CPU deltas; memory fields are true
  MB, matching their `_mb` names).

Rate limiting and SSE:

- The rate limiter is a sliding window (default 100 requests / 60 s) keyed
  by client IP: the transport peer address by default (spoofable
  `X-Forwarded-For`/`X-Real-IP` headers are ignored unless
  `OXO_FLOW_TRUSTED_PROXY` marks the deployment as behind a controlled
  reverse proxy, in which case the forwarded headers are honoured).
  Over-limit
  requests get `429` with the standard structured error body
  (`{"code":"RATE_LIMITED", ...}`) and a `Retry-After` header; keys whose
  timestamps have all expired are evicted opportunistically (every 1024
  checks, no background task).
- `/api/events` relies solely on axum's 15-second `KeepAlive` comment ping.
  If a subscriber falls behind the 100-slot broadcast buffer, the server
  emits a synthetic `{"type":"lagged","time":"…","data":{"missed":N}}`
  event so the client can refetch instead of silently missing run state
  transitions.

Share URL truth:

- The server has no native TLS, so minted share URLs use `oxo+http://`
  with the actual bind host/port (plus the `--base-path` prefix) by
  default — a hardcoded `oxo+https://` advertised TLS the process never
  spoke. IPv6 host literals are bracketed so the URL stays parseable.
- `OXO_FLOW_PUBLIC_URL` (e.g. `https://flows.example.com`) lets a
  reverse-proxy deployment declare its public origin; scheme and
  authority of minted URLs then come from that override. A value that
  also carries the mount path (`https://flows.example.com/oxo-flow`, the
  sub-path deploy recipe) replaces the local `--base-path` prefix
  instead of being appended to it; a scheme-less value still mints
  `oxo+http://` but logs a warning; an empty value warns and is treated
  as unset; a value without any host (including port-only `:8080`) is
  ignored with a warning and a local URL is minted instead; IPv6
  literals on the override path are bracketed. Import accepts both
  schemes, so older `oxo+https://` links keep working.

### Operator variables and internal/test variables

The environment variables documented below and in the deployment guides are operator-facing configuration knobs. Some variables exist only for test harnesses or internal process coordination (for example `OXO_FLOW_BACKGROUND_PID_FILE`, `OXO_FLOW_TRACK_PGID`, `OXO_FLOW_STDERR_ALREADY_REDIRECTED`, and names prefixed `OXO_FLOW_TEST_*`). Internal/test names are implementation details, are not a supported public configuration contract, and may change without operator-facing release guarantees. Operators should use the documented variables only; test harnesses own and pin their internal names.

Server process environment:

- `OXO_FLOW_BIN` — explicit path to the `oxo-flow` CLI binary the server
  spawns for run execution. It is the first of the binary search steps
  (env override → `CARGO_BIN_EXE_oxo-flow` → sibling of the running
  executable → parent directory → `PATH`), so a stale `PATH` entry can
  no longer silently win over a deliberate pin; the resolved path is
  logged at startup (from which the source is inferable) and a major/minor
  mismatch between the server and the spawned CLI is warned.
- `OXO_FLOW_WORKSPACE` — root directory for all user-scoped server data
  (`users/<u>/inputs/`, `users/<u>/runs/<run_id>/`; a legacy
  `users/<u>/pipelines/<pipeline_id>/` tree from pre-#1130 releases is no
  longer written). Defaults to a relative `workspace/` under the process
  cwd; container deployments should point it at a persistent volume. Path
  components are traversal-validated before use.

### Run workdir lifecycle

Every run created through `POST /api/runs` owns `users/<u>/runs/<run_id>/`
for its whole life. The workflow snapshot, staged `include_files`, uploaded
inputs, `.oxo-flow/checkpoint.json`, logs, and outputs are written there and
nowhere else; the directory name is the run id, so no other run can address
it. `retry` and `resume-checkpoint` are explicit continuations of a
predecessor's lineage and deliberately write into that predecessor's tree;
they are not new run history. A saved pipeline's `pipeline_id`
is an association (permissions, listing, admission ordering), never a
workdir. This is what makes a run's recorded checksums and run history
immutable (issue #1130 / audit RB-02). Admission is a single guarded
`INSERT ... SELECT ... WHERE NOT EXISTS ... ON CONFLICT DO NOTHING` keyed on
`pipeline_id` for saved pipelines and on `workdir` for ad-hoc runs, backed by
partial unique indexes (`idx_runs_live_pipeline`, `idx_runs_live_workdir`) on
both SQLite and PostgreSQL. The indexes are the guarantee PostgreSQL needs:
under READ COMMITTED two concurrent transactions can both pass `NOT EXISTS`,
and only `ON CONFLICT` decides the loser (zero rows affected → the documented
`409 RUN_NOT_TERMINAL`). The saved-pipeline slot is global to the pipeline,
not per user, so a `workspace`-shared pipeline admits one live run across all
its readers. `retry` and `resume-checkpoint` are the only paths that
deliberately continue a predecessor's workdir.

Backend-equivalent admission is now asserted, not assumed (`tests/admission_backend_parity.rs`, issue #1312): one shared scenario driver runs the SAME claims through the SAME `claim_run_row` helper against BOTH backends and compares each backend's verdict vector to one canonical contract (workdir-serialized claim; pipeline-serialized claim; concurrent double-claim admits exactly one; a terminal predecessor releases the guard; a zero-affected-rows rejection is `Ok(false)` — the exact input the retry/resume arms map to `409 RUN_NOT_TERMINAL` — never a storage error). The SQLite arm runs unconditionally; the PostgreSQL arm gates on `OXO_PG_TEST_URL` and skips when unset. This proves backend-equal admission VERDICTS for these scenarios in a single process on a single host. The 2026-11-26 #1402 campaign then measured the operational dimensions on one host: two instances on that host and in separate Linux network namespaces shared admission and the audit chain, a full and an asymmetric outbound partition were survived, connection exhaustion produced bounded structured errors, a clean restart and a `SIGKILL` crash during an uncommitted transaction both recovered, and per-user quota was isolated ([campaign report](../../../superpowers/audits/postgres-two-instance-2026-11-26.md)). Separate physical hosts, production-scale load, per-query network timeouts, database media corruption, and cross-host clock skew remain unmeasured, so no production-parity claim follows.

Runs also carry an **owner lease** (issue #1402): the inserting web process
records `owner_instance` and `heartbeat_at`. The executor guard
(`RunMonitorGuard`) is created at claim time in the request handler —
before pre-spawn materialization and input staging, which can outlast the
lease — and moved into the local spawn task, the SQLite re-attach, or the
remote dispatch; the heartbeat renews every 10 s only for runs with a
registered guard, so a queued row left by a failed pre-spawn step is not
renewed. A 30 s reaper finalizes a live run whose owner has been silent for
90 s, attributing the wrapper's `.exit-code` record read-only: only when it
postdates this run's `started_at` (`completed` for 0), and only for a
`running` row (`failed` otherwise). The record is removed only after a
terminal write succeeds (or the row is already terminal), so a transient
write failure is retried on the next sweep with the outcome preserved.
Every local spawn clears a predecessor's record first, and remote dispatch
clears it too before staging, so no predecessor record survives that
window; the completion pull then brings back the remote run's own record,
whose mtime comes from the submit host's clock and is therefore not a
reliable freshness signal under cross-host clock skew (a documented
residual). This is the automatic
reconciliation for the measured `kill -9` ghost row. A process never reaps
a row it is still monitoring, but it does reap its own unmonitored rows (a
failed pre-spawn step or a failed terminal write), so a single-instance
deployment recovers without a second server; terminal rows are never
selected, and rows written before the lease migration (NULL owner) are
skipped so a rolling upgrade cannot misclassify an old instance's live run.
Deliberate tradeoffs: the orphaned CLI is not signalled; a live owner that
cannot reach the database for longer than the TTL can be finalized by
another instance (the first terminal write wins); lease comparisons assume
loosely synchronized clocks — a host off by more than half the TTL can reap
early, and the campaign measured one host only; and a reaper finalization
is a system reconciliation with no audit row (the chain records API
mutations by an acting user).

SQLite serving assumes one web process per database file: its startup
recovery force-adopts live rows from the previous process, so two SQLite
web processes on the same file would fight over ownership. PostgreSQL is
the multi-instance backend.

Nothing reclaims those trees automatically: a long-lived server accumulates one
`users/<u>/runs/<run_id>/` directory per run forever. The manual sweeper is
[`oxo-flow runs gc`](../commands/runs.md), which deletes only terminal-run
trees past an age gate, never a live run, never a tree a later retry/resume
still points at, and never a tree carrying a declared protected output — the
shared `workdir` string across run rows is exactly the lineage signal it keys
on.

---

## UI Test Coverage

The SPA's published surfaces are exercised by Playwright specs in
`frontend/e2e/` against the real web server. Individual specs stub the
specific endpoints they need to force — AI provider/chat, pipeline
validation, auth, and file download — while the core-flow, mobile, WCAG and
Metro specs drive the real server. `make frontend-test` runs the suite
locally; the CI `e2e` job runs the same `npx playwright test` with no AI
provider environment (so the AI-disabled Dashboard/Settings states are the
ones gated) after asserting the served bundle is the freshly built artifact
(issue #555).

| Surface | Spec | Asserted contract |
| :--- | :--- | :--- |
| Browser E2E: author → validate → run → monitor | `core-flows.spec.ts` | Editor renders TOML + DAG, the validation badge reflects a real `POST /api/pipelines/validate`, Save creates a pipeline, and the Stage 4/5 case creates a real run, then verifies provenance/checksums and the output preview in the monitor/report. |
| Mobile viewport | `mobile-viewport.spec.ts` | At 390×844, routed content does not overflow: `.main-content.scrollWidth <= clientWidth + 1` on the dashboard, pipelines, editor, runs, monitor and settings routes (the document-level check alone is vacuous because `body`/`.main-content` clip `overflow-x`). A negative-control test injects a 2000px child and proves the detector reports it. The navigation drawer reaches the editor and an enabled Run button opens the run dialog with its close control inside the viewport; the React Flow canvas does not widen the routed content. |
| WCAG / accessibility | `accessibility.spec.ts` | axe-core scans the six primary routes in both color themes, the canvas editor mode (`/editor` with `oxo_editor_mode=canvas`: the React Flow rule cards and the CodeMirror TOML pane) in both themes, plus three mobile-viewport scans (drawer closed in both themes and open) with the WCAG 2.0/2.1 A + AA tag set; any violation fails the spec. No axe rule is disabled — the findings this gate surfaced were fixed in the UI (including the canvas rule-card `label-content-name-mismatch` and the dark TOML syntax contrast, issue #1223). CI runs with no AI provider, so the AI-disabled dashboard/settings states are gated; both those and the configured variants pass. The scanned routes are documented in the spec header. |
| Metro frame-time budget | `metro-performance.spec.ts` | A 6-rule workflow is switched Rule → Process → Module (a Sugiyama re-layout per switch) and the interaction-to-next-paint latency is measured with `performance.now` plus two `requestAnimationFrame`s after the click. Station counts prove the switch regrouped (Rule = 6, Process = 1); worst sample < 750 ms, median < 300 ms. |
| Keyboard graph navigation | `metro-canvas.spec.ts` | Metro station keyboard access is owned by issue #1148 / ledger UX-06; that spec ships with the keyboard-access change. It is not duplicated here. |
| Desktop (wry/tao) lifecycle | `scripts/desktop-smoke.sh` (CI `desktop` job, `make desktop-smoke`) | Build + launch against an isolated `HOME`, the embedded server creates its lock + SQLite database, a second instance is refused with the documented message, and SIGTERM exits 0. See `docs/guide/src/how-to/desktop-app.md` → Verification. |

Deliberately not covered by this suite: visual pixel rendering, real-device
mobile gestures, screen-reader output beyond what axe-core's rule set
evaluates, and the Windows desktop lifecycle (the release pipeline builds no
Windows bundle and the smoke test is POSIX-only). The screen-reader gap is
not incidental: axe-core checks the accessibility tree a screen reader
*consumes*, but it does not run one, so the Metro keyboard path and the
mobile drawer's announced output remain **manual and unmeasured** — no
NVDA/JAWS/VoiceOver/TalkBack pass has been performed. Windows desktop is
source-compatible but unbuilt and untested by CI; see
`docs/guide/src/how-to/desktop-app.md` → Windows.

## Domain-Driven Module Structure

| Domain | Path | Responsibility |
|--------|------|---------------|
| **workflow** | `domains/workflow/` | Pipeline parse, validate, prepare, DAG, format, lint, stats, diff, export, search, data discovery, plugin validation |
| **execution** | `domains/execution/` | Run create/status/cancel/retry, diagnostics engine (31 error patterns), sandbox workspace; runs delegate to the `oxo-flow` CLI subprocess, which owns checkpointing and invalidation |
| **ai** | `domains/ai/` | AI translate, explain, interpret, optimize; generation runs the shared agent + orchestrator harness (see [Architecture → AI Subsystem](architecture.md)) over the provider fallback chain |
| **auth** | `domains/auth/` | Login, session management, ORCID/GitHub OAuth2, RBAC |
| **chat** | `domains/chat/` | Real-time AI copilot chat and streaming; generation uses the same harness as translate, plus read-only run-diagnosis tools |
| **clusters** | `domains/clusters/` | Remote cluster submission over SSH, job polling, result pull-back |
| **collaboration** | `domains/collaboration/` | Fork, diff, share, import pipelines |
| **dag** | `domains/dag/` | DAG-specific types and operations |
| **observability** | `domains/observability/` | Health check, system info, runtime metrics, structured logging (3-layer), audit, SSE |
| **infra/db** | `infra/db/` | StorageBackend trait with SQLite and PostgreSQL implementations; migrated metadata surfaces and the startup PostgreSQL capability matrix |
| **infra/license** | `infra/license.rs` | License notice text, banner, X-OxoFlow-License header middleware (the web UI footer label lives in the SPA, `frontend/src/components/Layout.tsx`) |
| **infra/sse** | `src/sse.rs` | Real-time SSE broadcast channel for execution events |
| **hpc** | core `cluster.rs::generate_submit_script` (`crates/oxo-flow-core/src/cluster.rs`) | Slurm submit-script generation; scheduler detection stays in the web crate |

### Durable runtime state

Quota state — per-user counters, per-run resource reservations, and the
limits row — lives in the storage backend (`quota_usage`,
`quota_reservations`, `quota_config`), not in process memory. Admission is
one atomic compare-and-swap plus reservation insert, so concurrent requests
serialize on the database, restarts preserve usage, and multiple server
instances sharing a database enforce the same counters and limits (RB-09 /
issue #1133). Locally executed runs on Linux additionally enforce the
workflow's declared memory with `RLIMIT_DATA` on the spawned CLI process
tree; other platforms and remote (cluster) runs remain advisory — see
[Quota → Declared-resource enforcement](web-api.md#declared-resource-enforcement).

### Hash-chained audit trail (issue #1372)

The audit trail is two layers with different mutability:

- `audit_logs` — the mutable listing surface (`GET /api/audit`, the Audit
  page). Rows are written ahead of a mutation, finalized after it, and
  cascade-deleted when their user account is deleted.
- `audit_chain` + `audit_chain_head` — an append-only SHA-256 chain. Every
  audit write (pending intent, terminal finalize, legacy post-handler row)
  appends one immutable event carrying the complete canonical record, in the
  same transaction as the `audit_logs` write. Each event hashes its fixed
  field set and names its predecessor's hash; the single head row names the
  chain's end (genesis = seq 0, 64 zero hex digits). No code path updates or
  deletes a chain row; a database-level edit is detected by verification
  rather than prevented (see the limits below).

Append serialization is backend-native: SQLite opens `BEGIN IMMEDIATE` (the
write lock is taken before the head is read), PostgreSQL takes `SELECT …
FOR UPDATE` on the head row as the first statement of every chain write. Both
orders are identical across `begin`/`finalize`/`record`, so appends cannot
deadlock and cannot produce a gap or duplicate `seq`. That is true **by
construction** — the write lock / row lock is the serialization point. The
measured PostgreSQL evidence is one host: two instances on that host and two
instances in separate Linux network namespaces produced byte-identical
40- and 70-event exports; an asymmetric partition and a `SIGKILL` crash
during an uncommitted transaction both left a verifiable chain; and a
connection-exhaustion run verified ([campaign
report](../../../superpowers/audits/postgres-two-instance-2026-11-26.md)).
Separate physical hosts, remote/networked databases, production scale,
database media corruption, and cross-host clock skew remain unmeasured.
(SQLite serializes processes sharing the file on one host; a SQLite file on a
network filesystem is not a supported deployment.) A finalize is idempotent:
the `(audit_id, phase)` unique index admits one terminal event, a repeated
finalize is a no-op, and the first terminal outcome wins over a conflicting
later one (documented behavior, not an error). A snapshot
(`audit_chain_snapshot`) reads the head, events, and `audit_logs` in one
transaction — SQLite's WAL read snapshot is stable; PostgreSQL `FOR SHARE` on
the head makes chain appenders wait — so an export is consistent with respect
to chain appends. It is not a fully serialized read: on PostgreSQL READ
COMMITTED each plain `SELECT` takes a fresh snapshot, so a concurrent user
deletion (the `users` → `audit_logs` cascade, the only writer that changes log
rows without the head lock) can commit between the event and log reads and
surface a transient `MISSING_LOG_ROW` for an untampered chain.

Exports are canonical (`oxo-flow.audit-chain/v1`: sorted keys, canonical
metadata JSON, fixed `YYYY-MM-DDTHH:MM:SS.ffffffZ` timestamps for new rows)
and verify offline with `oxo-flow audit verify` (exit 0 valid, 2
invalid/tampered, 4 malformed with `AUDIT_EXPORT_INVALID`). The web
`GET /api/audit/chain/verify` answers 200 with `ok:false` findings for
deletion, reorder, insertion, mutation, duplicate phase, bad genesis, and
truncation.

Limits, stated plainly: the chain is not a signature and is not external
notarization. An actor who can rewrite the whole chain and the head
consistently — including deleting every event and resetting the head to
genesis — passes local verification; a previously exported document is what
exposes a later wholesale rewrite. Audit-row cascade deletes surface as
`MISSING_LOG_ROW`, by design. The chain does not cover `audit_logs` rows that
predate the upgrade (the migration does not backfill), and it proves nothing
about the truthfulness of an event the server itself wrote. A structurally
broken chain — a missing or deleted `audit_chain_head` row — is deliberately
not a 200 finding: the snapshot cannot be represented, both endpoints answer
`500 DB_ERROR` (fail-closed; every audited write also fails in that state),
and an export without a head is malformed input (exit 4) to the offline
verifier.

### Tenant execution boundary (RB-01)

Every locally executed run spawns through one choke point
(`spawn_background_run_with_args`), which layers five independent OS-level
controls on the CLI process tree (and therefore every rule shell it forks):

1. **Environment allowlist (always on, all platforms, RB-01a)** — the child
   is rebuilt from the engine's rule-shell policy; server secrets never
   ride along.
2. **Resource ceilings (Linux, opt-in)** — `RLIMIT_DATA` from the declared
   memory, plus operator-wide `RLIMIT_CPU`/`RLIMIT_NPROC` knobs. The
   ceilings are installed on the wrapper and inherited through every
   isolation layer below.
3. **Kernel filesystem/network sandbox (Linux 5.13+, opt-in)** —
   `OXO_FLOW_WEB_SANDBOX=landlock` installs a Landlock ruleset in a
   `pre_exec` hook: the spawned tree can read+execute system dirs and the
   engine binary and write only the run workdir, the per-run scratch dir,
   and the operator's engine dirs; `/proc`, `/sys`, `/etc`, the server's
   `$HOME`, the workspace root, and other tenants' workdirs are denied at
   the kernel level; TCP bind/connect is denied on kernel 6.7+ (ABI v4),
   abstract-UNIX-socket and signal scoping on 6.10+/6.12+ (ABI v5/v6).
   Fail-closed: an unhonorable sandbox request refuses the spawn.
4. **User + PID namespace (Linux, opt-in, local runs, issue #1401)** —
   `OXO_FLOW_WEB_ISOLATION=userns` spawns the CLI through
   `unshare --user --map-root-user --pid --fork --mount-proc --kill-child=SIGTERM`:
   the run is root inside its own user namespace (the server uid outside, so
   workdir ownership is unchanged), is PID 1 of a private PID namespace with
   a private `/proc`, and can neither see nor signal host or sibling-tenant
   processes (host-side signals still reach it — that is how cancel works). Availability is probed before use (AppArmor/sysctl host
   prerequisites) and a requested-but-unavailable mode fails the spawn —
   never a silent downgrade. Mutually exclusive with the Landlock mode:
   measured on a Landlock ABI 4 host, the `unshare` map write targets the
   dynamic `/proc/<pid>` of a forked child, which a pre-exec static ruleset
   cannot grant without re-exposing host `/proc`.
5. **Process-group bookkeeping (always on)** — the run owns a process
   group, so cancel/pause/resume signal the whole tree; with a namespace,
   the group SIGTERM reaches `sh`, `unshare`, and the namespace init
   directly (a CLI that traps it shuts down gracefully), while
   `--kill-child=SIGTERM` is the backstop if the `unshare` parent itself
   terminates, and the 5 s SIGKILL escalation tears the namespace down when
   the init ignores SIGTERM.

The implementation lives in `crates/oxo-flow-web/src/executor.rs` (the spawn
choke point and the rlimit `pre_exec` hooks), `crates/oxo-flow-web/src/sandbox.rs`
(Landlock), and `crates/oxo-flow-web/src/userns.rs` (namespaces); the
enforced-vs-residual matrix, the knob reference, and the two-tenant
adversarial evidence (`rb01_sandbox_adversarial.rs`,
`rb01_userns_adversarial.rs`) are in
[Create Run → Tenant isolation](web-api.md#tenant-isolation-and-the-run-sandbox).
Per-run OS identities via dedicated accounts and per-tenant containers
remain deployment modes, not enforced features — remote (cluster) runs
execute on the cluster host under the cluster's own controls, and the
namespace mode does not apply to them.

---

## API Namespace

```
/api
├── /health                 # Health check (with license, mode, component status)
├── /system                 # System info
├── /metrics                # Runtime metrics (CPU, memory, active runs)
├── /openapi.json           # OpenAPI 3.1 spec
├── /events                 # SSE event stream (real-time execution updates)
├── /audit                  # Audit logs (structured, with result field)
├── /hpc                    # HPC scheduler status (SLURM, PBS, etc.) — hpc mode only
│
├── /auth
│   ├── /login              # Login (username/password)
│   ├── /me                 # Current session info
│   └── /oauth/...          # OAuth2 authorize / callback
│
├── /license
│   ├── /                   # License status (type, validity, contact)
│   └── /upload             # Upload commercial license file
│
├── /users                  # User list/create/delete (admin)
│
├── /pipelines (new v0.8 API — replaces /workflows)
│   ├── /parse              # Parse TOML → structured pipeline
│   ├── /validate           # Validate pipeline DAG
│   ├── /prepare            # Prepare (expand wildcards, resolve envs)
│   ├── /dag                # Build DAG as JSON
│   ├── /format             # Canonical TOML formatting
│   ├── /lint               # Lint pipeline (valid + errors/lint findings)
│   ├── /stats              # Aggregate pipeline statistics
│   ├── /diff               # Diff two pipelines (by TOML content)
│   ├── /export             # Export Docker/Singularity packaging
│   ├── /search             # Search pipelines by name, tags, content
│   ├── /                   # GET: list pipelines; POST: save pipeline
│   ├── /{id}               # GET/PUT/DELETE pipeline by ID
│   ├── /{id}/fork          # Fork into user workspace (v0.8 collab)
│   └── /{id}/share         # Share pipeline (v0.8 collab)
│
├── /pipelines/import       # Import from oxo+http(s):// share URL (v0.8 collab)
│
├── /runs
│   ├── /                   # POST: create run; GET: list runs
│   ├── /{id}               # Run detail with log tail
│   ├── /{id}/status        # Real-time status (nodes, timeline, resources)
│   ├── /{id}/dag-status    # DAG JSON + per-node live status
│   ├── /{id}/diagnostics   # Diagnostic engine results (31 error patterns)
│   ├── /{id}/logs          # Execution logs
│   ├── /{id}/results       # Output files with sizes
│   ├── /{id}/retry         # Smart retry (failed + downstream only)
│   ├── /{id}/cancel        # Cancel running workflow
│   ├── /{id}/pause         # Pause running workflow
│   └── /{id}/resume        # Resume paused workflow
│
├── /data
│   ├── /analyze            # Scan files → detect format, suggest pipeline
│   ├── /reference          # Reference genome discovery
│   ├── /perceive           # Data perception (AI Companion)
│   ├── /reference/status   # Reference genome status
│   └── /samplesheet/parse  # Parse a samplesheet
│
├── /templates
│   ├── /                   # GET: list templates; POST: create
│   └── /{id}               # GET/DELETE template
│
├── /plugins
│   └── /validate           # Validate plugin manifest + signature
│
├── /ai
│   ├── /translate          # Natural language → validated .oxoflow (JSON)
│   ├── /translate/stream   # Same, streamed over SSE
│   ├── /explain            # Explain run failure + suggest fix
│   ├── /interpret          # Interpret results with caveats
│   ├── /optimize           # Optimize pipeline parameters
│   └── /config, /test      # AI provider configuration
│
├── /chat                   # AI Companion: /send, /send/json, /sessions
│
└── /pipeline/{id}          # DAG edit API: /command, /undo, /redo
```
(See [Web API](./web-api.md) for the complete API reference — the OpenAPI 3.1 spec is CODE-GENERATED via utoipa and served live at `GET /api/openapi.json` on any running server.)

---

## Structured Error Response

All errors follow this format — `detail` and `suggestion` are optional and
only present when a handler supplies them (e.g. `429` bodies carry
`detail: "retry in <N>s"`):

```json
{
  "code": "AUTH_REQUIRED",
  "message": "Authentication required in team/hpc mode",
  "suggestion": "Login at POST /api/auth/login to obtain a session token"
}
```

### Error Codes

| Code | HTTP Status | Description |
|------|-------------|-------------|
| `MISSING_FIELD` | 400 | Required request field missing (e.g. `toml_content`) |
| `DAG_EDIT_ERROR` | 400 | DAG edit command rejected (unknown operation, missing fields) |
| `AUTH_REQUIRED` | 401 | Authentication required |
| `AUTH_FAILED` | 401 | Invalid login credentials |
| `TOKEN_EXPIRED` | 401 | Session token missing or expired |
| `ACCESS_DENIED` | 403 | Permission denied |
| `NOT_FOUND` | 404 | Resource not found |
| `NO_UNDO` / `NO_REDO` | 404 | DAG edit history exists but the requested direction is exhausted |
| `EDIT_HISTORY_UNAVAILABLE` | 409 | No DAG edit history for this identity + pipeline (never edited, or lost to a server restart / LRU eviction); undo/redo fails closed |
| `EDIT_HISTORY_STALE` | 409 | Supplied `toml_content` is not the tip of the recorded DAG edit history; nothing is applied |
| `OAUTH_INVALID_STATE` | 400 | OAuth CSRF state missing, unknown, malformed, replayed, or past its 10-minute TTL |
| `OAUTH_STATE_MISMATCH` | 400 | OAuth CSRF state was issued for a different provider, redirect URI, or browser (missing/mismatched binding cookie — issue #1197). A browser-binding mismatch does not disclose which component failed; provider/redirect mismatches name the expected value by design |
| `OAUTH_REDIRECT_NOT_ALLOWED` | 400 | OAuth redirect target origin is not on the server-side allowlist |
| `RATE_LIMITED` | 429 | Request rate exceeded |
| `AUDIT_WRITE_FAILED` | 500 | A REQUIRED audit row for a security-relevant action could not be persisted; the request fails closed. Audit rows are written ahead of the mutation (a durable `pending` intent before the handler, finalized after) so a crash cannot leave a committed change with no audit row (WEB-15 / issue #1140, write-ahead ordering issue #1311) |
| `DB_ERROR` | 500 | Internal database error |

Domain-specific codes (all 400) include `PARSE_ERROR`, `VALIDATE_ERROR`, `PREPARE_ERROR`, `DAG_ERROR`, `LINT_ERROR`, `STATS_ERROR`, `DIFF_ERROR`, `EXPORT_ERROR`, `RUN_ERROR`, `RETRY_ERROR`, `DATA_ERROR`, `REF_ERROR`, `PLUGIN_ERROR`, `AI_TRANSLATE_ERROR`, `CHAT_ERROR`, `INVALID_URL`.

---

## List Responses

List endpoints do not share a single uniform envelope:

- `GET /api/runs` uses cursor pagination — `{items, next_cursor, total}`,
  where `next_cursor: null` means the last page.
- Most other list endpoints return bare JSON arrays capped at 100 items
  (`GET /api/pipelines` returns at most 100, ordered by last update). The
  exceptions are `GET /api/templates`, `GET /api/users`, and
  `GET /api/clusters`, which return every row and are intentionally uncapped
  administrative lists.

```json
[{ "id": "...", "name": "...", ... }]
```

---

## AI Agent Integration Guide

### Discovery

```
# Start here to discover the full API surface
GET /api/openapi.json
```

### End-to-End Workflow

```
1. GET /api/health              # Check server availability
2. POST /api/auth/login         # Authenticate (team/hpc mode)
3. POST /api/ai/translate       # [AI] "Run standard RNA-seq differential expression"
4. POST /api/runs               # Execute the pipeline (POST the TOML content)
5. GET  /api/events             # SSE real-time progress (optional)
6. GET  /api/runs/{id}/results  # Get structured results
```

### API Streaming

AI pipeline generation supports SSE streaming via the dedicated streaming endpoint:

```
POST /api/ai/translate/stream
Accept: text/event-stream

event: progress
data: {"step": "intent", "message": "Intent received", "intent": "..."}

event: progress
data: {"step": "match", "message": "Matching templates...", "templates_count": 5}

event: progress
data: {"step": "generate", "message": "Generating pipeline via AI..."}

event: done
data: {"pipeline_id": "...", ...}
```

---

## SPA Interaction Patterns

The frontend (`frontend/`, React 19 + TypeScript) follows a few shared UX conventions so behavior stays consistent across pages:

- **Collapsible sections**: Settings sections, the Rule Inspector field groups (inputs/outputs, resources, advanced), and the run monitor's alert cards + Recent Events table fold behind clickable headers (`role="button"`, `aria-expanded`, Enter/Space toggles, rotating chevron). Settings and Rule Inspector sections default to open, and the monitor's alert cards default expanded (one card per alert); the Recent Events table defaults collapsed for every run so a burst of alerts doesn't bury the live view. Collapsing only hides children; form state lives in the parent component, so no draft input is lost.
- **Styled confirmation dialogs**: destructive actions (pipeline delete) open the shared `Modal` component instead of `window.confirm` — the native dialog cannot be styled and blocks the event loop in the embedded desktop shell. The modal provides Escape/backdrop close, a focus trap, and focus restore.
- **Honest AI-disabled state**: when no AI provider is configured, the Dashboard entry card renders a "configure AI" call-to-action linking to `/settings` (with a note about server-side configuration), and the chat composer's editor hand-off CTA only renders outside the editor context.
- **Unified app navigation**: the SPA has exactly one primary nav — a sectioned sidebar (`WORKSPACE` / `INTELLIGENCE` / `INFRASTRUCTURE` / `ADMINISTRATION`) defined once in `Layout.tsx` with per-item role gating; the header is deliberately nav-free. At ≥1280px the sidebar is collapsible (240px ↔ 72px icon rail, persisted per device in localStorage, Ctrl/Cmd+B toggles); 1024–1279px forces the icon rail; below 1024px the sidebar becomes a drawer opened from the header toggle — a `role="dialog"` with focus trap, focus moved in on open, Escape/scrim/route-change dismissal. Icon-rail tooltips show on hover and keyboard focus. Version and license render in the page footer on every in-shell route; the Dashboard additionally renders a Version stat card from `health.version` (so the running engine version is visible at a glance). `/login` is a standalone centered card outside the app shell (same pattern as `/share`), so no app footer is shown on it.
- **Polling-stable UI state**: expansion/collapse choices on the run monitor are keyed by stable alert identifiers (`alert_id`, falling back to `rule-prediction`), so they survive the 5-second polling re-render.
- **Truthful environment labels on the monitor canvas**: the run monitor's DAG canvas draws each rule with the environment the run actually wraps it in — `GET /api/runs/{id}/dag-status` resolves it from the run's own pipeline snapshot the way the run path does (the same `apply_defaults` fold `run`/`dry-run` make: `env_group` → the rule's inline environment → `[defaults] environment`, issue #1210), and the canvas renders that kind (`conda`, `mamba`, `pixi`, `docker`, `singularity`, `venv`, `modules`, `system`) instead of painting every node `system` (issue #1149, audit UX-07). A node that no longer maps to a snapshot rule (an orphaned instance left by an edited workflow) is labelled `unknown` — the canvas never guesses `system`. The label is per rule, so the same rule's instances share it; the Metro view's process grouping therefore splits stations by real environment.
- **End-to-end workflow lifecycle & Command Palette**: the Command Center (`Dashboard.tsx`) surfaces a 5-stage lifecycle architecture grid (`1. Data & Samples` → `2. Author & AI` → `3. Validate & Lint` → `4. Execute & Guard` → `5. Provenance & Report`) that links directly to the corresponding editor modals (`/editor?modal=data`, `/editor?modal=engine`) and monitor tabs. A global keyboard Command Palette (`⌘K` / `Ctrl+K`) in `Layout.tsx` provides quick search across pages, lifecycle modals, and theme/locale toggles without adding header navigation links. The editor integrates sample-sheet cohort parsing (`POST /api/data/samplesheet/parse`), workspace file uploads (`GET/POST /api/files`), vendor MD5 delivery verification (`POST /api/data/verify`), `[engine]` disk-pressure & protected-tree configuration, canonical formatting (`POST /api/pipelines/format`), and best-practice linting (`POST /api/pipelines/lint`); the run launcher and monitor expose `--provenance` (`GET /api/runs/{id}/provenance-verify`), `--allow-protected-drift` (`GET /api/runs/{id}/protected-drift`), `--arg KEY=VALUE` overrides, and output file preview/download (`GET /api/runs/{id}/files`).
