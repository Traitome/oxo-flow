# Web API

oxo-flow includes a built-in REST API server for building, validating, running, and monitoring bioinformatics workflows. The server is built with [axum](https://github.com/tokio-rs/axum) and follows a **domain-driven modular monolith** architecture.

---

## API Design Conventions

- **Envelope**: success responses are bare JSON objects/arrays; errors are `{ code, message, detail, suggestion }` — `detail` and `suggestion` are always present and are `null` when the error carries no extra context
- **Errors**: the `code` value is a stable semantic string per failure class (`AUTH_REQUIRED`, `NOT_FOUND`, `RUN_RATE_LIMITED`, `AI_TIMEOUT`, …). Engine [diagnostic codes](./glossary.md#diagnostic-codes) (`E0xx`) surface inside `message` (e.g. a parse failure reported as `RUN_ERROR` carries `E017: unknown key …` in its message), not as the envelope `code`
- **Lists**: `GET /api/runs` returns a cursor-paginated envelope `{ items, next_cursor, total }` (limit ≤ 500, `status`/`q` filters); other list endpoints return bare arrays (≤ 100 items)
- **Versioning**: `/api/` prefix for all endpoints
- **Authentication**: in team/hpc mode, protected endpoints accept an `Authorization: Bearer <token>` session token or an `X-API-Key` header. The generated OpenAPI spec at `GET /api/openapi.json` declares both `bearerAuth` and `apiKey` security schemes; public endpoints (health, login, license, openapi.json, etc.) require no auth.
- **Self-discoverable**: OpenAPI 3.1 spec at `GET /api/openapi.json`. The spec is **code-generated** via [utoipa](https://github.com/juhaku/utoipa) from the `#[utoipa::path]` annotations on every route handler — there is no hand-maintained static file. `crates/oxo-flow-web/tests/openapi_gate.rs` is the drift gate: it asserts every route in the router appears in the generated spec, so a new route without an annotation fails CI.

### Structured Error Format

All errors return a unified JSON format:

```json
{
  "code": "AUTH_REQUIRED",
  "message": "Authentication is required for this endpoint",
  "detail": "The request did not include a valid session token or Bearer token",
  "suggestion": "Please login at POST /api/auth/login to obtain a session token"
}
```

`detail` and `suggestion` are **always serialized**; when an error has no
extra context they are `null` rather than absent, so clients can parse the
four-field shape unconditionally (the OpenAPI `ApiError` schema marks both
nullable).

Rate limiting follows the same contract: over-limit requests get
`429 {"code":"RATE_LIMITED", "message":"Rate limit exceeded", "detail":"retry in Ns", ...}`
with a `Retry-After` header (sliding window, 100 requests / 60 s per
client IP by default).

`POST /api/auth/login` adds a dedicated brute-force guard: **10 failed
attempts per 5 minutes per (account, source IP)** trip a lockout that
answers the same structured `429 RATE_LIMITED` (the countdown is in
`detail`; the login handler's error tuple cannot attach a `Retry-After`
header) before any credential check runs. Per-account keying means a
sprayed username cannot lock out other accounts; a successful login
resets the counter; `OXO_FLOW_DISABLE_RATE_LIMIT=1` disables every
limiter — the API limiter, the login brute-force guard, and the dedicated
run-creation limiter (#213) (tests/dev only).

Malformed input is answered in the same envelope, never axum's bare-text
rejection: a query string that does not fit the endpoint's parameters gets
`400 INVALID_QUERY`, and a JSON body that does not deserialize against the
endpoint's schema gets `400 INVALID_BODY` — both carry axum's parse message
in `detail` and a pointer to `GET /api/openapi.json` in `suggestion`. A body
that deserializes but fails a handler-level required-field check answers an
endpoint-specific code instead (e.g. `POST /api/runs` without
`toml_content` gets `400 MISSING`). Clients should rely on the four-field
envelope and treat `code` as informative; the endpoint-specific sections
below name the codes each route can return.

---

## Starting the Server

```bash
# Mode 1: Personal (default) — SQLite, no auth, localhost
oxo-flow serve

# Mode 2: Team — auth enabled, network-facing
oxo-flow serve --mode team

# Mode 3: HPC — cluster-aware
oxo-flow serve --mode hpc

# Or via the standalone binary:
oxo-flow-web --mode personal -p 3000
```

---

## System & Monitoring

### Health Check
```
GET /api/health
```
Returns status, version, mode, uptime, component health (database, filesystem, scheduler, AI provider), resource usage, and license info.

In `hpc` mode, `components.scheduler` reports `{"status":"ok"}` when a scheduler backend is detected (the same cached probe `/api/hpc` uses); it is `null` in other modes or when no backend is installed.

`components.ai_key_storage` is `"encrypted"` when `OXO_FLOW_MASTER_KEY` is set, `"plaintext"` otherwise.

`components.ai_provider` is `{"status": "connected", "latency_ms": null}` while a provider is active and `{"status": "disabled", "latency_ms": null}` when AI is off — explicitly pinned with `OXO_FLOW_AI_PROVIDER=disabled`, disabled at runtime, or nothing resolved. The pin outranks every persisted tier: `~/.oxo-flow/ai_config.json`, the platform config file, and DB-persisted settings can supply credentials for any other provider value but cannot resurrect a disabled registry. In that state the AI endpoints answer the structured `AI_NOT_CONFIGURED` error without any outbound request. Configuration updates follow the same rule with one distinction: the runtime-only `POST /api/ai/config` answers `409 AI_DISABLED_BY_ENV` (it has no durable tier, so refusing is the only honest answer), while the persisted `PUT /api/ai/config/server` and `PUT /api/ai/config/user` still save (200, `applied_to_runtime: false`) — Settings stays usable and the saved config applies after an unpinned restart — but never reconfigure the pinned runtime. `POST /api/ai/test` answers `success: false` with `provider: "disabled"` without constructing a provider or opening a connection (see the AI endpoints section below).

`components.engine` reports the engine CLI the server spawns for runs, resolved once at startup by running `<binary> --version`:

```json
{
  "status": "ok",
  "components": {
    "engine": {
      "version": "0.32.0",
      "path": "/usr/local/bin/oxo-flow",
      "version_mismatch": false
    }
  }
}
```

`version_mismatch` is `true` when the engine's major.minor differs from the server's — runs will execute under the engine's semantics, and the server log carries a drift warning with a hint to pin `OXO_FLOW_BIN`. The field is `null` when the probe failed (binary missing or unparseable `--version` output); the run log header remains the version record in that case.

### System Info
```
GET /api/system
```
Returns OS, architecture, PID, uptime, and version. **Team/hpc modes require authentication** (the endpoint left the anonymous whitelist in the v0.11 hardening).

The response also carries `engine` — the same engine-CLI info as `/api/health`'s `components.engine`.

### Runtime Metrics
```
GET /api/metrics
```
Returns real-time resource metrics: CPU%, memory (used/total/swap), active workflows, total requests, CPU count. **Team/hpc modes require authentication.**

### Audit Logs
```
GET /api/audit?days=7&page=1&per_page=50
```
Returns paginated structured audit entries:
```json
{
  "entries": [{ "timestamp", "user", "action", "resource", "result" }],
  "days": 7,
  "page": 1,
  "per_page": 50,
  "total": 128
}
```
`page` defaults to 1 and `per_page` defaults to 50 (max 200). **Team/hpc modes: admin-only** (the trail spans every user's actions; personal mode keeps the localhost trust model).

`user` is the actor's canonical `users.id`, stored verbatim with each row — the same identity sessions and ownership columns carry. That id is a UUID for DB-created/bootstrap accounts and the username itself for env-password-provisioned identities (`user`, `viewer`) and the `default` personal-mode pseudo-user, so a result set legitimately mixes both shapes. Historical rows are never rewritten; resolve a UUID to a human label through `GET /api/users` when one is needed (admin-only).

#### Audit durability (WEB-15 / issue #1140)

Every audit write is explicitly classified, and the class is part of the
route contract:

- **Required** — security-relevant actions: authentication attempts
  (`POST /api/auth/login`, logout, OAuth), user/role management
  (`POST /api/users`, `DELETE /api/users/{id}`), machine-key changes
  (`POST /api/auth/keys`, `DELETE /api/auth/keys/{id}`), run lifecycle
  control (`POST /api/runs` and `/{cancel,pause,resume,retry,resume-checkpoint,clean}`),
  pipeline deletion and share grants
  (`DELETE /api/pipelines/{id}`, `POST /api/pipelines/{id}/share`), AI
  provider key changes (`POST /api/ai/config`, `PUT /api/ai/config/server`,
  `PUT /api/ai/config/user`), cluster connection secrets
  (`POST /api/clusters`, `DELETE /api/clusters/{id}`), webhook configuration
  (`PUT /api/webhook`), and license upload (`POST /api/license/upload`).
  If the row cannot be written, the request **fails closed** with
  `500 {code: "AUDIT_WRITE_FAILED"}` carrying the storage error in
  `detail` and a remediation in `suggestion`. Exception: a response that is
  already a server error (5xx) is preserved, so the documented runs-domain
  `503 RUNS_REQUIRE_STORAGE_BACKEND` boundary still reaches the client on a
  deployment with no initialized storage backend.
- **Best-effort** — informational mutations (AI generation, parsing,
  validation, DAG edits, chat, pipeline create/update/fork/rollback,
  template writes, quota and event-ticket requests). A failed write is
  logged as a warning and the request answers normally.

**The audit row is not written in the same transaction as the mutation.**
The handler runs first; the audit insert is attempted afterwards. A
REQUIRED-audit failure therefore does **not** roll the action back — the
pipeline may already be deleted, the run retry may already have spawned,
the key revocation may already be applied, the account may already exist.
The `500 AUDIT_WRITE_FAILED` body says the action "may already have been
applied" and its `suggestion` tells the caller to verify the affected
resource (list/inspect it) before retrying, because a blind retry can
duplicate the effect (e.g. a second forked/deleted resource or a second
run). Writing the audit row in the mutation's transaction is tracked as
remaining work in the WEB-15 ledger entry. Per-route implication: every
REQUIRED route listed above can answer this 500 **after** applying its
change; treat the 500 as "state unknown", not as "nothing happened".

Both classes share one writer (the storage backend's `record_audit_log`)
behind the named `audit_required` / `audit_best_effort` helpers; no handler
writes the table directly. The middleware emits a structured `audit` event
for every mutation it handles, including when the database insert fails, so
machine consumers still see the attempt.

Successful sign-ins are the one handler-owned REQUIRED row (WEB-15 /
issue #1199): the middleware sees only the pre-auth request and would
misattribute the row to the `default` pseudo-user, so `POST /api/auth/login`
records its own canonical `users.id` row. That write is REQUIRED, so a
successful login on broken storage answers the same
`500 AUDIT_WRITE_FAILED` instead of the computed 200 — and the ordering
honesty applies here too: the session is persisted **before** the audit
write, so the `500` means a session token may already have been minted for
the account even though the sign-in response failed. Verify the session
state before retrying. Failed login attempts stay REQUIRED rows written by
the middleware. The license-upload metadata row stays best-effort: the
middleware already writes the REQUIRED row for `POST /api/license/upload`.

### Server-Sent Events
```
GET /api/events
Accept: text/event-stream
```
SSE stream for real-time workflow execution events: terminal events
(`run_completed`, `run_failed`, `run_cancelled`) plus per-rule events
(`rule_started`, `rule_completed`, `rule_failed`, `rule_skipped` — parsed
live from the engine's execution log). Keepalive is axum's 15-second
comment ping; if the client falls behind the broadcast buffer, the stream
carries a synthetic `{"type":"lagged","data":{"missed":N}}` event —
refetch run state when you see it.

**Team/hpc modes require a one-time `?ticket=`** minted at
`POST /api/events/ticket` under normal bearer auth (EventSource cannot set
an Authorization header; the ticket is single-use, expires in 30 s, and
replaces the session token that used to travel — and leak — in the URL).
`POST /api/events/ticket` responds
`{"ticket": "...", "expires_in_secs": 30}`. The stream is filtered to the
subscriber's own runs — admins see everything. Events carry a `user` field
(the owning user id, or `null` for system-wide events). The `run_started`
event's `data.username` field predates the canonical user model and also
carries the canonical user id (kept for wire compatibility — read the
envelope `user` field instead).

`run_completed` carries a `summary` field: the CLI's invalidation summary
extracted from the execution log — config changes, edited rule definitions,
and input-set changes that invalidated checkpoint records this run
(`null` when the run had no invalidation activity).

The stream ends when the server shuts down (SIGTERM/Ctrl+C): the response
is closed as part of the graceful drain, so an `EventSource` sees its
`error`/close at shutdown rather than a hung connection. Clients
should reconnect with backoff after an unexpected close.

---

## Authentication & Authorization

### Login
```
POST /api/auth/login
Content-Type: application/json

{"username": "admin", "password": "admin"}
```
Returns session token, username, and role.

> **Credentials are environment-driven, not hardcoded.** The example above
> only works when `OXO_FLOW_DEV_MODE=1` (accepts `password == username`) or
> when matching env passwords are set: `OXO_FLOW_ADMIN_PASSWORD`,
> `OXO_FLOW_USER_PASSWORD`, and `OXO_FLOW_VIEWER_PASSWORD` define the
> admin/user/viewer accounts. Without one of these the login returns 401 —
> set the env vars when launching `oxo-flow serve` (see
> [Web System Architecture](web-system-architecture.md)).
>
> **Identity is pinned server-side:** the shared user/viewer
> passwords never mint or adopt the `admin` identity (or any existing
> API-created account) — `admin` signs in only with
> `OXO_FLOW_ADMIN_PASSWORD`, and a username that belongs to a managed
> account requires that account's own password. OAuth identities are
> namespaced (`<provider>:<name>`) and can never collide with local
> usernames. Sessions store the canonical `users.id` resolved at login;
> role lookups match by id only and a session whose user row is gone
> (deleted user, stale pre-upgrade session) is rejected with 401.
>
> **Session lifetime is operator-tunable:** every minted session
> (login and OAuth callback) expires after `OXO_FLOW_SESSION_TTL_HOURS`
> hours — default `24`, minimum `1`, cap `8760` (one year). Unset keeps
> the historical 24-hour TTL; an unparseable or out-of-range value warns
> and falls back to the default. Existing sessions keep their minted
> expiry — the variable is read at session creation only.

### Check Session
```
GET /api/auth/me
Authorization: Bearer <token>
```
Returns `{"authenticated": true, "username": "admin", "role": "admin"}` or `{"authenticated": false}`.

### Sign Out
```
POST /api/auth/logout
Authorization: Bearer <token>
```
Revokes the presented session server-side — the token stops
authenticating immediately instead of living out its configured TTL. The
client should drop its local copy (`logged_out: true` on success).
Revocation is **fail-closed** (issue #1135): if the session row cannot be
deleted the endpoint answers `500 DB_ERROR` rather than `logged_out: true`,
so a client is never told a still-live token is dead. A token that was
already absent (e.g. a repeated logout) is the benign case and still
succeeds.
Deleting a user (`DELETE /api/users/{id}`) cascades: the deleted user's
sessions and API keys are removed with the account.

### License Status
```
GET /api/license
```
Returns license type, validity, commercial use flag, and contact info.

### Upload License
```
POST /api/license/upload
```
Admin-only outside personal mode; the payload is bounded (64 KiB). The
raw blob is **not** stored — only its size and SHA-256 land in the audit
trail (the endpoint does not yet activate licenses — no license format or
verification machinery exists yet; `GET /api/license` reports the built-in
academic license regardless of any upload).

### Users (admin)
```
GET    /api/users          # List users (admin only)
POST   /api/users          # Create user
DELETE /api/users/{id}     # Delete user
```

---

## Pipeline Lifecycle (v0.8 `/api/pipelines/*`)

> The pre-v0.8 `/api/workflows/*` endpoints no longer exist in the source — the running server exposes only the `/api/pipelines/*` API below.

### Parse
```
POST /api/pipelines/parse
Content-Type: application/json

{"toml_content": "<workflow TOML>", "format_version": "1.0"}
```
Returns structured pipeline: `pipeline_id`, `name`, `version`, `rules` (with summaries), `dag` (nodes + edges), `stats`. Pure function, zero side effects.

### Validate
```
POST /api/pipelines/validate
Content-Type: application/json

{"pipeline_id": "...", "toml_content": "<TOML>", "base_dir": "inputs"}
```
Returns `{ valid, errors: [{ code, message, rule, suggestion }] }`.
`base_dir` (optional, for missing-input checks) must be relative (no
`..`) and resolves inside the acting user's workspace.

### Prepare
```
POST /api/pipelines/prepare
Content-Type: application/json

{"toml_content": "<TOML>", "resolve_wildcards": true, "apply_defaults": true}
```
Expands wildcards, resolves environments. Returns `expanded_rules_count`, `wildcard_combinations`, `environment_setup_cmds`.

### Build DAG
```
POST /api/pipelines/dag
Content-Type: application/json

{"pipeline_id": "...", "toml_content": "<TOML>"}
```
Returns `{ nodes, edges, parallel_groups, critical_path, metrics }` as structured JSON.

### Format
```
POST /api/pipelines/format
Content-Type: application/json

{"toml_content": "<TOML>"}
```
Returns canonical TOML formatting.

### Lint
```
POST /api/pipelines/lint
Content-Type: application/json

{"toml_content": "<TOML>"}
```
Returns `{ valid, errors: [{ code, message, rule, suggestion }] }` — validation errors plus lint-level findings (e.g. missing description).

### Stats
```
POST /api/pipelines/stats
Content-Type: application/json

{"toml_content": "<TOML>"}
```
Returns aggregate pipeline statistics.

### Diff
```
POST /api/pipelines/diff
Content-Type: application/json

{"toml_a": "<TOML A>", "toml_b": "<TOML B>"}
```
Returns structured diffs: `{ diffs: [{ path, category, description, severity }] }`.

### Export
```
POST /api/pipelines/export
Content-Type: application/json

{"toml_content": "<TOML>", "format": "docker|singularity"}
```
Generates Dockerfile or Singularity definition.

### List / Save / Get / Update / Delete
```
GET    /api/pipelines              # List pipelines (most recent first, up to 100)
POST   /api/pipelines              # Save new pipeline
GET    /api/pipelines/{id}         # Get pipeline with TOML content
PUT    /api/pipelines/{id}         # Update pipeline
DELETE /api/pipelines/{id}         # Delete pipeline
POST   /api/pipelines/search       # Search by name, tags, content
```

#### Visibility policy (WEB-13 / issue #1138)

Every list, read, and write route evaluates ONE exported predicate —
`domains::workflow::visibility::can_read_pipeline` (re-exported at the
workflow domain root) — for reads, plus `can_write_pipeline` for mutations.
The policy is explicit and tested:

- **Owner:** a pipeline is always visible to its `user_id` owner.
- **Shared ("template"):** `visibility = "workspace"` makes a pipeline
  visible to every signed-in user. Personal mode has a single `default`
  pseudo-user, so there every pipeline is simply "own".
- **Other tenants:** another tenant's `private` or `link` pipeline is
  invisible. `link` is NOT a read grant — it exists so the owner can mint a
  share token, and the token (not this predicate) authorizes
  `GET /api/share/{token}` and `POST /api/pipelines/import`.
- **Admin override:** admins see every tenant's pipelines (and, as before,
  may modify them). This is the documented audit/administration exception.
- **Deleted:** deletion is a hard delete (the row is removed in both
  storage backends); a deleted pipeline is invisible to everyone — admins
  included — and every read answers `404`.
- **Unknown visibility values** fail closed as private.

List and search routes FILTER with the predicate; read/write routes answer
`404 NOT_FOUND` — never `403` — when it returns false, so the existence of
another tenant's pipeline cannot be probed. Covered paths: `GET
/api/pipelines`, `POST /api/pipelines/search`, `GET`/`PUT`/`DELETE
/api/pipelines/{id}`, both revision routes, rollback, fork, share, `POST
/api/runs` with a `pipeline_id`, `POST /api/ai/optimize` with a
`pipeline_id`, and `POST /api/pipelines/diff`/`export` when they resolve
saved ids.

Deliberate exemptions: the share landing and import routes authorize via
the share token; the `/api/templates` library is a global shared resource
whose rows carry no per-tenant visibility column (writes stay
owner/admin-gated); and the DAG edit endpoints operate on caller-supplied
TOML only, never on a stored pipeline.

The pipeline's `version` is always derived from the stored TOML's
`[workflow] version` key — the TOML is the source of truth.
An explicit `version` field on POST/PUT is ignored (never stored), so a
save whose content carries `version = "1.2.3"` reports `1.2.3` on the
next GET, and updating content without touching `[workflow] version`
bumps the reported version to match. This mirrors the CLI, where the
file on disk defines the version.

#### Optimistic concurrency (`ETag` / `If-Match`)

Every pipeline row carries a monotonic stored `revision`: it starts at `1`
on create and increments by exactly one on every successful write (PUT or
rollback). It is deliberately separate from the TOML `version` — a content
edit that leaves `[workflow] version` untouched still advances the token,
because the token detects writes, not semantic releases. It is not a JSON
body field; it travels as a strong HTTP validator so the response contract
stays backward compatible:

- `GET /api/pipelines/{id}`, `POST /api/pipelines` (create),
  `PUT /api/pipelines/{id}`, and `POST /api/pipelines/{id}/rollback`
  answer with `ETag: "<revision>"` (e.g. `ETag: "7"`).
- To make a write conditional, echo that value in `If-Match`:

```
PUT /api/pipelines/{id}
If-Match: "7"
Content-Type: application/json

{"toml_content": "<TOML>"}
```

The compare-and-swap is atomic in the storage backend — a single
`UPDATE ... WHERE id = ? AND revision = ?` guarded by the rows-affected
count, implemented in both the SQLite and PostgreSQL `StorageBackend` — not
a read-then-write race. A stale validator changes nothing and answers:

```
HTTP/1.1 409 Conflict

{"code": "PIPELINE_VERSION_CONFLICT",
 "message": "Pipeline <id> was modified by another writer",
 "detail": "expected revision 7 but the stored revision is 9",
 "suggestion": "Re-read GET /api/pipelines/<id> for its current ETag and retry with If-Match: \"9\"."}
```

A successful PUT answers `200` with the body and the NEW `ETag`. Omitting
`If-Match` keeps the historical unconditional update for backward
compatibility (the stored revision still advances, so a later conditional
writer can detect the intervening write); clients that care about lost
updates SHOULD always send it. `POST /api/pipelines/{id}/rollback` shares
the same contract. A malformed validator — non-integer, zero, negative, a
weak `W/"7"` form, a multi-token list, or `*` — is rejected with
`400 INVALID_IF_MATCH` rather than silently ignored.

---

## Execution & Runs

### Create Run

> **Database Backends**: run-record persistence and lifecycle routing use
> the unified `StorageBackend`, with SQLite and feature-gated PostgreSQL
> implementations. A CI smoke test against a live PostgreSQL container
> verifies the schema bootstrap and the core `StorageBackend` CRUD surface,
> and the storage contract was live-verified against `postgres:16`
> (2026-11-05: full contract round-trip passed, web suite 554 passed /
> 0 failed with the `postgres` feature); concurrency, crash recovery, and
> multi-instance behavior remain unvalidated. The runs gate is
> backend-agnostic: if no storage backend is initialized at all, these
> endpoints return
> `503 {"code": "RUNS_REQUIRE_STORAGE_BACKEND", "message", "detail", "suggestion"}`.
> Team/HPC execution also lacks an OS-level tenant boundary and must not be
> exposed to untrusted workflow authors. Issue #1129 (RB-01a) hardens the web
> spawn path — the child environment is allowlisted, so server secrets
> (`DATABASE_URL`, `OXO_FLOW_MASTER_KEY`, provider/MCP credentials) no longer
> reach a tenant-authored rule shell, and operator-wide `RLIMIT_CPU`/`RLIMIT_NPROC`
> ceilings join the existing `RLIMIT_DATA` on Linux. There is still NO
> filesystem namespace, uid boundary, or per-run container: a malicious workflow
> that guesses another tenant's absolute path can still read (and write) it.
> Treat the boundary as partial, not complete.

```
POST /api/runs
Content-Type: application/json

{"toml_content": "<workflow TOML>", "max_jobs": 4, "dry_run": false, "keep_going": false, "pipeline_id": "<uuid>", "cluster_id": "<cluster-id>", "samples": ["S1", "S2"], "targets": ["rule_name"], "include_files": {"envs/clinical/pixi.toml": "[env] ..."}, "config_overrides": ["min_quality=30"], "allow_protected_drift": false}
```
`toml_content` is the workflow source (required — the run is created from
it, not from a saved pipeline); `max_jobs`, `dry_run`, `keep_going`,
`pipeline_id`, `cluster_id`, `samples`, `targets`, `include_files`,
`config_overrides`, and `allow_protected_drift` are top-level fields
(not nested under a `config` object). Run-mutating endpoints
(create/cancel/pause/resume/retry/clean/resume-checkpoint) require the
`user` role or above — the viewer role is read-only — and a `pipeline_id`
must be readable by the acting user. This flat shape is the typed
`CreateRunRequest` schema — the OpenAPI spec declares it as the endpoint's
`requestBody`, and the handler parses exactly that type, so the spec, the
server, and the TypeScript client stay in lockstep. Returns
`{ run_id, status: "queued", estimated_resources, execution_plan }`.

`execution_plan` and `estimated_resources` are derived from the **resolved**
workflow, not from `toml_content` alone: when `include_files` are supplied, the
sources are written beside `workflow.oxoflow` and loaded through the same
`from_file` path the executor uses before the plan is built, so rules
contributed by an `[[include]]` — and a rule-level `extends` whose base lives in
one — appear in the reported plan and resource estimate. Without staged
sources the single-file parse is used, exactly as before. A BioType violation
contributed by a staged include keeps the stable
`400 TYPE_CONTRACT_VIOLATION` code.

The effective workflow (includes merged, `extends` resolved) is also persisted
in the run's working directory as `.oxo-flow/resolved-workflow.oxoflow`.
Retry, checkpoint resume (quota budgeting), `/dag-status`, `/status`,
`/instances`, and `/protected-drift` load that snapshot — falling back to the
host document for single-document runs — so a staged-include run reports,
budgets, and re-executes the same plan the CLI executed. The host TOML stays in
`pipeline_snapshot` for display.

Run creation pays the dedicated #213 spawn budget: `OXO_FLOW_RUNS_RATE_LIMIT`
run-creation attempts per minute **per acting identity** (default 5; `0`
disables the limiter). The budget is keyed by the authenticated `users.id`
(the `default` pseudo-user in personal mode), independent of the per-IP
request limiter above. Over budget, the attempt is rejected before any
workflow parsing or process spawn with
`429 {"code": "RUN_RATE_LIMITED", "message": "… retry in Ns", "detail": "Allowance: N runs per minute …"}`;
like every `ApiResult` error it carries no `Retry-After` header — the
countdown lives in `message`. The sliding window refills once the oldest
counted attempt is older than 60 seconds. Retries and checkpoint resumes pay
the same budget (see [Smart Retry](#smart-retry)).

Before spawning the executor the run stages the acting user's uploaded
files (`POST /api/files` → `workspace/users/<user>/inputs/`) into the run's
working directory, so `metadata_file` and relative input paths resolve the
same way they do on the CLI. Files already present in the workdir are never
overwritten.

Run creation fails **closed** before the executor is spawned (issue #1131 /
audit RB-03). A create whose pre-spawn staging cannot complete answers
non-2xx instead of starting a run with missing sources, and compensates so
no ghost `queued` row survives:

- A failure to materialize the workflow source, any `include_files`
  entry beside `workflow.oxoflow`, or the effective resolved-workflow
  snapshot answers
  `500 {"code": "RUN_ERROR", "message": "Failed to materialize …"}`, and a
  failure to stage uploaded inputs answers
  `500 {"code": "RUN_ERROR", "message": "Failed to stage uploaded inputs for run <id>"}`.
- On any of those failures the handler releases the admitted spawn
  reservation, deletes the just-inserted run row, and removes the run's
  workdir — so a failed admission leaves no trace. The same compensation
  covers the cluster-resolution paths: a `cluster_id` that fails to load
  (`500 DB_ERROR`) or resolves to no enabled cluster (`404 CLUSTER_NOT_FOUND`)
  also removes the row and workdir rather than leaving a ghost `queued` run
  nothing will ever execute. (Orphan recovery scans `running`/`paused`
  rows only; a `queued` row that never spawned would otherwise be
  unreachable, which is why the row is removed rather than left behind.)
- The spawn happens only after the workflow, its includes, the resolved
  snapshot, the staged inputs, and the run row are durably committed, so a
  client never sees a `2xx` for a run whose workdir is incomplete. A losing
  concurrent create still answers `409` with no trace (see the
  saved-pipeline slot rule above).

`pipeline_id` (optional) only *associates* the run with a saved pipeline —
the TOML still comes from `toml_content`. Every run created through this
endpoint, saved-pipeline or ad-hoc, gets its own immutable workdir
(`workspace/users/<user>/runs/<run_id>/`): the workflow snapshot,
`include_files`, staged inputs, checkpoint, logs, and outputs live there and
are never shared with another create (issue #1130 / audit RB-02). The run's `pipeline_snapshot` column stores the exact TOML it
was created from, so later edits to the saved pipeline cannot change what an
existing run executed or what `provenance verify` audits.

Saved-pipeline runs are admitted **one live run at a time** per pipeline: a
second `POST /api/runs` while a run of that pipeline is `queued`/`running`/
`paused` answers `409 RUN_NOT_TERMINAL`; a terminal (completed, failed,
cancelled) run releases the slot. The slot is global to the pipeline, not
per user: a `workspace`-shared pipeline admits one live run across all its
readers, so another identity's create gets the same 409 while the owner's run
is live. Pinned by `shared_pipeline_slot_is_global_across_identities`. This replaces the old checkpoint-reuse
across re-runs: a fresh create executes the whole pipeline. For
checkpoint-driven partial re-execution use
[retry](#smart-retry) (`POST /api/runs/{id}/retry`, re-executes in the
predecessor run's workdir) or `POST /api/runs/{id}/resume-checkpoint` —
those are explicit continuations of an
existing run lineage, not new run history. Ad-hoc runs are isolated by
their unique workdir and may run concurrently. Malformed or unknown
pipeline ids are rejected (`400 INVALID_PIPELINE_ID` /
`404 PIPELINE_NOT_FOUND`).

**Typed-workflow admission limit:** directly submitted typed workflows are
checked by the same core BioType validator as CLI preflight. Typed
declarations are detected structurally from parsed TOML, including both
table forms such as `[types.*]` / `[rules.types]` and inline forms such as
`types = { ... }`. The web admission layer resolves the full include graph
by staging `workflow.oxoflow` and all `include_files` sources on disk, then
re-parsing through the canonical `from_file` path (the same machinery the
CLI preflight uses) and running `validate_workflow_types` on the merged
config. This covers typed workflows with local include graphs and typed
includes (issue #1018). Scope note: the admission graph is built from the
workflow file's `[config]` table without `config_overrides` applied — the
CLI's own preflight re-runs the identical gate with overrides applied before
anything executes, so a workflow whose routing depends on an overridden
value fails at the spawned CLI, not silently (issue #1152 remaining gap).
Unresolvable includes
(`repo = ...` git includes, missing files) surface as
`400 {"code":"RUN_ERROR"}` from the canonical parser — not a blanket
typed-admission rejection. Only the host workflow and locally staged sources
reached through `[[include]]` participate in that check; unrelated staged
assets are ignored.

Execution flags are forwarded to the CLI executor:

- `dry_run: true` spawns the preview subcommand (`oxo-flow dry-run`) —
  nothing executes; the log shows the would-be plan.
- `max_jobs` defaults to **4** at this endpoint (the CLI flag itself
  defaults to 1): an omitted `max_jobs` still spawns the CLI with `-j 4`,
  which is also the value the resource estimate assumes.
- `keep_going` (boolean, optional, default `false`) maps to
  `--keep-going` (the CLI also accepts `-k`). The default is the
  historical abort-on-required-failure policy: when a required rule
  fails, the engine cancels the run's in-flight jobs. With `true`,
  already-running independent rules finish, ready independent rules keep
  dispatching, dependents of the failure stay blocked (never executed),
  and the run still exits non-zero. Run-only — ignored for
  `dry_run: true` (nothing executes). Cluster (`cluster_id`) runs carry
  the flag into the remote wrapper command the same way. Web **retry**
  does not re-apply it: the runs row does not persist the failure policy,
  and a retry re-executes under the default. Pass `keep_going` on a new
  create request instead.
- `samples` (array of strings, optional) maps to `--samples` (comma-joined).
- `targets` (array of strings, optional) maps to `-t` per entry.
- `include_files` (object, optional) stages **[[include]] source files**
  alongside the workflow: keys are workflow-relative paths (e.g.
  `envs/clinical/pixi.toml`), values are the file contents. Keys are
  validated as relative workspace paths — absolute paths, `..` components,
  and degenerate keys (empty, `.`-only, trailing slash) are rejected
  (`400 INVALID_INCLUDE_PATH`). Keys are canonicalized before validation
  and staging, so aliases such as `./workflow.oxoflow` and duplicate
  canonical keys, including case-insensitive aliases, are rejected; any
  canonical key equal to
  `workflow.oxoflow` (case-insensitively) is refused because it would
  overwrite the main workflow. Files are written beside `workflow.oxoflow`
  in the run directory, where the CLI's
  `resolve_includes` resolves them unchanged — the same machinery local
  `oxo-flow run` uses. The resolved workflow is also persisted as
  `.oxo-flow/resolved-workflow.oxoflow` in the run workdir, and `/status`,
  `/dag-status`, `/instances`, and `/protected-drift` load that snapshot
  (see the resolved-snapshot section above), so status/DAG views see the
  same merged rule set the run executes.
  There is no per-key content cap beyond the request-body limit (2 MiB by
  default, `body_limit` in the server config).
- `config_overrides` (array of strings, optional) passes CLI config
  overrides through to the spawned run as `--arg=KEY=VALUE` entries. Each
  entry must be a non-empty `KEY=VALUE` string (`400
  INVALID_CONFIG_OVERRIDE` otherwise); full override semantics
  (canonicalization, unknown keys, declared-key value checks) are enforced
  by the CLI — it **warns on unknown keys** (they change nothing) and
  **hard-fails on an invalid value for a declared key** (e.g.
  `config 'min_quality' expects an integer`). A typo in an override key is
  therefore only a warning, not a run failure.
  Cluster (`cluster_id`) runs carry the same overrides into the remote
  wrapper command. Overrides and include-file keys are validated **before**
  the run row is persisted, so a `400` never leaves a ghost `queued` run
  behind (a `400` is validated before anything is written).
- `allow_protected_drift` (boolean, optional, default `false`) forwards
  `--allow-protected-drift` to the spawned run: the CLI's
  run gate refuses to execute when files under a
  [`protected_data_trees`](workflow-format.md) tree changed since the
  recorded baseline; this flag is the web escape hatch to proceed anyway
  (the CLI still narrates the drift into the run log). Cluster
  (`cluster_id`) runs carry the flag into the remote wrapper command the
  same way. Web **retry** keeps the gate — only the original create
  request may relax it, mirroring the CLI's own resume. The flag only
  affects actual runs; with `dry_run: true` it is ignored (there is no
  gate to bypass in preview). Preview the drift first with
  `GET /api/runs/{id}/protected-drift` (see below) before overriding.
- `provenance` (boolean, optional, default `false`) forwards
  `--provenance` to the spawned run: the CLI records `sha256` checksums of
  every rule output into the checkpoint so the run can later be audited
  with `GET /api/runs/{id}/provenance-verify` (see below) or the CLI's
  `provenance verify`. Run-only — ignored for `dry_run: true` (the CLI
  rejects the flag there). Cluster (`cluster_id`) runs carry the flag
  into the remote wrapper command the same way. Web **retry** does not
  re-apply it (checksums are recorded by the original create request
  only).
- `cluster_id` (optional) names a configured SSH cluster connection: the
  workdir is staged to that host and executed there (see
  [Cluster Connections](#cluster-connections-remote-execution)).

### List Runs

```
GET /api/runs?limit=100&cursor=<created_at>&status=<status>&q=<search>
```

Cursor pagination, not page numbers: pass the `created_at` of the last row
of the previous page as `cursor` (`created_at < cursor`). `limit` defaults
to 100 and is capped at 500. The response is the envelope
`{ items, next_cursor, total }`, where `next_cursor: null` means the last
page. Each item carries `dry_run` — see [Run Detail](#run-detail).

### Run Status
```
GET /api/runs/{id}/status
```
Real-time status: `{ status, phase, nodes: [{ rule, status, started_at, duration_ms, exit_code }], timeline, resources }`. A rule that did not run — when-gated off (`when_verdicts == false`) or abort-cancelled (`rule_runs` status `cancelled`, which records no set membership) — is surfaced as `skipped`, never as a forever-`pending` node.

Timing guarantees for `nodes[]`: a terminal node that actually executed (`success`, `failed`, or run-level `cancelled`) reports `started_at` (RFC 3339 UTC) and `duration_ms` from the checkpoint's `rule_runs` start/end pair — completed rules pair the recorded start with their benchmark wall time; failed and cancelled rules derive the duration from the pair. Both fields are `null` when the underlying data genuinely does not exist: `pending`, `running`, `skipped`, synthetic "outputs up-to-date" completions (the rule did not execute in this run), and checkpoints written by engines predating the timestamp fields. The one legacy exception is a `success` node from an old checkpoint that recorded a benchmark but no start: it keeps the real `duration_ms` with `started_at: null` rather than fabricating a start.

### DAG Status
```
GET /api/runs/{id}/dag-status
```
DAG JSON with per-node live status. Color-coded: green=completed, blue=running, red=failed, gray=skipped or cancelled. Each node also carries the same `started_at`/`duration_ms` timing contract as [Run Status](#run-status) (`started_at` is an RFC 3339 UTC timestamp, not a duration substitute). Each node's `environment` is the backend kind the rule runs in (`conda`, `mamba`, `pixi`, `docker`, `singularity`, `venv`, `modules`, or `system`), resolved from the run's own pipeline snapshot the way the run resolves it — the same `WorkflowConfig::apply_defaults` call `run`/`dry-run` make, which folds the documented precedence onto each rule: `env_group` → the rule's inline environment → `[defaults] environment` (issue #1210). Because the monitor runs the same fold as execution, the label can never claim a wrapping the run does not perform. It is `null` only for a node that no longer maps to a snapshot rule (an orphaned instance of a workflow edited since the run), never a fabricated `system` (issue #1149, audit UX-07). A rule killed because the *run* was cancelled reports `status: "cancelled"` — never `failed` — while a rule with its own recorded exit code stays `failed` even if the run was later cancelled. `metrics` partitions the node set exactly: `completed_nodes` counts successful rules only, and `skipped_nodes` (when-gated off or abort-cancelled siblings of a failed run) plus `cancelled_nodes` (run-level cancellations) keep `completed + failed + cancelled + running + pending + skipped == total_nodes` true at every instant. The ETA average is computed over successful nodes' durations — failures, skips, and cancellations never inflate it.

### Diagnostics
```
GET /api/runs/{id}/diagnostics
```
Deterministic error analysis: `{ failed_nodes: [{ rule, error_pattern, likely_cause, suggestions, auto_fixable, fix_action, relevant_log_lines }], warnings, resource_bottlenecks }`. Uses 31 deterministic error patterns — zero AI in this endpoint. Diagnosis reads only the newest 256 KiB of `execution.log` (seek-based tail, torn first line dropped), never the whole file.

### Run Detail
```
GET /api/runs/{id}
```
The full run row: `{ id, user_id, pipeline_id, workflow_name, pipeline_snapshot, duration_secs, status, phase, dry_run, workdir, artifacts_incomplete, cluster_id, started_at, finished_at, created_at }` — the identity other run endpoints key off. `workflow_name` and `duration_secs` are the same values list items carry (`duration_secs` is the elapsed time between `started_at` and `finished_at`, `null` until the run finishes). (`pipeline_id`/`pipeline_snapshot` may be `null` for legacy rows; no `pid` — the host process id stays engine-internal.) `dry_run` mirrors the creation request (`true` for preview runs spawned by `dry_run: true`) and appears in both the list and detail responses — legacy rows created before the column existed read as `false`.

`artifacts_incomplete` (also on list items) is non-null only when a cluster run's result pull failed: the remote execution finished but its outputs could not be transferred back, so the local workdir — the source for the run's logs, preview and report — is missing or stale. It carries the pull error text (including the remote exit code). Such a run is **never** reported as `completed`: it is failed through the same non-success path a non-zero remote exit uses, so `status` reads `failed` with `phase` `failed` and the reason is both logged and persisted here for the UI (issue #1112 / audit RB-05).

`cluster_id` (also on list items) records the cluster a run was routed to, persisted at dispatch. It is `null` for local runs and for runs that carry no cluster routing fact (retry/resume re-execute locally). It is the durable source of truth for cancellation (see Cancel below): it survives a server restart, which the in-memory routing registry does not (issue #1257 / audit RB-05).

### Run Preview
```
GET /api/runs/{id}/preview
```
The instance-level dry-run plan (`checkpoint_preview` + execution order), persisted by the executor as `dry-run-preview.json` when a dry-run completes. Returns `404 NO_PREVIEW` for runs that never produced one — or when the workdir path is not a regular file (a rule-writable workdir may contain special files). Files above the 16 MiB server-written maximum return `413 PREVIEW_TOO_LARGE`.

### Protected-Tree Drift Preview
```
GET /api/runs/{id}/protected-drift
```
Read-only drift audit of the run's `[engine] protected_data_trees`. The response mirrors the CLI dry-run's `protected_tree_drift` object:

```json
{
  "drifted_files": ["raw/S1.fq: 1024 B @<baseline-time> → 0 B @<current-time> (TRUNCATED)"],
  "new_trees": 0,
  "warnings": []
}
```

- `drifted_files` — one human-readable line per drifted file (removed / added / changed, with `(TRUNCATED)` and `sha256:` labels — the same `ManifestChange::describe` rendering as the CLI dry-run and the run-gate refusal).
- `new_trees` — trees declared under `[engine] protected_data_trees` that exist but have no baseline yet. The next real `oxo-flow run` adopts them; this preview never does.
- `warnings` — per-tree problems that did not block the audit (unreadable tree, ignored empty path values).

The preview is **read-only**: unlike `oxo-flow run`, which persists new baselines into the checkpoint, nothing is recorded here — repeated calls are stable, and the checkpoint file is never created or modified. A run that is clean but not yet baselined reports empty `drifted_files` with a non-zero `new_trees`. The audit reads the tree as it exists **now**: files can still change between this call and the next run, which is exactly the window `allow_protected_drift` on Create Run covers. The declared trees come from the workflow the run executed (the persisted resolved snapshot / the run workdir sources, with the host document as fallback for single-document runs). It **fails closed**: if no workflow source can be loaded, the endpoint answers `500 {"code": "WORKFLOW_UNREADABLE", …}` instead of reporting a clean empty result — "could not read the declaration" is never presented as "no drift". Returns `404` for unknown or foreign runs; `503` when runs are not available on this backend.

### Provenance Verify
```
GET /api/runs/{id}/provenance-verify
```
Audit the output-file checksums a run recorded into its checkpoint (CLI→web parity, `oxo-flow --json provenance verify <checkpoint>`). The endpoint shells out to the CLI binary against the run's `workdir/.oxo-flow/checkpoint.json` and forwards the verdict:

```json
{
  "checkpoint": "/abs/workdir/.oxo-flow/checkpoint.json",
  "matched": 1,
  "mismatched": 0,
  "missing": 1,
  "cleaned": 0,
  "ok": false,
  "entries": [
    {"file": "gone.txt", "status": "missing"},
    {"file": "out.txt", "status": "matched", "expected": "sha256:…", "actual": "sha256:…"}
  ]
}
```

- `entries` — one per audited file, flat (the CLI nests them under `verify` in its `--json` document); `status` is `matched`, `mismatched`, `missing` (vanished output), or `cleaned` (temporary output swept by the end-of-run gate). Checksums use the CLI's `sha256:<hex>` form.
- `ok` — `true` only when `mismatched == 0 && missing == 0` (mirrors the CLI's exit 1 = drift policy; drift is a 200 finding here, not an HTTP error).
- `note` — present only when the checkpoint carries no stored checksums (the run was created without `provenance: true`): the CLI's hint to re-run with `--provenance`, plus `matched: 0` and empty `entries` (`ok: true`).

Checksum tracking is opt-in: pass `"provenance": true` on Create Run (forwarded as the CLI's `--provenance`) so the run records `sha256` checksums of its outputs into the checkpoint. The endpoint is **read-only** — verification never mutates the checkpoint. The checkpoint lives in the run's own workdir (`runs/<run_id>/.oxo-flow/checkpoint.json`), which no later run writes to, so a run's recorded checksums stay auditable for the life of the run tree (issue #1130 / audit RB-02); only `retry` and `resume-checkpoint` continue in that directory, by design. Errors: `404 NOT_FOUND` (unknown/foreign run), `404 NO_WORKDIR` (run has no workdir), `404 NO_CHECKPOINT` (no checkpoint under the workdir), `400 INVALID_CHECKPOINT` (the CLI rejected the checkpoint as malformed), `500 PROVENANCE_VERIFY_ERROR` (CLI output unparseable — usually a stale binary; set `OXO_FLOW_BIN` to the current one), `503` when runs are not available on this backend.

### AI Status
```
GET /api/runs/{id}/ai-status
```
Node-level execution statuses pulled from the run's checkpoint — the input the AI endpoints (explain/interpret) consume.

### Run Report
```
GET /api/runs/{id}/report
```
The deterministic report object the report page renders: run identity, node statuses with timings, output file tree. Zero AI — the same object answers `report/ask` and feeds `report/visualize`. Like the diagnostics endpoint, the report summarizes only the newest 256 KiB of `execution.log`; the full log stays on `GET /api/runs/{id}/logs`. The report is **failure-aware**: a failed run's narrative headline names the failed rules and exit codes, and carries `run_status` + `failed_rules: [{rule, exit_code, stderr_tail, stdout_tail}]` from the checkpoint's structured failure data.

### Report Q&A
```
POST /api/runs/{id}/report/ask
Content-Type: application/json

{"question": "which rules failed and why"}
```
Answers from the deterministic report (pattern matching over failed nodes, durations, file tree) — not an LLM call. Returns the answer string. On a failed run, EVERY question answers from the failure data (rule, exit code, bounded stderr excerpt — falling back to the stdout tail when stderr is empty) — never a completion claim.

### Run Status exit codes and failure tails
`GET /api/runs/{id}/status`, `/dag-status`, and `/instances` populate `exit_code` and the terminal timing fields from the checkpoint's `rule_runs` records: a rule whose record carries a code (a completed rule's `0`, or a failure's recorded code) reports it, plus bounded `stderr_tail` and `stdout_tail` on failed nodes (some tools print their root cause on stdout). A rule the engine settled **without** an accounting record — a synthetic up-to-date row, a pre-upgrade checkpoint, or cluster accounting a backend could not settle (RB-06) — reports `exit_code: null`: unknown is never rendered as a fabricated `0`, and the monitor shows `-`. The AI monitor (`/ai-status`) receives the same tails and classifies failures through the shared `DiagnosticsEngine` pattern table before falling back to exit-code-only signatures; its alerts carry a stable `alert_id` for cross-poll dedup. The AI chat `get_run_status` tool carries the same fields.

### Report Visualization
```
POST /api/runs/{id}/report/visualize
Content-Type: application/json

{"type": "files" | "durations" | "volcano"}
```
Chart data built from real sources: `files` maps the run's file tree (`{name, size_bytes}`), everything else maps per-rule durations from the checkpoint. No canned rows.

### Smart Retry
```
POST /api/runs/{id}/retry
Content-Type: application/json

{"skip_succeeded": true}
```
The retry spawns a real run, so it pays the same runs-per-minute
limiter and the same atomic quota admission as run creation — a retry that
exceeds the budget answers `429 RUN_RATE_LIMITED` / `QUOTA_EXCEEDED`. It
executes in the **predecessor run's workdir** (like `resume-checkpoint`):
retry is an explicit continuation of one run lineage, not a new entry in run
history, so it is the one path that writes into an existing run tree by
design. A fresh `POST /api/runs` never does — it always gets its own workdir
(issue #1130).
The endpoint accepts **no repair payload**: the diagnostics `fix_action`
shown by the monitor is advisory (placeholder values, nothing applies it),
so the web UI labels its action "Retry unchanged" rather than claiming a
fix was applied. See
[Diagnostics Engine → Failure-panel actions in the web monitor](diagnostics-engine.md#failure-panel-actions-in-the-web-monitor).
Like every `ApiResult` error, the 429 carries no `Retry-After` header
(the typed tuple cannot carry headers) — the countdown lives in the
error `message` instead (`retry in {wait_secs}s`).
`skip_succeeded` (default `true`) chooses which rules re-execute. The
returned plan and the spawned CLI flags are derived from the same mapping
(`retry_spawn_flags`), so the promise and the execution cannot drift:

- `skip_succeeded: true` — the retry spawns with `--resume-failed` (no
  `--rerun`). Failed and cancelled rules re-execute, the checkpoint's
  cascade invalidation re-runs their downstream dependents, and completed
  rules outside that closure keep their checkpoint records and are skipped.
  `will_rerun` lists the re-executed set; `will_skip` lists the skipped
  completed rules.
- `skip_succeeded: false` — the retry spawns with `--rerun`, which
  re-executes EVERY rule regardless of checkpoint state. `will_rerun` lists
  every rule in the workflow (DAG order) and `will_skip` is empty — the same
  set the execution runs.

`from_rule` (issue #1017): when set to a rule name (e.g. `"b"`), the retry
re-runs from that rule **downstream** — the engine computes the
transitive-dependent closure of the named rule in the DAG and invalidates
their checkpoint entries before the freshness gate runs, so downstream rules
re-execute with the new upstream outputs instead of silently passing the
freshness gate with stale artifacts. The spawned CLI receives
`--from-rule <name>` directly (the same closure computation the CLI's own
`--from-rule` flag runs — one engine, two surfaces). Failed and cancelled
rules in the named rule's upstream target set have no success checkpoint
record, so they re-execute as well; the plan lists them in `will_rerun`
alongside the downstream closure. `--from-rule` subsumes
`skip_succeeded` for the re-executed set: completed rules outside that set
keep their checkpoint state, so the plan lists them in `will_skip`
regardless of `skip_succeeded`. A `from_rule` naming a rule not in the DAG
surfaces as a `400 RuleNotFound` from the spawned CLI's own DAG walk.

The retry **really executes**: the returned `new_run_id` is a real run in
the database (same workdir, same owner), spawned with the flag pair above so
the rules the plan lists re-execute despite their existing outputs and the
checkpoint's cascade invalidation re-runs their downstream dependents.
The retry **replays the original run's `config_overrides`**:
the new row carries the stored overrides forward and the spawned CLI
receives the same `--arg=KEY=VALUE` entries, so a retry re-executes with the
configuration it claims to redo instead of silently falling back to
defaults.
Returns `{ new_run_id, will_rerun: [...], will_skip: [...] }` — plus an
optional `note` when the plan is empty on a previously failed run (it died
before executing any rule, e.g. a configuration error at spawn; the retry
re-fails identically unless the error is fixed).
When `from_rule` is set, the plan's `will_rerun` list contains the named
rule plus every transitive dependent (the downstream closure) and any
failed/cancelled rule in its upstream target set (those re-execute too,
since they have no success checkpoint record), and `will_skip` contains the
remaining completed rules — the same set the engine will execute at run
start.

### Cancel
```
POST /api/runs/{id}/cancel
```
Cancels a running/pending run. The response is
`{run_id, status, cancelled_at, unresolved_jobs}`.

A **remote** run is cancelled against the cluster recorded on its persisted
`runs.cluster_id` column, so cancellation survives a server restart — the
in-memory routing registry is consulted only as a fast path, not as the
source of truth (issue #1257 / audit RB-05). Because a routed job can outlive
the wrapper that launched it, the endpoint no longer reports a best-effort
signal as an unqualified success: every job the cancel path signalled but
could not **confirm** dead appears in `unresolved_jobs`, an array of
`{run_id, cluster_id, reason}` (empty when every target was confirmed gone).
This covers a remote process that survived `SIGTERM`/`SIGKILL`, a cluster
whose configuration was deleted out from under the run, and a DB error
resolving the cluster. Treat a non-empty `unresolved_jobs` as "the kill was
attempted but not verified" and follow up on the named job.

### Pause / Resume
```
POST /api/runs/{id}/pause
POST /api/runs/{id}/resume
```
Pauses a running run (`{"reason": "..."}` optional; other body fields are
ignored) and resumes it. On **resume**, a `from_rule` field is rejected with
`400 UNSUPPORTED_FIELD` — a paused run continues in place, so re-running from a
specific rule has no meaning (see Smart Retry above: cancel the run and retry
it instead). A terminal run in either endpoint returns `409 RUN_NOT_ACTIVE`.

Both endpoints signal the run's live process group, so a row with no
registered group is refused with `409` rather than flipped in the DB:
**pause** answers `409 NO_PROCESS_GROUP` (a queued run has nothing to freeze)
and **resume** answers `409 RUN_NOT_RESUMABLE` (a paused run whose process has
exited has nothing to unfreeze — writing `running` would show it executing
with nothing behind it). In both cases the stored status is left unchanged.

The status flip itself is a compare-and-set: **pause** only moves a row that is
still `running`, and **resume** only moves a row that is still `paused`. If the
row leaves that state in the window between the handler's read and its guarded
write, the transition loses the race and the endpoint answers
`409 RUN_STATE_CHANGED` without rewriting the row. A losing request re-reads the
row and reconciles the process group with what it finds, so row and process
cannot disagree. It unfreezes the group (`SIGCONT`) only if the run is no longer
`paused` — i.e. it went terminal; if it observes `paused` the freeze came from a
concurrent **same-target pause** that won, and the loser leaves it frozen. It
re-freezes the group (`SIGSTOP`) only if the run is no longer `running` — i.e. it
went terminal; if it observes `running` the unfreeze came from a concurrent
**same-target resume** that won, and the loser leaves it running. The two
handlers are therefore idempotent under concurrent same-target requests and never
leave a terminal row whose process is frozen (or a paused row whose process is
running).

### Logs
```
GET /api/runs/{id}/logs
```
Returns the execution log text — bounded to the newest 8 MiB with an explicit `[log truncated …]` marker line when the log is larger (buffering a multi-GB log per request was a memory-amplification surface). Read the full file from the run workdir when the marker appears.

### Results
```
GET /api/runs/{id}/results
```
Returns output file tree with sizes and types.

### Files — download / preview / zip
```
GET /api/runs/{id}/files?path=<relative-path>
GET /api/runs/{id}/files?path=<relative-path>&preview=true
```
The read-only result-delivery layer:

- **file** → bytes with `ETag`, `Content-Disposition: attachment`, and
  single-range support (`Range: bytes=a-b` → 206; malformed/multi-range
  requests degrade to the full body per RFC 9110)
- **directory** → a streaming STORE-mode zip (no temporary archive)
- `preview=true` → truncated JSON for text-ish formats (100 KB cap) or
  inline image bytes; other types return `415 NO_PREVIEW`
- paths are sandboxed to the run's workdir (traversal rejected); sensitive
  filenames (.env, keys, credentials) are never served
- browser downloads use the SPA's authenticated `fetch` path (`Authorization: Bearer`
  header → Blob → object URL); no token is placed in a download URL's query
  string, and an unauthenticated `401` redirects the SPA to the login page
  instead of saving an empty file (WEB-14 / issue #1139)

### Instances
```
GET /api/runs/{id}/instances
```
The sample×rule instance table: every expanded instance the checkpoint
knows about (`qc_auto-discovered_S1` → rule `qc`, group `auto-discovered`,
sample `S1`) with status, duration, and exit code — answers "which sample
under which rule failed".

### Upload & list user inputs
```
POST /api/files        # multipart: field "path" (optional subdir) + file parts
GET  /api/files        # list the acting user's uploaded inputs
```
Uploads land in `workspace/users/<user>/inputs/` (chunked to disk, 8 GiB
per-file cap).

---

## Data Discovery

### Analyze Data
```
POST /api/data/analyze
Content-Type: application/json

{"paths": ["/data/*.fastq.gz", "/data/*.bam"], "max_depth": 2}
```
Deterministic file scanning + format inference + pipeline recommendation. Returns `{ files: [{ path, size, format, format_confidence, paired_with?, sample_name? }], summary, suggested_workflow }`. Format detection uses filename extension matching — **not AI**. `paired_with` echoes the mate's client-supplied path when the request contains a mutual pair (`_R1`/`_R2` or `_1.`/`_2.` in the file name) and both files share a detected, non-unknown format; it is `null` otherwise.

### Reference Discovery
```
POST /api/data/reference
Content-Type: application/json

{"genome": "hg38", "components": ["fasta", "gtf", "star_index"]}
```
Finds installed reference genome components and reports missing ones with download commands.

### Reference Status
```
GET /api/data/reference/status
```
Which common reference genomes have their expected components present at the standard `/data/references/` locations — the deterministic counterpart to the POST discovery above. Probed components per genome: `hg38` and `mm10` check `genome.fa`, `star` (STAR index directory) and `genes.gtf`; `mm39` checks only `genome.fa` and `star` (the `[[references]]` auto-build needs no annotation to construct an index). A genome counts as `installed` only when at least one probed component exists; builds with nothing on disk are reported under `missing_genomes`. Response shape (`components` is a legacy alias of `found` — both carry the absolute paths that exist on disk):

```
{
  "installed": [
    { "genome": "hg38", "components": ["/data/references/hg38/genome.fa", "..."],
      "found": ["..."], "missing_components": ["..."] }
  ],
  "missing_genomes": ["mm39"],
  "missing": ["/data/references/mm39/genome.fa"],
  "download_commands": ["# mm39: stage the expected files under /data/references/mm39 (genome.fa, STAR index) ..."]
}
```

`download_commands` carries staging *guidance*, not fetchable URLs: oxo-flow hosts no reference mirror. Point users at `reference_dir` + `[[references]]` auto-build in the workflow format for index construction.

### Data Perception
```
POST /api/data/perceive
Content-Type: application/json

{"paths": ["/data/"], "description": "RNA-seq paired-end human samples"}
```
Data-profiling report built from paths (the same scanner as `/api/data/analyze`) or from a natural-language description. Powers the "what data do I have" panel.

### Samplesheet Parsing
```
POST /api/data/samplesheet/parse
Content-Type: application/json

{"content": "sample,fastq_r1,fastq_r2,condition\nS1,a_R1.fastq.gz,a_R2.fastq.gz,ctrl"}
```
Parses pasted CSV/TSV samplesheet content into the standard column structure (`sample`, `fastq_r1`, `fastq_r2`, `condition`). Returns `{ format, samples: [...] }` — the bridge from a spreadsheet to `[[sample_groups]]`.

### Verify Data
```
POST /api/data/verify
Content-Type: application/json

{"tree": "inputs/delivery-2026-10", "md5_content": "d41d8cd98f00b204e9800998ecf8427e  raw/S1.fq\nd41d8cd98f00b204e9800998ecf8427e  raw/S2.fq\n"}
```
Audits a delivered data tree against a vendor md5 sidecar — the web slice of the CLI's `oxo-flow verify-data` command. `tree` is workspace-relative (uploads live under `inputs/`, run workdirs under `runs/<id>/`; absolute paths and `..` are rejected). The sidecar arrives inline in the standard `md5sum` text format and is staged server-side; the CLI subprocess streams an md5 over every file.

Response is HTTP 200 even when drift is found — the counts ARE the finding:

```
{
  "tree": "inputs/delivery-2026-10",
  "matched": 12, "mismatched": 1, "missing": 0, "extra": 1,
  "entries": [
    {"file": "raw/S1.fq", "status": "matched", "expected": "d41d...", "actual": "d41d..."},
    {"file": "raw/S2.fq", "status": "mismatched", "expected": "aa...", "actual": "bb..."},
    {"file": "raw/unlisted.txt", "status": "extra", "actual": "cc..."}
  ]
}
```

`status` vocabulary: `matched`, `mismatched` (includes unreadable files, with a `note`), `missing`, `extra` (present but unlisted — always non-compliant). An `extra` entry may carry a `note` instead of an `actual` checksum in two cases: the file could not be read for checksumming, or a subdirectory of the tree could not be traversed at all (`tree walk failed` note — the audit is incomplete, the named path may be a directory, and the verdict has already failed with exit-equivalent `extra > 0`). An empty sidecar returns all-zero counts with `"note": "The md5 sidecar lists no files."` and no extras scan. A sidecar the CLI hard-rejects (malformed line, duplicate path, traversal) yields `400 INVALID_SIDECAR` with the CLI's stderr as detail. HTTP errors are otherwise reserved for infrastructure problems: `400 INVALID_TREE` (unknown path or traversal), `500 VERIFY_ERROR` (spawn failure or unparseable CLI output). Errors use the structured `{code, message, detail, suggestion}` shape. The inline sidecar counts against the server's default ~2 MiB JSON body limit (~50k entries); larger sidecars should be split by directory.

---

## Templates

```
GET    /api/templates
POST   /api/templates
GET    /api/templates/{id}
DELETE /api/templates/{id}
```
Built-in and user-created pipeline templates. System templates are read-only. The list endpoint does not accept filter query parameters.

---

## Plugins

### Validate Plugin
```
POST /api/plugins/validate
Content-Type: application/json

{"manifest": {"name": "...", "version": "1.0", "plugin_type": "rule"}, "trusted_keys": {"key1": "hex..."}}
```
Validates a plugin manifest and optionally verifies its HMAC signature against trusted keys. Returns `{ valid, name, version, plugin_type, signature_valid, errors }`.

---

## AI (Phase 2 — calls deterministic APIs above)

```
POST /api/ai/translate          # NL intent → validated .oxoflow (JSON response)
POST /api/ai/translate/stream   # Same, streamed over SSE (progress → done events)
POST /api/ai/explain            # Explain run failure + suggest fix (run owner or admin)
POST /api/ai/interpret          # Interpret results with caveats (run owner or admin)
POST /api/ai/optimize           # Optimize pipeline parameters (pipeline reader via id; owner-supplied TOML unaffected)
GET  /api/ai/config             # Get AI provider configuration (public; authenticated admins see provider/model)
POST /api/ai/config             # Update the shared provider (admin-only outside personal mode)
POST /api/ai/test               # Test the provider (admin-only outside personal mode)
GET  /api/ai/config/user        # The acting user's private provider config
PUT  /api/ai/config/user        # Set the acting user's private provider config
GET  /api/ai/config/server      # Server-level provider config (admin-only outside personal mode)
PUT  /api/ai/config/server      # Set the server-level provider config
GET  /api/ai/config/effective   # The config a run would use: user → server → env fallback chain (endpoint/model redacted for non-admins)
GET  /api/knowledge/tools       # Tool catalog the AI can call
GET  /api/knowledge/skills      # Skill catalog the AI can follow
```

Both knowledge endpoints accept `?q=<query>&limit=<1-50>` (default 20).
`/api/knowledge/tools` treats an empty `q` as a browse: it returns the
first `limit` Bioconda entries. `/api/knowledge/skills` tokenizes the
query and ranks matches (rare terms first); an empty or unmatched `q`
returns an empty `skills` array — `total` is always the full catalog
size, not the hit count.

The translate endpoints take `{ "intent": "<natural-language description>",
"context": { "data_analysis_id": "<uuid>" } }` — `intent` is REQUIRED (sending
only a `prompt` field fails deserialization); `context` is optional.

`POST /api/ai/translate` (the non-streaming variant) budgets the WHOLE
generation — every tool and validation round — at
`OXO_FLOW_AI_TIMEOUT_SECS` (default 300 s). The route budget fires one
second before the provider client's per-request timeout, and a
timeout-class provider error is classified into the same answer, so the
documented response holds whichever timer expires first. When the budget
expires it answers `504 {code: "AI_TIMEOUT"}` with the standard
`{code, message, detail, suggestion}` body, and `suggestion` names the knob.
Use `POST /api/ai/translate/stream` for generations expected to run longer:
the SSE route is not capped by this total budget.

`GET /api/ai/config` stays public for anonymous feature discoverability:
non-admin callers outside personal mode (team/hpc) — including anonymous
callers — receive only
`{provider: "", model: null, api_url: null, is_configured}`. Personal mode
has no sign-in at all (loopback, single local user), so its callers always
receive the unmasked payload — the exception the SPA settings page relies
on. Presenting a
valid credential opts the request into normal authentication, so an admin
sees the configured provider, model, and URL (never the API key); an invalid
credential is rejected with `401` rather than silently downgraded to the
masked anonymous payload.

### Provider metadata is role-gated (WEB-15 / issue #1140)

Outside personal mode, provider metadata is admin-only:

- `GET /api/ai/config` is the public discoverability surface and stays the
  strictest mask: anonymous and non-admin callers receive only
  `is_configured` (`provider: ""`, `model: null`, `api_url: null`);
  authenticated admins see the full resolved configuration.
- `GET /api/ai/config/effective` is authenticated. Non-admin
  `user`/`viewer` callers receive the resolution chain's provider NAMES and
  enabled state, but `effective.model`, `effective.api_url`,
  `tiers.env_model`, `tiers.env_url`, `tiers.server_model`,
  `tiers.user_model`, and `tiers.user_api_url` are `null` — the endpoint
  URL identifies the internal AI proxy/gateway and the model id discloses
  the provider's shape. Admins receive the full chain.
- `GET /api/ai/config/user` returns a row the caller saved themselves in
  full (it is their own configuration). When no per-user row exists, the
  endpoint mirrors the SHARED runtime; that mirrored `api_url` and `model`
  are redacted for non-admins the same way.
- `GET /api/ai/config/server` and `PUT /api/ai/config/server` stay
  admin-only (403 `ACCESS_DENIED` otherwise), as does the runtime-only
  `POST /api/ai/config`. `PUT /api/ai/config/user` is **not** admin-gated:
  it writes only the acting user's own row (outside personal mode a
  non-admin's save is persisted but not applied to the shared runtime —
  `applied_to_runtime: false`).
- API keys and key prefixes are never returned by any of these endpoints.

Personal mode is loopback single-user (no roles), so its local callers keep
the unmasked view — the exception the SPA settings page relies on.
`GET /api/ai/config/effective` rejects an unauthenticated caller with the
normal `401` auth gate.

The config endpoints form a three-level resolution chain: a user's private
provider overrides the server-wide one, which overrides environment
variables. `GET /api/ai/config/effective` shows the resolved result and
which level supplied each field. The `effective.model` field is the model
the active backend was actually constructed with: an explicit config model
wins, otherwise the environment override (`ANTHROPIC_MODEL`,
`OPENAI_MODEL`, `OLLAMA_MODEL`, `OXO_FLOW_AI_MODEL`) or the provider
default is reported — it is `null` only when no provider is configured.
`GET /api/ai/config` returns the same resolved model. API keys are never echoed back — responses
mark `key_set` instead. Rows are encrypted at rest only when
`OXO_FLOW_MASTER_KEY` is configured; without it, the local database uses the
documented plaintext compatibility mode.

There is deliberately no DELETE endpoint: resetting a tier means saving
`provider: "disabled"` through the same PUT endpoints. A disabled user-tier
row makes that user fall back to the shared runtime; a disabled server-tier
row disables the shared runtime (immediately via the same reconfiguration
path, and again on the next startup when the row is restored). Omitting
`provider` in a PUT body defaults to `"disabled"`, so a save that only flips
a toggle must carry the current provider over (the bundled Settings page
does exactly that).

Provider names are validated before either configuration endpoint writes
to storage. An unsupported name returns `400 INVALID_AI_PROVIDER` and
leaves the existing configuration untouched. If storage succeeds but the
runtime cannot activate the new provider, the endpoint returns
`500 AI_RECONFIGURE_FAILED`; the durable row remains available for the
next startup, but the caller must correct the provider settings rather
than assuming the running process switched.

### Pinned disabled AI (`OXO_FLOW_AI_PROVIDER=disabled`)

An explicit `disabled` is an operator-level switch: it outranks every
configuration tier and every runtime update. While it is set:

- `POST /api/ai/config` (runtime-only — applying to the live registry is
  its only effect) returns `409 AI_DISABLED_BY_ENV` with the standard
  `{code, message, detail, suggestion}` shape.
- `PUT /api/ai/config/server` and `PUT /api/ai/config/user` still persist
  the row and return `200` with `applied_to_runtime: false`: Settings stays
  usable and the saved configuration applies after an unpinned restart,
  but the pinned runtime is never reconfigured.
- `POST /api/ai/test` returns `200` with `success: false`,
  `provider: "disabled"`, and a message stating that no connection was
  attempted. No provider is resolved and no HTTP client is constructed.
- `GET /api/ai/config` and `GET /api/ai/config/effective` report
  `provider: "disabled"` / `is_configured: false`. The per-user and
  server reads add `disabled_by_env: true` and report the effective
  `configured` as `false`; the stored row is still returned so the
  Settings page can show what is saved.
- Generation, chat, explain, interpret, and optimize keep answering
  `503 AI_NOT_CONFIGURED`.

Lifting the pin requires restarting the server without the variable (or
with a provider name); configuration saves are rejected, not queued,
while it is set.

See [AI Translation Layer](ai-translation.md) for details.

---

## Chat (v0.8 AI Companion)

```
POST /api/chat/send        # Streamed chat over SSE
POST /api/chat/send/json   # Same, single JSON response
GET  /api/chat/sessions    # The acting user's chat sessions
```

**Chat session persistence is best-effort** (WEB-15 / issue #1140): a
completed reply is persisted to the chat session store and the endpoint
still succeeds if that write fails — the failure is logged as a warning.
`GET /api/chat/sessions` returns an empty list when session storage is
unavailable or a read fails, so an empty history is not proof that no
conversation happened. The SSE/JSON response remains the source of truth
for the current turn; `message` on the `done` event repeats the chat-mode
reply so a lost stream frame cannot leave an empty bubble.

### Intent routing

Chat requests are classified before any generation work happens, by a
deterministic keyword/pattern classifier (`domains/chat/intent.rs`) — no
extra model call, so routing never doubles a request's token cost. The
decision is returned as the `mode` field:

| Mode | Trigger | Behavior |
|------|---------|----------|
| `pipeline` | Pipeline vocabulary (workflow/pipeline/rules/TOML/DAG, omics assays, file formats, tool names), run-diagnosis wording, an attached `data_paths`/`samplesheet`, or an explicit `run_id` scope | The existing `PipelineGenAgent` loop: validated `.oxoflow` TOML plus `toml_content`, `rules`, and `dag` |
| `chat` | Greetings/small talk, explanation questions ("What is FastQC?"), meta-instructions ("Reply with the single word OK.") — i.e. no pipeline vocabulary | One plain-text completion; `message` carries the reply; **no** pipeline generation |

The bias is deliberately conservative: a message naming a workflow, an
assay, a file format, or a tool routes to generation unless it is phrased
as an explanation question. `context.data_paths` / `context.samplesheet`
and a request-level `run_id` always force generation (data analysis and
run diagnosis need the tool loop). `context.intent` is a pipeline label
hint only — it never changes the route, so the SPA's page name cannot turn
conversational turns into pipelines.

### `POST /api/chat/send/json` — response shapes

Chat intent (one provider completion, no workflow):

```json
{ "mode": "chat", "message": "OK" }
```

Pipeline intent (the existing fields are unchanged; `mode` is added):

```json
{
  "mode": "pipeline",
  "pipeline_id": "…",
  "toml_content": "[workflow]\n…",
  "intent": "RNA-seq analysis",
  "data_report": null,
  "validation": { "valid": true, "errors": [] },
  "rules": [ … ],
  "dag": { "nodes": [ … ], "edges": [ … ], "parallel_groups": [ … ] }
}
```

### `POST /api/chat/send` — SSE event contract

The stream opens with a `ready` handshake, then forwards the real agent
loop (tool rounds, validation rounds) as typed events:

| Event | Payload | Meaning |
|-------|---------|---------|
| `ready` | `{keepalive_secs, idle_timeout_secs, mode}` | Handshake, sent before the first provider call. `keepalive_secs` is the comment-frame cadence; `idle_timeout_secs` (`3 × keepalive_secs`) is the **client watchdog budget** — silence longer than this means the connection is dead (proxies can drop it silently), and the client must abort and offer a retry rather than spin forever. `mode` is the intent route (`chat` or `pipeline`), so the client can pick its renderer up front. |
| `status` | `{message}` | Human-readable phase (`planning`, `executing tools`). Pipeline mode only. |
| `progress` | `{round, max_rounds, step, tool, elapsed_ms, prompt_tokens, completion_tokens}` | Structured loop progress, emitted at every phase boundary: `step` is `plan`, `generate`, `tool` (with `tool` naming the running tool), or `validate`. `elapsed_ms` is measured from loop start; token counts are cumulative. Clients render this to distinguish "slow" from "dead". Pipeline mode only. |
| `tool_call` | `{name, args}` | The model requested a grounded tool call. Pipeline mode only. |
| `tool_result` | `{name, summary}` | The tool finished; `summary` is a truncated preview (errors ride here too). Pipeline mode only. |
| `text` | `{chunk}` | One complete round response (token-level streaming is not implemented). In `chat` mode this carries the whole plain-text reply. |
| `action` | `{action_type, data}` | Structured action — `pipeline_ready` carries the validated TOML and its validation result. Pipeline mode only; chat turns never emit it. |
| `error` | `{code, message}` | Terminal failure (`CHAT_ERROR`, `AI_NOT_CONFIGURED`, `EMPTY_MESSAGE`). In pipeline mode the loop aborts and its transcript is archived with `outcome: "Failed"` and the reason. |
| `done` | `{session_id, mode, rounds, prompt_tokens, completion_tokens}` / `{session_id, mode: "chat", message}` | Terminal success marker. Pipeline mode: always sent after `action`, clients stop reading here. Chat mode: `message` repeats the full reply (so a lost `text` frame cannot leave an empty bubble), and the token counters are omitted. The early `EMPTY_MESSAGE` / `AI_NOT_CONFIGURED` error paths also terminate with `{session_id, mode}`. |

Transport behavior:

- Keep-alive **comment frames** (`: ping`) are emitted every 15 s. They are
  not events, but they are bytes on the wire — an inactivity watchdog must
  treat them as traffic so a slow but healthy generation (8–14 minute runs
  are normal) is never cut off.
- Each provider request is bounded by `OXO_FLOW_AI_TIMEOUT_SECS` (default
  300 s). A stalled endpoint surfaces as an `error` event within that budget,
  never as an indefinite hang.
- A client that disconnects mid-generation flips the loop's cancel flag; the
  orchestrator stops between provider calls and before each tool rather than
  burning paid rounds nobody is reading. The aborted run is archived (below).
- The browser-side watchdog is derived from `ready.idle_timeout_secs`; on
  expiry the client aborts the fetch, renders the error with a Retry action,
  and re-enables the send button.
- `POST /api/chat/send/json` (the non-streaming variant) is subject to the
  same whole-generation route budget as `POST /api/ai/translate` (described
  in the AI section above): every tool and validation round budgeted at
  `OXO_FLOW_AI_TIMEOUT_SECS` (default 300 s), firing one second before the
  provider client's per-request timeout, with `504 {code: "AI_TIMEOUT"}`
  when it expires. The SSE route above is not capped by this total budget;
  use it for generations expected to run longer.

### Chat session archives

Every agent-loop run — successful or aborted — is persisted under
`.oxo-flow/ai_sessions/` as `<timestamp>-<command>-<id>.json`; set
`OXO_FLOW_AI_SESSION_DIR` to redirect the archive (tests and CI point it at
a scratch directory so a run never writes into an operator's real archive).
Successful generations archive `outcome: "Success"`; a run that fails
archives `outcome: "Failed"` with an `error` reason (the post-mortem
record). A run aborted by a JSON route's total budget (the `504 AI_TIMEOUT`
path) also archives `outcome: "Failed"` with the timeout reason — the route
timer drops the orchestrator future before it can archive its own failure,
so the route writes the synthetic record. Writes are temp-sibling + atomic
rename, so a reader never observes
a truncated file. Pruning
only reclaims archives older than ten minutes, so a fresh failure record — or
the file an operator is inspecting — is never rotated out by concurrent
writers. Chat-intent replies do not run the agent loop and therefore produce
no archive; they are appended to the chat session store like every other
assistant message.

### AI provider probe

`POST /api/ai/test` is **single-flight**: concurrent callers share one
outbound provider round-trip (queued behind an async mutex) and a result
younger than 10 seconds is reused. The probe calls the provider directly and
never writes an `ai_sessions` archive, so repeated settings checks cannot
pollute the audit trail or trip the provider's rate limiter.

---

## OAuth

```
POST /api/auth/oauth/authorize   # Begin the OAuth flow → provider authorize URL
POST /api/auth/oauth/callback    # Provider callback → session token
```

The redirect URI is `OXO_FLOW_OAUTH_REDIRECT_URI` when set, with a fixed
fallback of `http://localhost:3000/api/auth/oauth/callback` (see the
environment table in `AGENTS.md`). Both endpoints resolve it with the same
precedence — an explicit request value, then the environment variable, then
the fallback — and the callback may echo it as `redirect_uri`; whichever
value the callback presents must equal the one the state was issued for.

### State binding and single use (issue #1141)

`POST /api/auth/oauth/authorize` issues a CSRF `state` bound server-side to
`(provider, redirect URI, issued-at timestamp)`. The callback verifies AND
consumes it in one atomic delete, before any token exchange:

- unknown, malformed, or already-consumed (replayed) state →
  `400 {code:"OAUTH_INVALID_STATE"}`;
- a state older than the 10-minute TTL → `400 {code:"OAUTH_INVALID_STATE"}`
  with an "expired" message;
- a state issued for a different provider or a different redirect URI →
  `400 {code:"OAUTH_STATE_MISMATCH"}`.

A mismatched binding still consumes the state: it can never be retried, and
two concurrent callbacks can never both validate the same value.

### Browser binding (issue #1197)

The state is also bound to the browser that started the flow, closing the
login-CSRF variant where an attacker starts an authorize flow, obtains a
valid state, and tricks another browser into completing the callback:

- `POST /api/auth/oauth/authorize` generates a random per-flow binding value
  and returns it as the `oxoflow_oauth_binding` cookie — `HttpOnly`,
  `SameSite=Lax`, `Path` scoped to the OAuth routes (the parent of the
  request path, e.g. `/api/auth/oauth`), and `Max-Age=600` (the state TTL).
  `Secure` is added only when the browser-facing callback origin is https;
  a plain-http localhost deployment never carries it, because browsers drop
  `Secure` cookies on http. The raw value is never returned in the response
  body; only its SHA-256 digest is stored next to the state row
  (`oauth_states.binding_hash`, with idempotent SQLite and PostgreSQL
  migrations).
- `POST /api/auth/oauth/callback` requires the cookie to hash to the stored
  binding. Missing, mismatched, or empty bindings — and historical rows
  written before the migration (empty digest) — answer the same
  `400 {code:"OAUTH_STATE_MISMATCH"}` as a provider/redirect mismatch. A
  browser-binding mismatch does not say which component failed; the binding
  is checked before the provider/redirect bindings for that reason. (The
  provider and redirect checks themselves name the expected value, by
  design — that is what makes a misconfiguration diagnosable.)
- A binding mismatch still consumes the state (single use). Handler-generated
  callback responses clear the cookie on success and failure; a request
  rejected by body extraction (malformed JSON → `400 INVALID_BODY`) answers
  before the handler, so that short-lived cookie simply expires.

The shipped SPA has no OAuth sign-in UI. A browser client that implements
the two steps must use same-origin requests so the browser stores and
returns the cookie automatically; non-browser API clients must capture the
`Set-Cookie` value from the authorize response and send it back on the
callback request.

Scope: this proves that a callback can only complete a flow started by the
browser presenting the same binding value — cross-browser completion and
cookie-less replay are rejected by the tests below. It is not a claim that
login CSRF is impossible in every threat model (e.g. an attacker able to
write cookies into the victim's browser is outside this control).

### Redirect allowlist

The redirect target's **origin** must appear in a server-side allowlist
derived from the origin configuration the server already trusts — there is
no separate OAuth-redirect setting:

- `OXO_FLOW_ALLOWED_ORIGINS` (comma-separated), or the built-in localhost
  origins when it is unset — the same list the CORS layer uses;
- the origin of the server-configured `OXO_FLOW_OAUTH_REDIRECT_URI` (or the
  documented fallback), which every working deployment needs;
- the origin declared by `OXO_FLOW_PUBLIC_URL`, when set (reverse-proxy
  deployments).

Paths are not constrained — the provider's registered callback URI pins the
path, while the origin is what stops a crafted authorize request from
sending the authorization code to another host. An off-list target is
rejected with `400 {code:"OAUTH_REDIRECT_NOT_ALLOWED"}` at authorize time,
and again at callback time (defense in depth) if the configuration changed
between issue and callback.

The local user record is idempotent on the provider identity: the canonical
users row is keyed by the namespaced id `oauth:{provider}:{provider_user_id}`
(e.g. the ORCID iD), so re-login reuses the same row. Display names are NOT
unique across provider accounts — two ORCID accounts can share a given name —
so when a display name is already taken the row is persisted under a fallback
username derived from the unique provider id (`{provider}:{provider_user_id}`);
the login still succeeds and the session always resolves to a real user row.
When the user row cannot be ensured (database unavailable or insert fails),
the callback answers `503 {code:"DB_ERROR"}` instead of minting a session
that would fail closed on every subsequent request.

---

## Collaboration (Phase 3)

```
POST /api/pipelines/{id}/fork    # Fork into workspace (owner = session user)
POST /api/pipelines/{id}/share   # Share (link/public only; URL follows the actual transport)
POST /api/pipelines/import       # Import from oxo+http(s):// share URL
GET  /api/share/{token}          # PUBLIC landing payload (no session required)
```
`GET /api/share/{token}` powers the share landing page: pipeline identity,
DAG rule order, TOML, owner, expiry, and the most recent terminal run —
the token itself is the authorization. Expired links return `410`.

Share URLs minted by `POST .../share` use `oxo+http://` by default (the
server has no native TLS) and `oxo+https://` only when `OXO_FLOW_PUBLIC_URL`
declares a public HTTPS origin; the import endpoint accepts both schemes.
An override that carries its own mount path (e.g.
`https://flows.example.com/oxo-flow` behind a sub-path proxy) replaces the
local `--base-path` prefix instead of being appended to it.

See [Collaboration](../how-to/collaboration.md) for details.

### Version History
```
GET  /api/pipelines/{id}/revisions        # Snapshot list (newest first, ≤ 50)
GET  /api/pipelines/{id}/revisions/{rev}  # One snapshot's full TOML
POST /api/pipelines/{id}/rollback         # {"revision_id": ...} — restore
```
Every save/update snapshots the previous content; rollback preserves the
current version as a new revision (nothing is lost).

Snapshot insertion is transactional with the pipeline write (issue #1136):
on create the row and its initial snapshot, and on update/rollback the row
write and its pre-write snapshot (plus the 50-row retention prune), commit
in one storage transaction. If the snapshot insert fails, the pipeline
write rolls back and the request fails — a successful response can never
be missing its revision row. The snapshots and the compare-and-swap both
run on SQLite and PostgreSQL. (The pre-existing `save_pipeline` upsert
used by fork/import does not write revision rows; fork/import copies carry
`revision = 1` and no history.)

## Run Administration

```
POST /api/runs/{id}/clean              # CLI clean on the run's workdir
POST /api/runs/{id}/resume-checkpoint  # CLI resume from .oxo-flow/checkpoint.json
```
`resume-checkpoint` continues an unfinished run in place as a NEW run row
(`{"max_jobs": 2}` optional). Both are ownership-checked like every other
run endpoint.

## Webhooks

```
GET /api/webhook   # admin-only outside personal mode — { enabled, url, secret_set, events, signature_scheme }, secret never echoed
PUT /api/webhook   # admin-only outside personal mode — {"enabled": true, "url": "https://...", "secret": "...", "events": [...], "signature_scheme": "..."}
```
Runs POST a signed payload to the configured URL on terminal states. The
signature scheme is configurable:

- `hmac-sha256` — RFC 2104 HMAC (`X-OxoFlow-Signature: hmac-sha256=<hex>`).
  This is the default for a new web-server row and for a workflow `[webhook]`
  section.
- `sha256-keyed` — frozen `sha256(secret‖body)` compatibility. Existing
  web-server rows keep this scheme when an update omits `signature_scheme`;
  it is not the default for a new row.

An unknown or non-string `signature_scheme` returns
`400 INVALID_SIGNATURE_SCHEME` rather than being silently coerced. The PUT
response and subsequent GET report the scheme actually stored. The settings
UI exposes the choice and labels the legacy scheme as compatibility-only.

`events` is an exact subscription: a terminal run posts only the event it
actually reached, so `["workflow_completed"]` stays silent on a failed run
and `["workflow_failed"]` stays silent on a successful one. An empty list
(a saved `[]`, or a row predating per-event filtering) subscribes to both
terminal events.

Validation on `PUT`: `url` must parse as an absolute
`http://` or `https://` URL with a host (`400 INVALID_URL` otherwise — a
bare `https://` prefix no longer passes). Enabling delivery requires a
usable signing secret: either supply `secret` in the request or leave a
previously stored one in place (`400 WEBHOOK_SECRET_REQUIRED` otherwise) —
a webhook URL is a bearer credential, and unsigned delivery lets anyone who
learns the URL forge events. Disabling (`enabled: false`) never demands a
secret. A stored secret that fails to decrypt (e.g. the master key was
rotated) reads as absent: enable is refused until a fresh `secret` is
supplied — silently signing with an empty key would
produce forgeable deliveries.

Admin-only outside personal mode — the endpoint is shared infrastructure.

## API Keys

```
POST   /api/auth/keys       # {"name": "ci-bot"} → { id, name, key } (shown once)
GET    /api/auth/keys       # The acting user's keys (hashes only)
DELETE /api/auth/keys/{id}  # Revoke immediately
```
Machine credentials: send `X-API-Key: oxo_...` instead of a Bearer session.
Keys resolve to the same ownership context (a key's requests see exactly
what its owner sees), are stored as SHA-256 hashes, and revocation is
immediate.

## Quota

Runs are admitted against a **durable, atomic** per-user quota. The limits
check and the run's resource reservation commit together in the storage
backend (SQLite/PostgreSQL), so two concurrent `POST /api/runs` can never
both pass a limit of one; counters survive server restart, and every server
instance pointed at the same database enforces the same numbers (RB-09 /
issue #1133). Over-limit requests get `429 QUOTA_EXCEEDED` with the
violation list, and a rejected request never inserts a run row.

```
GET  /api/quota           # current limits and usage
PUT  /api/quota           # update limits (admin-only; personal mode bypasses admin checks)
```

`PUT /api/quota` accepts `{ max_concurrent_runs, max_total_threads, max_total_memory_mb, max_runs_per_day }` — all four fields are required (no defaults applied server-side). The limits are persisted through the storage backend (one singleton row), so they survive restart and apply to every instance; on a server running without storage the update stays process-local.
Usage is visible at `GET /api/quota` (`{ enabled, limits: {…}, usage: { active_runs, used_threads, used_memory_mb, runs_today } }`) and is read from the durable counters. `runs_today` is a rolling 24-hour window that restarts lazily on the next admission (a restart does not reset it). Every terminal path (completed/failed/cancelled) releases the run's reservation idempotently; reservations whose run row is terminal or missing are reclaimed at server startup.

### Declared-resource enforcement

Declared resources are enforced, not only counted:

- **Memory, Linux local runs**: when a workflow declares memory
  (`[rules.resources] memory`, or the deprecated per-rule `memory`), the
  executor installs `RLIMIT_DATA` on the spawned CLI and every rule
  subprocess at the run's declared total (with a 512 MB floor so the engine
  itself can start). A process that allocates past the ceiling fails the
  allocation, so the rule dies and the run fails — observable rejection,
  not an advisory overage. Runs that declare no memory have no claim to
  enforce.
- **Operator-wide CPU / process ceilings (web spawn path, Linux)**: on top of
  the declared-memory ceiling the web spawn path can install two
  operator-wide `pre_exec` limits on the spawned CLI (and therefore its whole
  process tree), configured through the server environment rather than the
  workflow:

  - `OXO_FLOW_WEB_RLIMIT_CPU_SECS` — `RLIMIT_CPU`, in CPU-seconds.
  - `OXO_FLOW_WEB_RLIMIT_NPROC` — `RLIMIT_NPROC`, a per-real-uid process cap.

  Both default to unset, and `0` (or any unparseable value) means "do not
  install" — nothing changes unless the operator opts in, so a typo can never
  silently disable the ceiling. A process past `RLIMIT_CPU` is sent `SIGXCPU`
  (then `SIGKILL`), so a runaway rule fails observably instead of pinning a
  server core. `RLIMIT_NPROC` counts every process owned by the server's real
  uid, not just this run's tree — set it well above the server's steady-state
  process count (it is a fork-bomb backstop, not a per-run quota). Both are
  Linux-only; off Linux the install is a no-op.

- **Child environment allowlist (web spawn path)**: the spawned CLI no longer
  inherits the server's environment. The web spawn path reuses the engine's own
  rule-shell policy (`apply_rule_env_allowlist`), so the child sees only
  `PATH`/`HOME`/`LANG`/`LC_*`, the engine's non-secret configuration knobs
  (e.g. `OXO_FLOW_TMPDIR`, `OXO_FLOW_MODULE_CACHE`, `OXO_FLOW_STRICT_PLACEHOLDERS`,
  `OXO_FLOW_BIN`), and the explicitly declared passthrough names — never
  `DATABASE_URL`, `OXO_FLOW_MASTER_KEY`, provider keys, or MCP tokens. The
  passthrough contract is reproduced end-to-end: the server forwards both
  the `OXO_FLOW_ENV_PASSTHROUGH` list and the named variables themselves, so
  the CLI child's rule shells re-admit exactly the values an operator's
  direct `oxo-flow run` would see. This is
  the same policy the local executor already applied to rule shells, so the web
  path is no longer the weaker surface.
- **Other platforms**: Darwin rejects `RLIMIT_DATA`/`RLIMIT_AS` changes with
  `EINVAL`; there the local ceiling remains advisory (the quota counters
  still apply). See the follow-up tracking issue for portable enforcement.
- **Remote (cluster) runs**: the local process-tree rlimit cannot travel to
  the cluster host, so the remote wrapper instead forwards the declared
  ceilings as engine flags — `--max-memory <MB>` and `--max-threads <n>`,
  emitted only when the workflow declares them (a `0`/unset ceiling
  auto-detects remotely, exactly like a local run with nothing declared).
  On the cluster host those drive the engine's scheduler `ResourcePool` and
  `check_budget_feasibility` gate, so a rule requiring more than the declared
  total hard-fails before it is scheduled. The mechanism differs from a local
  Linux run (rlimit vs. scheduler budget) but both cap the same declared
  total; the remote path additionally applies a thread ceiling, which the
  local web path does not (the engine auto-detects there).

## Cluster Connections & Remote Execution

```
GET    /api/clusters                  # configured SSH connections
POST   /api/clusters                  # upsert (admin-only outside personal mode)
DELETE /api/clusters/{id}
POST   /api/clusters/{id}/probe       # SSH connectivity + scheduler detection (admin-only outside personal mode)
```
The stored `ssh_key` is a secret: it is never returned by any
response (including the upsert echo) — clients see `ssh_key_set: true |
false` instead. When `OXO_FLOW_MASTER_KEY` is set, the key is sealed at
rest with the same AES-256-GCM scheme as AI provider keys; without it, the
database stores plaintext compatibility-mode rows.
`ssh_key` accepts a **path or key material**: material is written to a
private `0600` temp file for the SSH invocation, so either form works.
`POST /api/runs` accepts `cluster_id`: the run then stages its workdir to
the remote host (tar over stdio — no rsync), launches it detached (own
session via `setsid`, with `nohup` fallback) so the launch call returns
while the workflow keeps running, and pulls the results back on completion
so every downstream endpoint (logs, files, report) works unchanged. The
pulled stream is extracted through the engine's validated archive reader:
absolute/`..` member paths and link targets that are absolute or contain
any `..` component are rejected by name, every member is verified on disk,
and `execution.log`, `.exit-code`, plus a run-specific completion record
tarred **after** the result tree must be present — a truncated stream
cannot carry the record (issue #1157 / audit CL-04). If that pull fails,
the run is **not** finalized as a success — it is failed with the pull
error and the run row records `artifacts_incomplete` (see
[Run Detail](#run-detail)), because the local workdir the downstream
endpoints read may be missing or stale (issue #1112 / audit RB-05).
A pulled run is finalized as a success only after its declared outputs are
re-verified locally: once the transfer is intact, the server loads the run's
resolved-snapshot workflow, expands its wildcards exactly as the remote CLI
did, and checks every declared output of every **completed** rule through the
engine's single output-existence oracle (`rule_outputs_exist_with_patterns`,
so `{sample}` / `[[values]]` fan-out and `output_pattern` rules are covered
concretely). A completed rule whose declared output is absent from the pulled
tree fails the run through the same path as a broken pull — it is failed and
the row records `artifacts_incomplete` naming the rule and the missing path —
so the remote engine's exit-0 is no longer the sole contract (issue #1282 /
audit CL-04 + RB-05). When the workflow or checkpoint cannot be resolved the
re-check is skipped rather than failing a legitimate run. Cancel kills the
wrapper's recorded process group, so the remote CLI and rule subprocesses stop
rather than being orphaned. See
[Run on a cluster](../how-to/run-on-cluster.md).

---

## HPC

```
GET /api/hpc
```
Returns scheduler status (SLURM, PBS/Torque, LSF, SGE), available queues, and node count. Nodes are deduplicated by name (SLURM lists a node once per partition it belongs to). This route is only mounted when the server runs in `hpc` mode, and the generated OpenAPI document only advertises it in that mode.

---

## DAG Editing

```
POST /api/pipeline/{id}/command   # Apply an edit command (add/remove rule, connect, ...)
POST /api/pipeline/{id}/undo      # Revert the last edit (JSON body: {"toml_content": ...})
POST /api/pipeline/{id}/redo      # Re-apply the last undone edit (JSON body: {"toml_content": ...})
```

`undo`/`redo` require a JSON body carrying the caller's current TOML; a
recorded transition is applied only when it matches that state exactly.

**Scope, caps, and lifetime (issue #1137).**

- **Scope** — history is keyed by the acting identity *and* the pipeline id.
  The identity is the same `users.id` the other pipeline handlers resolve
  (the authenticated session/API-key user; the `default` pseudo-user in
  personal mode). Two users editing the same pipeline id own independent
  stacks and cannot see or undo each other's edits.
- **Caps** — at most `MAX_EDIT_HISTORY_DEPTH` (50) transitions per stack;
  the oldest is dropped on overflow. At most `MAX_EDIT_HISTORY_PIPELINES`
  (256) stacks are kept process-wide; recording an edit for a new
  identity/pipeline pair evicts the least-recently-used stack.
- **Lifetime** — history is in-memory only. A server restart empties every
  stack, and `undo`/`redo` then fail closed with HTTP `409` and code
  `EDIT_HISTORY_UNAVAILABLE` (with a suggestion to re-apply the change)
  instead of silently no-opping or guessing an undo target. A client
  document that is not the recorded history tip is refused with HTTP `409`
  and `EDIT_HISTORY_STALE`; `404 NO_UNDO` / `NO_REDO` mean history exists
  but that direction is exhausted.

See [DAG Edit API](dag-edit-api.md) for the full command reference.

---

## See Also

- [System Architecture](architecture.md) — Domain-driven module structure
- [Web System Architecture](web-system-architecture.md) — Router design and middleware
- [AI Translation Layer](ai-translation.md) — AI integration design
- [Diagnostics Engine](diagnostics-engine.md) — Error pattern library
- [Deployment Modes](../how-to/deploy-modes.md) — Personal/Team/HPC
