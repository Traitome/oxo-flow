# AI Agent Context & Instructions: oxo-flow

This document is the primary source of truth for AI agents working on oxo-flow.

CHANGE MANAGEMENT: any structural change (new crate, new subcommand, renamed flag, version bump, port change, provider change, domain change) requires updating this file and the corresponding docs in `docs/guide/src/`, cross-referenced against README.md. Stale docs cause cascading errors in AI-assisted development.

## Project Overview
`oxo-flow` is a high-performance bioinformatics pipeline engine built in Rust. It focuses on reproducibility, environment isolation, and AI-assisted workflow development.

## Workspace Layout
- `crates/oxo-flow-core`: DAG engine, executor, environment management, config, scheduling, reporting
- `crates/oxo-flow-ai`: AI companion — provider abstraction, skill system, agent orchestration, MCP
- `crates/oxo-flow-cli`: CLI binary (`oxo-flow`) — 39 subcommands via clap derive. Agent-facing docs are embedded: `docs/guide/src/SKILL.md` is canonical, `crates/oxo-flow-cli/assets/SKILL.md` is the build-time copy (`make skill-drift` gate, `make sync-skill` refreshes), and `oxo-flow agent-docs` / `init --agent` scaffold `AGENTS.md` + `CLAUDE.md` + `SKILL.md` into a project
- `crates/oxo-flow-web`: Axum web server + React 19 SPA, 100+ OpenAPI-documented operations across 9 domains
- `crates/oxo-flow-desktop`: Native desktop shell (wry + tao) around the embedded web server. EXCLUDED from the workspace (`exclude` in the root Cargo.toml) because tao/wry need GUI toolchains headless CI must not pull in — build it with a separate `cargo build` invocation; its version is kept in lockstep by `scripts/bump-version.sh`. Windows is source-compatible only (not built, not tested — UX-09 decision, see desktop-app.md); `make desktop-smoke` is POSIX-only
- `editors/vscode`: First-party VS Code extension (`traitome.oxo-flow`) — `.oxoflow` language (TextMate grammar), schema-generated completion/hover data, CLI-backed diagnostics/formatting, and run/dry-run/graph/resume/AI commands. Self-contained npm project (TypeScript, esbuild, **zero runtime dependencies**); its version is kept in lockstep by `scripts/bump-version.sh`. Released as a VSIX attached to every GitHub release and published to Open VSX (`OVSX_PAT`) and the VS Code Marketplace (`VSCE_PAT`) by the `publish-vsix` CI job. Fast gate: `make extension-test`; headless integration tests: `make extension-integration`. Completion/hover data is GENERATED from `docs/schema/oxoflow-v1.schema.json` by `editors/vscode/scripts/generate-completions.mjs` — after schema changes run `npm run generate` in that directory (CI drift-checks the committed output)
- `examples/`: Reference `.oxoflow` (TOML-based) pipeline files
- `tests/`: Integration tests covering CLI and core functionality

## Tech Stack & Conventions
- **Language:** Rust (Edition 2024).
- **Async:** `tokio` for concurrency.
- **Error Handling:** `thiserror` for library errors; `anyhow` for CLI/Bin.
- **State/Config:** `serde` with TOML as the primary format.
- **Logging:** Structured logging via `tracing`.
- **CLI:** `clap` with the derive API.
- **Graph Logic:** `petgraph` for DAG operations.

## Development Workflow (Mandatory)
Before concluding any task, the following suite **must** pass:
```bash
make ci
```
*Included in `make ci` (all gates are mandatory): `fmt`, `clippy` (`--workspace --all-targets -D warnings`), `build`, `test`, `schema-drift`, `skill-drift`, `version-check`, `audit`, `deny`, `aws-legacy-tripwire`, `issue-id-tripwire`, `npmmirror-tripwire`, `truthset-stamp`, `secrets`, `docs`, `docs-lint` (MD032 markdown-list gate over `docs/guide/src/**/*.md`, pinned markdownlint-cli2, rules config `docs/md032.jsonc`), `frontend-lint`, `eval-tests`, `eval-lint`, the publication-artifact provenance gate `provenance-check` and the report-generator regeneration gate `reports-regen` (both #1305), the documentation-claims drift gate `claims-check` (`scripts/check-doc-claims.py` + `docs/claims.json`: CLI/diagnostics/SCI/OpenAPI counts, TYPE/META code families, BioType truth-set N, repair N/13, and the documented backend maturity are derived from the built binary, core sources, evidence artifacts, and the generated OpenAPI spec — it runs after `build` because it executes the binary), and the extension fast gate `extension-test` (npm ci + compile + unit tests + `vsce package` for `editors/vscode`). Headless VS Code integration tests are a separate target: `make extension-integration`. Desktop shell lifecycle (launch → embedded server → single-instance refusal → clean SIGTERM exit, ledger UX-09) is a separate target too: `make desktop-smoke`, run by the CI `desktop` job; it stays out of `make ci` because the wry/tao GUI toolchain is deliberately outside the workspace gate.*

## Key Design Principles
1. **DAG-First Execution:** Everything is a graph. Validate dependencies before execution.
2. **Environment Agnostic:** Support 8 backends: conda, mamba, pixi, docker, singularity, venv, system, HPC modules.
3. **Wildcard Expansion:** Native support for `{sample}`, `{chr}` patterns with regex constraints.
   **Cohort metadata contracts (issue #1368):** the optional `[workflow.metadata_schema]` table (`id_column` default `"sample"`, `required_columns`, `allow_extra_columns` default `true`, `coverage` `off|warn|error` default `warn`, `columns[]` with `name`/`required`/`unique`/`type`/`regex`/`enum`) validates the `metadata_file` table. Findings `META-001`–`META-012` are stable and row-level (sorted by `(code,row,column,id)`). Declaring the schema is the strictness switch: without it the table is free-form and non-coverage findings are warnings (schema-dependent codes cannot fire); with it they are errors and `validate`/`run` fail. `coverage` governs `META-009`/`META-010`/`META-011` only; a declared `metadata_file` (even header-only) and rows both count as a contract. A declared `id_column` is strict; the first-column fallback is schema-less compatibility only. `validate --json` and `dry-run --json` expose findings in a `metadata` array plus `metadata_digest` (evaluate the as-authored config before `--samples` filtering; `--as-include` skips). The canonical digest (`sha256:<hex>` over the defaulted schema + rows sorted by `(id, canonical values)`/column; typed canonicalization: integer/float re-formatted, boolean lowercased, strings trimmed; empty omitted — byte-stable across TSV/CSV/JSON and row order) is stored on `WorkflowConfig::metadata_digest`, recorded in `CheckpointState::metadata_digest`, folded into `compute_workflow_resolved_hash`, and reported/bound by `provenance verify`/`attest`. Docs: `docs/guide/src/reference/workflow-format.md` (Metadata Contract), `commands/validate.md`, `commands/dry-run.md`, `commands/provenance.md`, `reference/glossary.md`.
4. **Reproducibility First:** Every execution must be reproducible and auditable via provenance.
5. **Disk-Pressure Guard:** Optional `[engine]` section (`min_free_disk` / `reclaim_free_disk`, issue #843) drives a Normal → Reclaim → Hold → abort ladder at run time; reclamation reuses the `temporary = true` tombstone + cascade-up machinery. Documented in `docs/guide/src/reference/workflow-format.md` (`[engine]` section) and `docs/guide/src/commands/run.md`. The same `[engine]` section also carries `tmp_dir` (issue #907): a global scratch directory injected as `TMPDIR`/`TMP`/`TEMP` into every rule shell and hook so tools stop spilling caches into the (often small) system temp volume; `OXO_FLOW_TMPDIR` overrides it per invocation. `[engine] strict_placeholders` (issue #1078 F2) is the execution-time placeholder gate: default `true` fails a rule before spawn when its rendered command still carries an engine placeholder (`{sample}`, `{meta.<col>}`, `{config.<key>}`, `{input…}`, `{output…}`), because the shell would receive literal brace text; `false` / `OXO_FLOW_STRICT_PLACEHOLDERS=0` restores the warning-only behavior. `singleton = true` only silences the static W041 diagnosis and opts out of meta-implied fan-out — it never softens the runtime gate. Shell `${...}` is never treated as a placeholder. A separate **plan-time** gate hard-fails `run`/`dry-run` before the DAG builds when any rule references an undefined `{config.<key>}` (E005) — that gate is not controlled by `strict_placeholders`/`OXO_FLOW_STRICT_PLACEHOLDERS`. The web run-admission path (`POST /api/runs`) enforces the SAME gate (issue #1289 / audit RB-04 residual) on the resolved config after `config_overrides` are applied, failing closed with `400 RUN_ERROR` before any run row or workdir is written. `[engine] dispatch_order` (issue #1089, PR #1090) selects the ready-rule tie-break: `"rule"` (default) keeps the historical instance-name sort (byte-identical to prior releases); `"sample"` orders ties by the instance's fan-out binding (group/sample first, then remaining bindings) and rule declaration order, keeping a sample's stages adjacent instead of running in global phases; because priority aging still applies, per-sample drain is best-effort, not guaranteed. Priority (declared + aging) stays the primary key on both local and cluster paths. Documented in `docs/guide/src/reference/workflow-format.md` and `docs/guide/src/how-to/run-on-cluster.md`.
6. **Normalized Resource Ledger (issue #1371):** `oxo-flow report --run <DIR> --resource-ledger -f json|csv` renders `oxo-flow.resource-ledger/v1` from the checkpoint — per-rule metrics plus run totals, each with `source {backend, tool, version?, note?, raw_fields}`, and optional reconciliation `conflicts` against an `--acct` sacct CSV (tolerance: 5%, floored at 1 s / 1 MiB). Unknown is never zero: unreported metrics are absent/empty, never `0`. Measurements persist in the checkpoint's `resource_usage` map (serde-default, populated by `CheckpointState::record_run`) so failed rules keep their CPU/I-O/wait/source too — local `/proc/<pid>/io` is folded on every attempt, including the successful fall-through. Additive metrics (wall, CPU, I/O, wait) sum; memory is a peak (max). Every `run` (local or cluster) draws an invocation boundary by marking all previously recorded rules in a per-rule `skipped_attempts` map beside `rule_runs` (so the last real execution record survives); terminal attempts clear the mark, so the ledger never presents preserved work — all-up-to-date re-runs, `--target`/`--from-rule` subsets, cluster no-ops/subsets, blocked rules — as this run's execution; `scheduler_totals` always carries `scheduler_source`, and `reconciliation` records what was compared. The benchmark fallback is attempt-scoped (task-7c I-1): `benchmarks` is used only when the latest `rule_runs` attempt is `success` (or the record predates status tracking), so a failed/cancelled/timed-out re-execution uses its own `resource_usage` and run timing, counts in `rules_executed`/`executed_rule_names`, and is never suppressed by a synthetic `recorded_as` marker left by an earlier invocation. `--acct` reconciliation requires `-f json`: CSV cannot carry `scheduler_totals`/`reconciliation`/`conflicts`, so the CLI fails closed (exit 1, usage error) before reading the export. Both formats carry the reserved `user_seconds`/`system_seconds`/`peak_memory_bytes` fields and `source.raw_fields` (CSV: compact key-ordered JSON cell). One shared `parse_rss_bytes`/`parse_rss_mb` parser serves every scheduler adapter and the `--acct` importer (`c` = bytes, `n` = per-node qualifier, bare = KiB, `MaxRSS=0` = unknown/not zero). A backend may only report fields its store actually returns — extending accounting means extending the parser AND its `ResourceSource.raw_fields`, never synthesizing a value. The ledger is a CLI-only surface (no web route); see `docs/guide/src/commands/report.md`, `how-to/run-on-cluster.md`, and `reference/execution-backends.md`.

## Coding Standards
- **Type Safety:** No `unsafe` unless strictly justified.
- **Documentation:** Public APIs in `oxo-flow-core` should have doc comments.
- **Testing:** New features require corresponding unit or integration tests.
- **Errors:** Return `Result` early; use context where helpful.
- **Markdown lists:** Always insert a blank line between a text paragraph and a list (`-`/`*`). A paragraph followed immediately by `-` on the next line will NOT render as a list in many Markdown engines — the `-` is shown as literal text. Do NOT insert blank lines between items within the same list; only between a preceding paragraph and the list start.

## Engineering Invariants

General rules against the classic failure mode "new feature works, but breaks something else". All of them have already been violated in shipped bugs; apply them while writing code, not in review:

- **Reuse, never retype.** If a list, whitelist, or transformation already exists somewhere, export and call it. Hand-copied logic always drifts from its origin.
- **Don't mirror semantics — share them.** Code that must match another module's behavior either calls the same function or tests every branch of the original against the source code, never against an example.
- **Collectors never short-circuit.** In loops that accumulate findings, gate each check by its own condition; a `continue` or early return silently disables every check after it.
- **Persisted artifacts are order-independent.** Anything serialized for comparison or provenance uses key-ordered structures (BTreeMap/BTreeSet) — HashMap iteration order is not stable.
- **Expand before deriving.** Resolve variable/config expansion before deriving canonical forms (templates, fingerprints, paths); reversing the order silently breaks matching.
- **A diagnostic ships with its docs.** A new warning or check without a corresponding docs entry is an incomplete change.

## Documentation System
- `docs/guide/` — MkDocs-based user guide (run `mkdocs serve` in `docs/guide/` to preview)
- `docs/guide/src/reference/web-api.md` — REST API reference (structured errors, endpoints)
- `docs/guide/src/reference/web-system-architecture.md` — Web system architecture
- `docs/guide/src/reference/architecture.md` — Overall system architecture
- `docs/guide/src/reference/ai-cli.md` — Complete AI CLI reference
- `GET /api/openapi.json` — OpenAPI 3.1 schema generated at runtime
- `docs/schema/oxoflow-v1.schema.json` — Workflow format JSON Schema (canonical source for the VS Code extension's generated completion data)
- `docs/claims.json` — machine-checkable registry for the `make claims-check` gate: each entry names a deterministic source (built CLI, core constant table, evidence JSON, generated OpenAPI spec, directory listing) and the assertions the user-facing docs must satisfy. Change the source or the docs when a claim legitimately changes; never edit a claimed number in isolation
- `docs/guide/src/reference/vscode-extension.md` — VS Code extension reference (features, settings, publishing)

## Web System (AI-Native API)
The web crate (`oxo-flow-web`) is designed as an AI-native API surface:

- **Structured errors**: All responses use `{code, message, detail, suggestion}` format
- **API discovery**: `GET /api/openapi.json` returns full OpenAPI 3.1 schema
- **Intent-driven authoring**: `POST /api/ai/translate` maps a natural-language intent to a validated `.oxoflow` pipeline (SSE variant: `POST /api/ai/translate/stream`)
- **SSE streaming**: `GET /api/events` provides real-time execution events
- **Run staging parity with the CLI (CLI→web milestone)**: `POST /api/runs` accepts `include_files` (workflow-relative [[include]] sources staged beside `workflow.oxoflow`; keys are canonicalized before validation and staging — no absolute paths, no `..`, no duplicate canonical or case-insensitive canonical keys, and no canonical `workflow.oxoflow` key, else `400 INVALID_INCLUDE_PATH`), `config_overrides` (`KEY=VALUE` strings passed to the spawned CLI as `--arg`; shape-gated `400 INVALID_CONFIG_OVERRIDE`, and — since issue #1277/RB-04 — resolved and applied to the admission graph itself through the same `oxo-flow-core::config` helpers the CLI preflight uses, so the override decides the resolved DAG the canonical checker sees, not only the spawned CLI's run), `allow_protected_drift` (issue #935 — forwards `--allow-protected-drift` so a run may proceed past the protected-tree gate; retry keeps the gate), and `provenance` (forwards `--provenance` so the run records `sha256` output checksums into its checkpoint; retry does not re-apply it). Direct typed workflows are checked by the same core BioType validator as CLI preflight: the web admission layer resolves the full include graph by staging the workflow and all `include_files` sources, re-parsing through the canonical `from_file` path, applying `config_overrides`, and running `validate_workflow_types` on the merged config (issue #1018/#1152 — the old text-scan `TYPED_ADMISSION_UNSUPPORTED` gate is gone; issue #1277/RB-04 applied the overrides to the admission graph so CLI and web evaluate the identical resolved workflow). Unresolvable includes surface as `400 {"code":"RUN_ERROR"}` from the canonical parser, not a blanket typed-admission rejection. Unrelated staged assets do not participate in that check. Cluster runs carry overrides and both flags into the remote wrapper. `GET /api/runs/{id}/protected-drift` previews `[engine] protected_data_trees` drift against the run's checkpoint baseline (read-only, dry-run semantics — baselines are not adopted). `GET /api/runs/{id}/provenance-verify` audits the checkpoint's recorded output checksums by shelling out to the CLI's `provenance verify --json` (read-only; output-checksum drift or a workflow file that no longer matches its recorded content digest — issue #1350's `workflow_content_check` — is a 200 finding with `ok: false`, not an HTTP error; `400 INVALID_CHECKPOINT` on a malformed checkpoint). Portable run attestations are a CLI surface: `oxo-flow provenance attest <checkpoint> [--sign --key <seed>]` emits the canonical `oxo-flow.attestation/v1` document binding the recorded workflow identity/includes/config/input+output checksums/environment identity/outcome, and `provenance attest-verify <file> [--key]` exits 0 valid-or-unsigned / 2 invalid signature / 3 key mismatch / 4 malformed (no PKI or legal qualification is claimed). See `docs/guide/src/reference/web-api.md` (Create Run, Run protected-drift preview, Run provenance-verify) and `docs/guide/src/commands/provenance.md` (Portable run attestations)
- **Run workdir immutability & admission (issue #1130 / audit RB-02)**: every create-run — ad-hoc or from a saved pipeline — executes in its own `workspace/users/<u>/runs/<run_id>/` workdir; the workflow snapshot, staged includes/inputs, checkpoint, logs, and outputs are never shared with another run, and the legacy `pipelines/<pipeline_id>/` run workdir is no longer created. `pipeline_id` is an association only (permissions/listing). Admission is one guarded `INSERT ... SELECT ... WHERE NOT EXISTS ... ON CONFLICT DO NOTHING` (shared SQL builder in `infra/db/mod.rs`) backed by partial unique indexes `idx_runs_live_pipeline` / `idx_runs_live_workdir` on both SQLite and PostgreSQL — the index is what makes the claim atomic under PostgreSQL READ COMMITTED, where two transactions can both pass `NOT EXISTS`; zero affected rows maps to `409 RUN_NOT_TERMINAL`. Saved-pipeline runs serialize on `pipeline_id` globally (one live run per saved pipeline across all readers, so a `workspace`-shared pipeline 409s another identity), ad-hoc/retry/resume serialize on `workdir`. `claim_run_row` is the single helper used by create/retry/resume. PostgreSQL atomicity is proven by the live smoke concurrency tests (`test_concurrent_pipeline_admission_admits_exactly_one`, `test_concurrent_workdir_admission_admits_exactly_one`) under `OXO_PG_TEST_URL`, and backend-EQUIVALENT admission verdicts are additionally asserted by the differential harness `tests/admission_backend_parity.rs` (issue #1312): the SAME scenarios (workdir-serialized claim, pipeline-serialized claim, concurrent double-claim admitting exactly one, terminal predecessor releasing the guard, zero-affected-rows → `Ok(false)` — the sole input the retry/resume arms map to `409 RUN_NOT_TERMINAL`) run through the same `claim_run_row` against BOTH backends and must equal one canonical contract; the SQLite arm runs unconditionally, the PostgreSQL arm gates on `OXO_PG_TEST_URL`. The harness proves backend-equal verdicts in a single process on a single host — multi-instance deployment, pool exhaustion, network partitions, and crash recovery remain unproven. Retry, resume-checkpoint, and apply-repair deliberately continue a predecessor run's workdir (checkpoint-driven partial re-execution / repair) and are the only such paths. Create/stage/spawn is fail-closed (RB-03, #1131): every pre-spawn staging step (workflow, includes, resolved snapshot, uploaded inputs) is fatal and compensates — release the spawn reservation, delete the run row (`delete_run` on both backends), remove the workdir — so no failed create leaves a ghost `queued` row, a pinned quota reservation, or an orphan directory (fault-injection test `pre_spawn_fault_no_spawn.rs`). PostgreSQL crash-recovery/multi-instance admission remains unproven
- **Cross-surface remediation continuity (issue #1302, ROADMAP 12.3)**: a reviewed repair patch — a whole `.oxoflow` workflow in the exact shape the CLI's `apply_fix` writes — applies identically on every surface. The CLI seam is `run --apply-patch <FILE>` (#1308, shared `apply_and_verify_repair` path, fixed session label `reviewed-patch`, `REPAIR_APPLIED`/`REPAIR_RERUN_*` narration, checkpoint `repair_record`). The web surface is `POST /api/runs/{id}/apply-repair` on a terminal FAILED run: the route pre-flights the patch through the shared core parser (`400 INVALID_PATCH`), stages it into the run workdir (`.oxo-flow/reviewed-repair-<run_id>.oxoflow`), and shells the SAME CLI invocation (`run <workflow> --workdir <workdir> --apply-patch <patch>`) through the web executor's spawn machinery with retry-identical budgets (rate limiter, durable quota admission, atomic claim, REQUIRED audit row); the web layer never writes the workflow itself — the CLI apply path (validate → archive → atomic write) is the only workflow mutator. `GET /api/runs/{id}/repair` reads the checkpoint `repair_record` through the engine's canonical loader. The VS Code surface is `oxo-flow.applyRepair` (checkpoint picker → patch picker → the same argv as a task). The cross-surface parity harness `tests/remediation_continuity.rs` feeds ONE reviewed patch through the CLI, the web route, and the extension's exact argv, asserting byte-identical applied workflow + identical `REPAIR_*` outcome (the extension leg is the CLI path under the extension's argv — the extension shells the CLI by construction; its argv shape is pinned by `editors/vscode` unit tests). See `docs/guide/src/reference/web-api.md` (Apply Reviewed Repair, Repair Record), `docs/guide/src/reference/vscode-extension.md` (Apply Reviewed Repair), and `docs/guide/src/commands/run.md` (Reviewed-patch repair)
- **Pagination**: `GET /api/runs` uses cursor pagination `{items, next_cursor, total}`; most other list endpoints return bare capped arrays (≤ 100); the storage layer exposes an offset-style `Paginated<T> {items, page, per_page, total_items, total_pages}` internally. There is deliberately no single uniform envelope — document per endpoint (see web-api.md)
- **Decoupled Persistence & Unified StorageBackend (issue #207 code path landed, live CRUD verified)**: run execution and its lifecycle bookkeeping route through the unified `StorageBackend` trait, with SQLite and PostgreSQL implementations. CI runs a live-PostgreSQL smoke test (`services: postgres`) pinning schema bootstrap + core `StorageBackend` CRUD (users, runs, run nodes, audit logs, webhook config, chat sessions) against a real server; dated historical evidence on 2026-11-05 additionally showed a live `postgres:16` `postgres_storage_contract` round-trip and a web-suite result of 554 passed / 0 failed with the `postgres` feature — those exact totals are historical, not current CI evidence. Concurrency, crash-recovery, and multi-instance behavior remain unproven — do not claim production PostgreSQL parity without them. If no backend is initialized, requests answer `503 {code:"RUNS_REQUIRE_STORAGE_BACKEND"}` via the backend-agnostic router-layer gate (`require_storage_backend_for_runs` in server.rs; any initialized backend passes).
- **Write-ahead audit trail (issue #1311 / WEB-15 D1)**: every state-changing request commits a durable `pending` audit intent row BEFORE its handler runs (`StorageBackend::begin_audit_intent`, one transaction with the synthetic `users` seed, both backends), and the same row is finalized to `success`/`failure` after the handler (`finalize_audit_intent`). A crash between the mutation and its outcome write leaves a durable `pending` row — no committed state change can lack its audit row; a `pending` row read later means exactly that (request died mid-flight, mutation may or may not have committed) and no sweeper rewrites it. If the write-ahead insert cannot be persisted, the request degrades to the legacy post-handler write. The REQUIRED fail-closed `500 AUDIT_WRITE_FAILED` (non-5xx only), the already-5xx preservation boundary (WEB-17 `503 RUNS_REQUIRE_STORAGE_BACKEND`), and the handler-owned successful-login row (session-first ordering, documented sign-in hint) are unchanged. Audit `metadata` JSON columns are key-ordered at serialization (`serde_json` BTreeMap, no `preserve_order`). See `docs/guide/src/reference/web-api.md` ("Write-ahead ordering") and `tests/audit_tx.rs`.
- **Hash-chained audit records (issue #1372)**: every audit write also appends an immutable event to `audit_chain` + the one-row `audit_chain_head` (genesis seq 0, 64 hex zeros) in the SAME transaction as its `audit_logs` write, serialized by SQLite `BEGIN IMMEDIATE` / PostgreSQL `SELECT … FOR UPDATE` on the head. That serialization is by construction; the measured PostgreSQL evidence is single-host/single-instance PostgreSQL 16.15, and multi-instance deployment remains unmeasured. A structurally broken chain (missing/deleted head row) answers `500 DB_ERROR` fail-closed, not a 200 finding; the offline verifier treats a headless export as malformed (exit 4). Each event hashes its fixed canonical field set (`oxo_flow_core::audit_chain`, sorted-key JSON + format tag, canonical metadata) and names its predecessor's hash; `(audit_id, phase)` is unique, so a retried finalize is idempotent and the first terminal outcome wins (a conflicting later outcome appends nothing — documented). `GET /api/audit/chain/export` (admin-only) returns the deterministic `oxo-flow.audit-chain/v1` document (head + seq-ordered events + full `audit_logs` snapshot); `GET /api/audit/chain/verify` (admin-only) returns 200 `{ok, events, head, findings}` — tampering is a finding with `ok:false`, never an HTTP error. `oxo-flow audit verify <export.json>` re-checks an export offline (exit 0 valid, 2 invalid/tampered, 4 malformed/`AUDIT_EXPORT_INVALID`) via the shared core verifier. Coverage: deletion, reorder, insertion, mutation, duplicate phase, bad genesis, truncation, and audit-row-vs-chain mismatch. LIMITS: not a signature and not external notarization — an actor who rewrites the whole chain + head consistently passes; a previously held export is what exposes a wholesale rewrite; pre-chain `audit_logs` rows are not covered; user deletion cascades log rows and surfaces as `MISSING_LOG_ROW`. Docs: `web-api.md` ("Audit chain export & verification"), `web-system-architecture.md` ("Hash-chained audit trail"), `security.md` (Layer 6), `deploy-modes.md` (backup/restore caveat), `docs/guide/src/commands/audit.md`. Tests: `crates/oxo-flow-core/src/audit_chain.rs` + `audit_chain/tests.rs` (unit/tamper matrix), `crates/oxo-flow-web/tests/audit_chain.rs` (SQLite concurrency/idempotence + gated PostgreSQL arm), `crates/oxo-flow-web/tests/audit_chain_api.rs` (endpoints + tampering).

## Frontend SPA
The frontend is a React/TypeScript SPA built with Vite, located in `frontend/`:

```bash
# Development mode (two terminals):
cd frontend && npm run dev    # Vite dev server on :5173, proxies /api to :3000
cargo run -p oxo-flow-web      # API server on :3000 (default; OXO_FLOW_PORT)

# Build for production:
cd frontend && npm run build   # Outputs to crates/oxo-flow-web/static/
```

### Web Module Structure

The web crate uses a domain-driven modular monolith with 9 domains:

- `domains/workflow/` — Pipeline parsing, validation, DAG building, formatting
- `domains/execution/` — Run lifecycle, diagnostics engine (31 patterns), retry logic
- `domains/ai/` — AI copilot prompts, service orchestration, handlers, agents
- `domains/auth/` — Authentication, OAuth stubs, license endpoints
- `domains/collaboration/` — Pipeline fork, share, import
- `domains/observability/` — Health checks, system info, metrics
- `domains/dag/` — DAG-specific types and operations
- `domains/chat/` — Real-time chat and SSE streaming
- `domains/clusters/` — Cluster scheduler submission and monitoring

Each domain follows: `types.rs` (data) → `service.rs` (pure logic) → `handlers.rs` (HTTP adapters).

## Docker Deployment

### Quick Start
```bash
# Build and start (single command)
docker compose up -d

# Or build manually
docker build -t oxo-flow .
docker run -d -p 3000:3000 -v oxo-flow-data:/app/data oxo-flow
```

### Environment Variables

| Variable | Required | Default | Description |
|----------|----------|---------|-------------|
| `OXO_FLOW_MODE` | No | `personal` | Serve mode for `oxo-flow serve`: `personal` (loopback, no auth), `team`, or `hpc` — same as `--mode` |
| `OXO_FLOW_HOST` | No | `127.0.0.1` | Bind host for `oxo-flow serve` (same as `--host`) |
| `OXO_FLOW_BASE_PATH` | No | `/` | Sub-path the app is mounted under (same as `--base-path`) |
| `OXO_FLOW_ALLOWED_ORIGINS` | No | localhost origins | Comma-separated CORS allowlist for the web server; empty = only `localhost:3000`/`:5173` and `127.0.0.1:3000`/`:5173`. The same origin list is the server-side allowlist for OAuth redirect targets (issue #1141) |
| `OXO_FLOW_AI_PROVIDER` | No | auto-detect | `"claude"`, `"openai"`, `"deepseek"`, `"ollama"`, or `"disabled"`. Unset = auto-detect from standard provider credentials (`ANTHROPIC_AUTH_TOKEN`, `OPENAI_API_KEY`, `DEEPSEEK_API_KEY`, `OLLAMA_HOST`, …); `disabled` is only the outcome when nothing resolves, and an explicit `disabled` always wins — it is pinned after the config-file, platform-config, and DB tiers load, so none of them can resurrect a disabled registry. In that state `GET /api/health` reports `components.ai_provider.status = "disabled"`, the runtime-only `POST /api/ai/config` answers `409 AI_DISABLED_BY_ENV`, the persisted `PUT /api/ai/config/server|user` still save (200, `applied_to_runtime: false`, never reconfigure the pinned runtime), and `POST /api/ai/test` answers `success: false` / `provider: "disabled"` without dialing out |
| `OXO_FLOW_AI_API_KEY` | No | — | Generic API key fallback |
| `OXO_FLOW_AI_API_URL` | No | (provider default) | Custom API endpoint URL |
| `OXO_FLOW_AI_MODEL` | No | (provider default) | Model name override |
| `OXO_FLOW_AI_CONFIG` | No | `$HOME/.oxo-flow/ai_config.json` | Explicit AI config file path, used for BOTH reads and writes (`ai_config_path()`). When set it is authoritative: the legacy `$HOME/.config/oxo-flow/ai_config.json` migration fallback is skipped. Empty = unset; a relative path anchors to the process cwd; parent directories are created on save. Hermetic knob for tests/CI — an in-process web test without it overwrote the operator's real config (test isolation incident). Test harnesses set it in `crates/oxo-flow-web/tests/common/mod.rs` and `tests/common/mod.rs` |
| `OXO_FLOW_AI_MAX_TOKENS` | No | `16384` (Claude) / unset (OpenAI-compatible) | Output-token ceiling. The Claude/Anthropic backend ALWAYS sends it (default 16384), calibrated for thinking-style backends whose reasoning blocks count against `max_tokens`. On OpenAI-compatible requests it is sent as `max_tokens` only when the variable is explicitly set (unset = omit the field and let the endpoint's default apply); invalid values warn once and stay omitted. The field is deliberately `max_tokens` — endpoints demanding `max_completion_tokens` are not auto-mapped. Ollama ignores it |
| `OXO_FLOW_AI_CONTEXT_TOKENS` | No | unset | Declared context window for third-party proxies fronting a smaller model (e.g. `131072`). The shared estimator/clamp (1.5× safety factor + 4096-token margin, floored at 1024) subtracts estimated input from the window before the output budget is sent: always on the Claude backend; on OpenAI-compatible requests only when `OXO_FLOW_AI_MAX_TOKENS` is explicitly set. Unset = no clamping. Ollama ignores it |
| `OXO_FLOW_AI_FALLBACK` | No | `auto` | Web `POST /api/ai/translate` provider-fallback policy. `auto` keeps the historical chain (acting user's provider → configured runtime → env-discovered Claude/OpenAI/Ollama) and logs a WARN on every fallback; `disabled` limits the chain to the first usable provider (acting user's, else runtime), so an explicit provider pin cannot spend a different credential. Unrecognized values warn and keep `auto`. In both modes a billed attempt (max rounds, empty/invalid response, tool error) is terminal — the next provider is NOT tried. The CLI and web chat have no cross-provider fallback chain — this knob affects translate only |
| `OXO_FLOW_AI_TIMEOUT_SECS` | No | `300` | Per-request timeout for all AI provider backends. A measured GLM thinking round took 145 s — the old 120 s default killed it mid-round. Raise further for slower endpoints, together with OXO_FLOW_AI_MAX_TOKENS. Also the TOTAL budget for the non-streaming web AI JSON routes (`POST /api/chat/send/json`, `POST /api/ai/translate`): the route budget fires one second under the provider timeout and a timeout-class provider error is classified into the same `504 AI_TIMEOUT`, so the documented answer holds whichever timer expires first |
| `OXO_FLOW_MASTER_KEY` | No | — | AES-256-GCM seed encrypting AI provider keys at rest (`v1:`-prefixed rows in the local DB). Unset = plaintext legacy mode |
| `ANTHROPIC_AUTH_TOKEN` | No | — | Claude/Anthropic API key (overrides OXO_FLOW_AI_API_KEY) |
| `ANTHROPIC_BASE_URL` | No | `https://api.anthropic.com/v1/messages` (endpoint; base `https://api.anthropic.com`) | Anthropic-compatible API endpoint URL |
| `ANTHROPIC_MODEL` | No | `claude-sonnet-4-20250514` | Claude model name |
| `OPENAI_API_KEY` | No | — | OpenAI-compatible API key (overrides OXO_FLOW_AI_API_KEY) |
| `OPENAI_BASE_URL` | No | `https://api.openai.com/v1/chat/completions` (endpoint; base `https://api.openai.com/v1`) | OpenAI-compatible API endpoint URL |
| `OPENAI_MODEL` | No | `gpt-4o` | OpenAI-compatible model name |
| `OLLAMA_HOST` | No | — | Local Ollama endpoint (enables Ollama auto-detection) |
| `OLLAMA_MODEL` | No | (provider default) | Ollama model name (rides the canonical model tier under an explicit `model` override; #948 P2-3) |
| `OXO_FLOW_FRONTEND_DIR` | No | — | Path to built frontend dist directory. The web server also honors a bare `FRONTEND_DIR` as a legacy fallback (checked only when this variable is unset) |
| `NO_COLOR` | No | unset | Standard no-color convention: when set, CLI output drops ANSI colors (same effect as the global `--no-color` flag); command-level `--no-color` flags win |
| `RUST_LOG` | No | unset | Standard `tracing` env filter for CLI/web logs (e.g. `debug`, `oxo_flow=trace`); unset uses the default level driven by `-v`/`--quiet` |
| `DATABASE_URL` | No | `sqlite://oxo-flow.db` | Database selection shared by `oxo-flow serve`, `oxo-flow-web`, desktop, and `oxo-flow runs gc`: a `postgres://`/`postgresql://` URL selects the PostgreSQL backend (requires the `postgres` compile feature; the published Docker image omits it), anything else stays SQLite. Web domain handlers route through the unified `StorageBackend`; a CI smoke test (`services: postgres`) verifies schema bootstrap + core `StorageBackend` CRUD on a live server, and the storage contract was live-verified against `postgres:16` (2026-11-05), single-host concurrency/crash-recovery/instance-sharing measurements exist (`docs/superpowers/audits/postgres-ledger-2026-11-19.md`, ROADMAP 12.1 "Implemented and verified"), but production scale, pool exhaustion, network partitions, and true multi-instance deployment remain unmeasured. Run endpoints gate on backend-agnostic `RUNS_REQUIRE_STORAGE_BACKEND` — absence of production parity must not be read from the gate |
| `OXO_FLOW_PORT` | No | `8080` | Web server port (also settable via `--port`). The standalone `oxo-flow-web` binary defaults to `3000` |
| `OXO_FLOW_ADMIN_PASSWORD` | Team/HPC | — | Admin sign-in password. Without at least one of ADMIN/USER/VIEWER_PASSWORD in team/hpc mode the server still starts but every login is rejected until one is set (warned at startup) |
| `OXO_FLOW_USER_PASSWORD` | Team/HPC | — | Standard-user sign-in password |
| `OXO_FLOW_VIEWER_PASSWORD` | Team/HPC | — | Read-only sign-in password |
| `OXO_FLOW_DEV_MODE` | No | unset | `"1"` accepts `password == username` logins for any user (local development only). In team/hpc modes the server refuses to start on a non-loopback bind with it set; personal mode already force-binds loopback (it warns and ignores the requested non-loopback host), so it never starts network-exposed with dev mode either |
| `OXO_FLOW_DISABLE_RATE_LIMIT` | No | unset | `"1"` disables every rate limiter — the login/API limiter and the dedicated run-creation limiter (#213): "off means off" on both paths (tests only) |
| `OXO_FLOW_OAUTH_REDIRECT_URI` | No | `http://localhost:3000/api/auth/oauth/callback` | Redirect URI registered with the OAuth provider (ORCID); set when running behind a proxy or non-default port. Its origin is always on the OAuth redirect allowlist (issue #1141), and the issued `state` is bound to the exact URI |
| `OXO_FLOW_PULL_MAX_BYTES` | No | `1073741824` (1 GiB) | Upper bound for `pull`/bundle downloads (buffered before checksum verify); raise for larger bundles |
| `OXO_FLOW_OUTPUT_TAIL_BYTES` | No | `65536` (64 KiB) | Per-rule stdout/stderr tail persisted in checkpoint.json (`stdout_tail`/`stderr_tail`); the in-RAM capture window widens to match when set above its 1 MiB floor. Invalid/zero values fall back to the default with a warning |
| `OXO_FLOW_HOOK_TIMEOUT_SECS` | No | `300` | Per-hook timeout for `pre_exec`, rule-level, and workflow-level hooks (issue #1015 H-3). Hook output is captured through the same capped collector as rule shells; a `pre_exec` hook past the deadline aborts the rule, best-effort hooks warn. Invalid or `0` values fall back to the default so a typo can never disable the abort |
| `OXO_FLOW_TMPDIR` | No | — | Per-invocation override for `[engine] tmp_dir` (#907): injects `TMPDIR`/`TMP`/`TEMP` pointing at this directory into every rule shell and hook (rule-level + workflow-level), and exports the same variables into every cluster job script (issue #1013 C2). Value used verbatim — must be absolute (no `~/` or `..` expansion); a relative value anchors to the process cwd, unlike the config key which anchors to the workflow directory. Created on demand at run start (nodes see the shared-filesystem path; submit host performs the mkdir) |
| `OXO_FLOW_ENV_PASSTHROUGH` | No | unset | Comma-separated list of ambient environment variable names re-admitted into rule shells and hooks (issue #1108, RB-15). Rule/pre_exec/hook shells run under `env_clear()` plus a fixed allowlist (`PATH`, `HOME`, `LANG`, `LC_*`, declared `envvars`, and the `TMPDIR`/`TMP`/`TEMP` triple), so an env-less rule can no longer read the engine's `ANTHROPIC_AUTH_TOKEN`/cloud tokens from its own environment. Names not listed stay hidden; whitespace around each name is trimmed and empty entries are ignored. An environment variable (not an `[engine]` key) because the need is host/invocation-specific, mirroring `OXO_FLOW_TMPDIR`. The same allowlist isolates the HPC job scripts `oxo-flow cluster submit` generates for SLURM/PBS/SGE/LSF (they wrap the rule command and the array dispatch line in `env -i ...`, since schedulers export the submit host's environment by default), the reference-build shell and `batch` sugar shells (#1233), and the web spawn path forwards both this variable and the named values so web-spawned runs reproduce the CLI-direct contract (#1241, review). Local docker/singularity rules additionally re-admit the exact container-daemon names in `CONTAINER_DAEMON_ENV_VARS` (issue #1173, RB-15 residual: `DOCKER_HOST`, `DOCKER_CONFIG`, `DOCKER_CERT_PATH`, `DOCKER_TLS_VERIFY`, `DOCKER_BUILDKIT`, and the `APPTAINER_*`/`SINGULARITY_*` cachedir/bind/tmpdir/localcachedir pairs — exact names, never a prefix; declared `envvars` still win); remote cloud dispatch (k8s/aws/gcp) never forwards the submit host's environment into a job body — the job sees only the image's own `ENV`, declared `envvars`, and the routed `OXO_FLOW_SENSITIVE_<n>` secret references (boundary pinned per backend by tests, issue #1171); a future change that adds env forwarding there must route it through this same allowlist |
| `OXO_FLOW_STRICT_PLACEHOLDERS` | No | unset (gate on) | Per-invocation override for the execution-time placeholder gate (issue #1078 F2): `0`/`false`/`no`/`off` restores the historical warn-and-execute behavior for workflows that still contain unbound engine placeholders; `1`/`true`/`yes`/`on` forces the gate on. An unparseable value warns and falls through to `[engine] strict_placeholders` (default on) — a typo never silently disables the gate |
| `OXO_FLOW_DISK_TICK_SECS` | No | `60` | Seconds between disk-pressure measurements while a run parks under Hold (#843). Invalid or `0` values fall back to the default silently |
| `OXO_FLOW_DISK_HOLD_ROUNDS` | No | `2` | Grace park rounds under Hold before the run aborts (#843); the abort message lists the exact count. Invalid or `0` values fall back to the default so a typo can never disable the abort |
| `OXO_FLOW_WEB_RLIMIT_CPU_SECS` | No | unset (= do not install) | Operator-wide `RLIMIT_CPU` ceiling, in CPU-seconds, installed by the web spawn path on the spawned CLI and its whole process tree (#1129, RB-01a). Unset **and** `0` mean "do not install"; an unparseable value is warned and treated as unset. A process past the ceiling takes `SIGXCPU` (then `SIGKILL`), so a runaway rule fails observably instead of pinning a server core. Linux-only — a no-op off Linux |
| `OXO_FLOW_WEB_RLIMIT_NPROC` | No | unset (= do not install) | Operator-wide `RLIMIT_NPROC` process cap, installed alongside `OXO_FLOW_WEB_RLIMIT_CPU_SECS` by the web spawn path (#1129, RB-01a). Unset **and** `0` mean "do not install". Counts every process owned by the server's **real uid**, not just this run's tree — set it well above the server's steady-state process count (fork-bomb backstop, not a per-run quota). Linux-only — a no-op off Linux |
| `OXO_FLOW_WEB_SANDBOX` | No | unset (= sandbox off) | Per-run OS filesystem/network sandbox for web-spawned runs (#1129, RB-01): `landlock` installs a kernel Landlock ruleset (Linux 5.13+) in a `pre_exec` hook so the CLI and every rule shell it spawns can only read+execute system dirs and the engine binary, and write the run workdir, the run scratch dir, and the operator's engine dirs — `/proc`, `/sys`, `/etc`, the server `$HOME`, and other tenants' workdirs are denied at the kernel level; TCP bind/connect is additionally denied on kernel 6.7+ (ABI v4), abstract-UNIX-socket and signal scoping on 6.10+/6.12+ (ABI v5/v6). **Fail-closed**: any configuration or kernel error refuses the spawn and marks the run failed — a typo'd value never silently disables the gate. Off Linux the requested mode is an error. See the web API reference for the enforced-vs-residual matrix |
| `OXO_FLOW_SANDBOX_READ_PATHS` | No | — | Comma-separated absolute read-only trees granted to sandboxed runs (reference data, tool installs); the grant is read + directory-listing + execute, never write. Relative entries are rejected (fail-closed). Only read when `OXO_FLOW_WEB_SANDBOX` is enabled |
| `OXO_FLOW_SANDBOX_WRITE_PATHS` | No | — | Comma-separated absolute read-write trees granted to sandboxed runs on top of the automatic write set (run workdir, `OXO_FLOW_TMPDIR`, `OXO_FLOW_MODULE_CACHE`, `OXO_FLOW_ENV_STORE`, `$HOME/.oxo-flow/locks`). CAUTION: a granted write tree is shared by ALL runs of the server account — grant data/tool trees, never another tenant's workspace. Relative entries are rejected (fail-closed) |
| `OXO_FLOW_WEB_SANDBOX_NET` | No | `deny` (when the sandbox is on) | Network posture for sandboxed runs: `deny` = TCP bind/connect denied where the kernel supports it (best-effort — a warning is logged on ABI < 4 kernels), `require` = fail-closed when the kernel cannot deny TCP (operators who must never run network-capable tenants), `off` = no network restriction. Unrecognized values fail the spawn (fail-closed). Linux-only; UDP/ICMP are not covered by the Landlock ABI on 6.x kernels (documented residual) |
| `OXO_FLOW_ENV_STORE` | No | unset (store disabled) | **Opt-in** shared pixi/venv environment store (#904): setting a non-empty path activates it (location IS activation) — each env materializes once per content key under that root, workflow trees hold symlinks; unset = self-contained in-tree envs exactly like pre-#904. GC via `oxo-flow env gc` (checkpoint-ref + age gated, `--dry-run` first); broken links auto-rebuild on the next run. Per-process variable: on multi-user servers each user who opts in gets their own store unless an operator points everyone at a shared path |
| `OXO_FLOW_AI_FETCH_ALLOW` | No | — | Comma-separated hostnames/IP literals exempted from the AI fetch/MCP SSRF guard (#204, #518); use only for trusted internal endpoints. Governs reachability only — data release is separate (`OXO_FLOW_AI_FETCH_TRUSTED_ORIGINS`) |
| `OXO_FLOW_AI_FETCH_TRUSTED_ORIGINS` | No | unset (no origin pre-authorized) | Comma-separated exact origins whose model-initiated `fetch_url` calls are pre-authorized for data release (#1123, audit AI-05), e.g. `https://docs.example.com,http://127.0.0.1:9000`. `scheme://host[:port]`, default ports omitted, paths ignored, no wildcards/suffix aliasing; unparseable entries are warned and ignored. Unset/empty = every model-initiated fetch needs an explicit human approval, and non-interactive sessions refuse. Distinct from `OXO_FLOW_AI_FETCH_ALLOW` (SSRF reachability) |
| `OXO_FLOW_AI_SESSION_DIR` | No | `.oxo-flow/ai_sessions` (cwd-local, else `$HOME/.oxo-flow/ai_sessions`) | Redirects the AI session archive (issue #1040). Writes are temp-file + atomic rename; a failed/aborted run persists `outcome: "Failed"` with its reason, and retention never prunes archives younger than 10 minutes. Set it for test harnesses and shared/read-only project trees |
| `OXO_FLOW_MCP_TOKEN` | No | — | Generic bearer token (#948 P3-7) sent as `Authorization: Bearer …` only to MCP origins listed in `OXO_FLOW_MCP_TRUSTED_ORIGINS` (#1123); withheld entirely from every other endpoint; read from the env, never from skill manifests; never logged (not even a prefix) |
| `OXO_FLOW_MCP_TOKEN_<SERVER>` | No | — | Per-server bearer token override (server name uppercased, non-alphanumeric → `_`, e.g. `fs-gdrive` → `OXO_FLOW_MCP_TOKEN_FS_GDRIVE`); wins over `OXO_FLOW_MCP_TOKEN` and is destination-scoped by construction, so it needs no trust-list entry |
| `OXO_FLOW_MCP_TRUSTED_ORIGINS` | No | unset (generic token never released) | Comma-separated exact origins approved to receive the generic `OXO_FLOW_MCP_TOKEN` (#1123), e.g. `https://mcp.example.com,http://127.0.0.1:9000`. `scheme://host[:port]`, no wildcards/suffixes; the `mcp://` spelling is accepted as its `http://` transport origin; unparseable entries are warned and ignored. Governs credential release only — MCP reachability still needs `OXO_FLOW_AI_FETCH_ALLOW` |
| `OXO_FLOW_UNSAFE_WILDCARDS` | No | unset | Set to `"1"` to relax the wildcard safe-default charset check (#203); command-substitution values stay blocked and a warning is logged once |
| `OXO_FLOW_RUNS_RATE_LIMIT` | No | `5` | Run-creation allowance per identity per minute on POST /api/runs (#213); `0` disables the dedicated limiter |
| `OXO_FLOW_TRUSTED_PROXY` | No | unset | `"1"` makes the rate limiter trust the client-supplied `X-Forwarded-For` / `X-Real-IP` headers when keying clients (the audit trail records no IP at all). Leave unset unless the server sits behind a proxy that overwrites them — otherwise a client can spoof its identity and bypass the login limiter |
| `OXO_FLOW_PUBLIC_URL` | No | unset | Operator-declared public origin for minted share URLs (#907 R16): the web server has no native TLS, so by default share URLs use `oxo+http://` with the local bind host/port (IPv6 literals bracketed); set e.g. `https://flows.example.com` (scheme recommended — a scheme-less value is coerced to `oxo+http://` with a warning; an empty value warns and is treated as unset; a value with no host — including port-only `:8080` — is ignored with a warning and a local URL is minted instead) to emit `oxo+https://flows.example.com/share/<token>` for reverse-proxy TLS deployments — the declared authority replaces the local host/port guess, and if the value carries its own mount path (`https://flows.example.com/oxo-flow`) it replaces (not doubles) the local `--base-path` prefix. IPv6 literals are bracketed on the override path too. Import accepts both schemes |
| `OXO_FLOW_SESSION_TTL_HOURS` | No | `24` | Session lifetime in hours for every minted session (login + OAuth callback), issue #976. Min `1`, cap `8760` (one year); unparseable or out-of-range values warn and fall back to the default. Read at session creation only — existing sessions keep their minted expiry |
| `ORCID_CLIENT_ID` / `ORCID_CLIENT_SECRET` | Team/HPC | — | ORCID OAuth app credentials enabling `ORCID` sign-in; both must be set (paired with `OXO_FLOW_OAUTH_REDIRECT_URI`) |
| `ORCID_SANDBOX` | No | unset | Set (any value, even empty) to point ORCID sign-in at `sandbox.orcid.org` instead of production — for testing the OAuth flow against ORCID's sandbox memberships |
| `GITHUB_CLIENT_ID` / `GITHUB_CLIENT_SECRET` | Team/HPC | — | GitHub OAuth app credentials enabling `GitHub` sign-in; both must be set, else the callback rejects the provider (`GITHUB_CLIENT_ID not set`). Paired with `OXO_FLOW_OAUTH_REDIRECT_URI` |
| `OXO_FLOW_BIN` | No | — | Explicit path to the `oxo-flow` CLI binary used by the web server/desktop to spawn runs (first of the binary search steps, before the cargo-target lookup and `PATH`); a deliberate pin operators can distinguish from a stale `PATH` hit via the startup engine probe |
| `OXO_FLOW_WORKSPACE` | No | `workspace` (relative to process cwd) | Root directory for all web-server user data: `users/<u>/inputs/`, `users/<u>/runs/<run_id>/` (each run's immutable workdir; the pre-#1130 `users/<u>/pipelines/<pipeline_id>/` tree is no longer written). Usernames/run ids are traversal-validated; point this at a persistent volume in container deployments |
| `OXO_FLOW_MODULE_CACHE` | No | `~/.cache/oxo-flow/modules` | Cache root for git-pinned `[[include]]` module checkouts (`<repo>@<ref>@<hash>` layout); override for read-only `$HOME` or shared caches. See the `[[include]]` section in the workflow-format reference |
| `OXO_REGISTRY_MIRRORS` | No | — | Container-registry mirror map for singularity/apptainer/docker image pulls: semicolon-separated `host=mirror` pairs (e.g. `docker.io=mirror.gcr.io;quay.io=quay-mirror.example.com`); case-insensitive, rewrites registry-qualified specs only (other schemes pass through); cache keys keep the original spec. See the China-mirrors reference |
| `OXO_FLOW_K8S_DOCKER_SOCKET` | No | unset (= off) | `"1"`/`"true"` mounts the Kubernetes node's `/var/run/docker.sock` into generated Job pods (hostPath type `Socket`) and exports `DOCKER_HOST=unix:///var/run/docker.sock` — a **fallback-only** channel for jobs whose command reaches the node daemon (a non-renderable environment such as a local SIF/conda/pixi/venv/modules that folds its wrapper into the command, or a `system` rule that calls docker). The rule image must carry a docker CLI, and the mount grants host-root-equivalent access — opt-in, trusted single-operator clusters only; an unrecognized value stays off. Since #1388 a `docker = "<spec>"` or `singularity = "docker://<spec>"` environment is rendered as the pod's own image (the local wrapper is dropped and this socket is deliberately NOT attached — the pod cannot use it); non-renderable environments are annotated `oxoflow.io/environment-render: wrapper-fallback` in the Job and recorded per rule in the checkpoint `cluster_images` map (`provenance verify` prints it, digest only when the submit host's local docker reports one offline). Registry-restricted nodes additionally rewrite the manifest `image` through `OXO_REGISTRY_MIRRORS` at render time only (the identity/annotation keeps the original spec). Live evidence: `docs/superpowers/audits/kubernetes-image-render-2026-11-25.md` |
| `OXO_FLOW_K8S_IMAGE_RENDER` | No | unset (image rendering on) | `0`/`false`/`no`/`off` forces the pre-#1388 **wrapper fallback** for renderable container environments on the Kubernetes backend (issue #1388): the folded `docker run`/`singularity exec` wrapper is submitted again, pair it with `OXO_FLOW_K8S_DOCKER_SOCKET=1` for a docker rule that needs host paths outside the workdir or a nested container. Truthy values keep image rendering on; an unrecognized value warns and stays ON (the default). The forced fallback is annotated and recorded as `wrapper-fallback`; a `system` rule is always recorded/annotated as `system` (no wrapper exists) |
| `OXO_ENV_LOCK_TIMEOUT_SECS` | No | `7200` (2 h) | Max wait for the per-environment creation lock (flock under `~/.oxo-flow/locks/`, one per env — different envs never block each other); concurrent runs building the same env queue instead of racing, and expiry fails fast with a diagnostic instead of hanging. OS releases the lock if the holder dies |
| `OXO_S3_FORCE_PATH_STYLE` | No | unset | `"1"`/`"true"` forces path-style S3 addressing (`https://endpoint/bucket/key` instead of virtual-host style) for S3-compatible servers (MinIO, LocalStack); pair with `AWS_ENDPOINT_URL`. See the cloud-storage reference |
| `OXO_FLOW_CLUSTER_BACKEND` | No | `slurm` | Default backend for `oxo-flow cluster submit` when `-b/--backend` is absent: `slurm`, `pbs`, `sge`, `lsf`, `kubernetes`/`k8s`, `aws-batch`, `gcp-batch`, or `tencent-batch`. See the cluster command reference |
| `TENCENTBATCH_INSTANCE_TYPE` | No | `S5.MEDIUM4` | CVM instance type the `tencent-batch` backend bakes into each task's inline MANAGED compute env (`ComputeEnv.EnvData.InstanceType`). The profile's `queue` key carries the availability `Placement.Zone` (required) and `account` carries the `ImageId` (required) for this backend |
| `TENCENTBATCH_ENV_ID` | No | unset (inline env) | Target a PRE-CREATED named compute environment (`tccli batch CreateComputeEnv`) instead of an inline anonymous one: the task references `EnvId` and drops `ComputeEnv` entirely (`account`/ImageId then unused). Live-verified: some accounts fail anonymous job-level env creation while the same payload runs to SUCCEED on a named env |
| `OXO_FLOW_SECRET_REFS` | No | — | Operator-side secret-reference map for cloud dispatch (issue #1120, RB-12): `NAME=REFERENCE` pairs separated by `;`/`,`/newline (e.g. `OXO_FLOW_SENSITIVE_0=projects/p/secrets/token/versions/latest`), injected as GCP Batch `taskSpec.environment.secretVariables` entries for every routed `OXO_FLOW_SENSITIVE_<n>`; malformed entries are warned and skipped, never guessed. AWS Batch's `SubmitJob` has no secrets field (map the names on the job definition instead — the map is ignored there); Kubernetes uses the `oxoflow-secrets` Secret. See the workflow-format `[config]` section and run-on-cluster.md |

### AI Provider Examples

Compose passes AI/deployment env through: `docker-compose.yml` declares the variables below as value-less `environment:` entries, so host-exported vars flow into the container (`OXO_FLOW_AI_PROVIDER=... docker compose up -d` works as written). Unset vars are simply absent inside the container.

**Claude (Anthropic):**
```bash
OXO_FLOW_AI_PROVIDER=claude ANTHROPIC_AUTH_TOKEN=sk-ant-<YOUR-KEY> docker compose up -d
```

**DeepSeek via Anthropic-compatible API:**
```bash
OXO_FLOW_AI_PROVIDER=claude \
  ANTHROPIC_BASE_URL=https://api.deepseek.com/anthropic \
  ANTHROPIC_AUTH_TOKEN=sk-<YOUR-KEY> \
  ANTHROPIC_MODEL=deepseek-chat \
  docker compose up -d
```

**OpenAI-compatible (DeepSeek, Groq, Azure, etc.):**
```bash
OXO_FLOW_AI_PROVIDER=openai \
  OPENAI_BASE_URL=https://api.deepseek.com/v1 \
  OPENAI_API_KEY=sk-<YOUR-KEY> \
  OPENAI_MODEL=deepseek-chat \
  docker compose up -d
```

**Ollama (local):**
```bash
OXO_FLOW_AI_PROVIDER=ollama docker compose up -d
```

## AI Provider Architecture

The AI provider system supports four backends via an enum-based dispatcher:

- **Claude** — Anthropic Messages API (including Anthropic-compatible third-party endpoints)
- **OpenAI** — OpenAI Chat Completions API (compatible with DeepSeek, Groq, Azure, Together, etc.)
- **DeepSeek** — Native DeepSeek API (`DEEPSEEK_API_KEY`)
- **Ollama** — Local Ollama API (default: `http://localhost:11434`)

With `OXO_FLOW_AI_PROVIDER` unset, the provider is auto-detected from
standard shell credentials (`ANTHROPIC_AUTH_TOKEN`, `OPENAI_API_KEY`,
`DEEPSEEK_API_KEY`, `OLLAMA_HOST`, …) — "disabled" is only the outcome when
no provider/credential resolves, and an explicit `disabled` always wins
(config via env vars or `~/.oxo-flow/ai_config.json`).

The `scripted` test backend is compiled out of production builds: its constructors exist only under `cfg(test)` or the `test-scripted` Cargo feature, which only dev-dependencies enable (issue #1040 B3), so a real server cannot install a stub provider. The web chat SSE stream emits `ready` (keep-alive cadence + client idle budget) and per-round `progress` frames, bounds each provider call with `OXO_FLOW_AI_TIMEOUT_SECS`, cancels the loop when the client disconnects, and archives every agent-loop run (successes and failures) atomically (`docs/guide/src/reference/web-api.md`); `POST /api/ai/test` is single-flight with a 10 s result cache.

Providers are selected at startup via `OXO_FLOW_AI_PROVIDER` env var and initialized once through `AiProviderRegistry::global()` (web crate, `src/ai_provider.rs`). Web chat routes each message deterministically (issue #1040 P2-4, `domains/chat/intent.rs` — keyword/pattern classifier, no extra model call): conversational turns answer with one plain-text completion (`mode: "chat"`), while pipeline-intent turns (workflow vocabulary, omics assays, attached data, or a `run_id` scope) run generation. Workflow generation is UNIFIED (issue #342): every generation surface — CLI `template --ai`, `POST /api/ai/translate`, and pipeline-intent web chat — runs the shared `PipelineGenAgent` (oxo-flow-ai, `src/agent/pipeline_gen.rs`: engine-accurate prompt, canonical TOML extraction, injected validator) through the `Orchestrator` tool-calling + validation-feedback loop, differing only in provider resolution, tool policy, session destination, and presentation. The web keeps its deterministic template-keyword fallback and per-user provider/caching. The CLI additionally offers the opt-in Scientist Team profile (`--ai-team-profile full` / `[ai] team_profile`, oxo-flow-ai `src/agent/team.rs`: bounded task contract, deterministic Curator brief, independent review with one bounded fix); compact is the shipped default and the full profile's marginal value is measured in `eval/frontier/`. The CLI also exposes a harness-scoped `template --ai --ai-contract-feedback` flag (issue #1315): when set, `generate_workflow`'s validator closure additionally runs the engine's BioTypes (`TYPE-*`), scientific-preflight (`SCI-*`), and `deep_check` (`D00*`) checks (via the reused `check_workflow_types` / `analyze_scientific_constraints` / `compute_deep_check` entry points, `contract_feedback_errors` in `ai_template.rs`) and feeds their findings back as `ValidationResult::failed` errors. It is OFF by default — normal `template --ai` behavior (syntax + structural feedback only) is unchanged — and exists solely so `eval/frontier/run_eval.py --variants` can run the pre-registered matched three-arm ablation (Arm A `unconstrained` / Arm B `syntax-only` / Arm C `full-contract`, plan:109-111). A non-governed 36-intent operational run of the three arms is recorded in `docs/superpowers/audits/ai-synthesis-ablation-2026-11-26.md` (#1412: Arm A 0/36, Arm B 28/36, Arm C 18/36; McNemar B vs C p=0.0129); the governed pre-registered campaign (multiple seeds, held-out intents, pre-registered analysis) remains outstanding.

**Translate provider fallback is attributable and controllable** (`domains/ai/service.rs`). The chain is unchanged by default and runs the acting user's provider → the configured runtime → env-discovered Claude/OpenAI/Ollama; a successful generation returns top-level `provider`, `model`, `fallback_used`, and `attempts[]` (`{provider, model, success, reason}`) alongside the existing fields, a cache hit carries the original attribution, and every fallback logs a WARN naming the from/to provider, model, and prior failure. Exhausted chains return a structured `AI_TRANSLATE_ERROR` naming the last provider/model and the orchestrator cause (e.g. `Agent exceeded max rounds (10)`); a provider timeout's 504 `detail` names the last attempted provider. `OXO_FLOW_AI_FALLBACK=disabled` truncates the chain to the first usable provider; the default is `auto`. A billed attempt is terminal: a failure that may already have billed the provider (max-round exhaustion, empty/invalid response, tool error — `AiError::may_have_billed`) stops the chain instead of trying the next candidate, so one generation is never paid for twice; only network/authentication/rate-limit/5xx/timeout failures fall through. No API key or endpoint URL is ever included in attribution, attempts, logs, or errors: transport/HTTP failures are reduced to a sanitized category (`network error`, `authentication failed`, `provider error (HTTP 502)`, `request timed out`, `Agent exceeded max rounds (N)`) before entering `attempts[].reason`, the 400 message, or the WARN log. The CLI and web chat use a single resolved provider and have no cross-provider chain.

## AI CLI Integration

The `oxo-flow-ai` crate provides shared AI infrastructure for CLI and web.

### Quick Start

```bash
# Configure any AI provider
export OXO_FLOW_AI_PROVIDER=deepseek    # or claude, openai, ollama
export DEEPSEEK_API_KEY="sk-<YOUR-KEY>"

# Check status
oxo-flow ai                            # Shows provider, endpoint, test connectivity

# One-line enable in .oxoflow file
[ai]
enabled = true                          # All AI commands auto-activate

# Generate workflows
oxo-flow template "RNA-seq with STAR" --ai
oxo-flow template "variant calling" --ai --from-url https://example.com/protocol

# Analyze workflows (auto-detect — no --ai needed if [ai] enabled)
oxo-flow dry-run workflow.oxoflow      # AI resource/safety audit
oxo-flow validate workflow.oxoflow     # AI semantic validation
oxo-flow lint workflow.oxoflow         # AI best-practice check
oxo-flow debug workflow.oxoflow        # AI command explanation

# Error recovery (auto-detect from [ai] section)
oxo-flow run workflow.oxoflow          # AI diagnoses + fixes on failure
oxo-flow run workflow.oxoflow --ai-recover --ai-max-retries 3
```

### Architecture

| Crate | Role |
|-------|------|
| `oxo-flow-ai` | L1 Provider (4 backends) + L2 Knowledge + L3 Agent/Tools |
| `oxo-flow-cli` | L4 Command integration: template, dry-run, validate, lint, debug, run, resume, ai |
| `oxo-flow-web` | Web integration over oxo-flow-ai |

### Key Design Decisions

- **AI is optional**: no `[ai]` section → zero AI overhead
- **One line to enable**: `[ai] enabled = true` in .oxoflow → all commands auto-detect
- **Provider-agnostic**: OpenAI-compatible (DeepSeek, Groq, Together), Anthropic-compatible (Claude), Ollama local
- **Everything archived**: AI sessions + modifications auto-saved to `.oxo-flow/ai_sessions/`
- **5-layer architecture**: L1 Provider → L2 Knowledge/Config → L3 Agent/Tools → L4 Commands → Session

### Frontend Architecture
- **Framework**: React 19 + TypeScript, Vite build
- **Routing**: React Router (client-side SPA routing)
- **DAG Visualization**: React Flow (`@xyflow/react`)
- **API Integration**: Fetch-based client with structured error handling
- **Real-time**: SSE via EventSource for run lifecycle events
- **Styling**: CSS custom properties, dark theme default (light is a toggle), responsive layout

### Pages
| Route | Page | Description |
|-------|------|-------------|
| `/` | Dashboard | Engine status, recent runs, quick actions |
| `/editor` | Pipeline Editor | TOML editor + interactive DAG viewer + AI generate |
| `/pipelines` | Pipeline Library | Browse and use pipeline templates |
| `/runs`, `/runs/:id` | Run Monitor | Run history, SSE live events, run details |
| `/login` | Login | Sign-in for team/HPC modes |
| `/chat` | Chat | Real-time AI copilot chat |
| `/clusters` | Clusters | Cluster scheduler submission and monitoring |
| `/users` | Users | User management (admin) |
| `/audit` | Audit | Audit log browser |
| `/settings` | Settings | Server settings and AI configuration |
| `/docs` | API Docs | Embedded OpenAPI/Swagger viewer |
| `/share/:token` | Share | Shared pipeline view via token |

### Key Components
- `components/Layout.tsx` — Sidebar navigation + main content outlet
- `components/WorkflowCanvas.tsx` — Interactive DAG using React Flow + d3-dag
- `api/client.ts` — Typed API client with structured error handling
- `api/types.ts` — TypeScript types matching the Rust API responses
- `utils/identityStorage.ts` — identity-scoped client state (issue #1144 / UX-02): draft, chat, and canvas buckets are keyed by the username the server reports (`oxo_identity` caches it, `GET /api/auth/me` must confirm it before the first paint and an unconfirmed identity fails closed with no drafts — never a cached-identity fallback), logout wipes every identity-scoped key and suppresses further writes, and the pre-#1144 browser-wide `oxo_session` blob is discarded. Do not add new account-scoped `localStorage` keys outside this module
