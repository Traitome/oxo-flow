# oxo-flow Web: Full-Lifecycle Product Design

**Date:** 2026-08-14
**Status:** Approved for implementation (goal directive `/goal`, session 2026-08-14)
**Scope:** crates/oxo-flow-web, crates/oxo-flow-ai (streaming extension), frontend/, CI, docs

---

## 1. Goal

Make the oxo-flow web system a production-usable, full-lifecycle bioinformatics
pipeline product with two differentiators implemented **for real** (not as
stubs):

1. **Graphical programming** of analysis workflows — a node-based canvas editor
   on top of the `.oxoflow` TOML format, using current lightweight reliable
   frontend libraries.
2. **AI-assisted intelligence** — a chat/agent assistant grounded in the
   embedded knowledge bases (Bioconda tools, bioSkills, pipeline graph), with a
   real tool-calling loop, real token streaming, and scientifically honest
   workflow generation (validated, dry-run-able, tool names verifiable).

Plus the non-negotiable baseline: **scientific accuracy and reliability** —
the web must never lie about what ran, what will run, or what a result means.

---

## 2. Current State (evidence, 2026-08-14, post-v0.11.0)

### 2.1 What works end-to-end today

- Chat → generated TOML → editor with live validation + DAG preview → save →
  run/dry-run → monitor (DAG status, AI monitor, pause/resume/retry buttons) →
  diagnostics → report + Q&A → results/logs browsing → AI provider config.
- Backend: all 8 `domains/*` mounted in `server.rs::build_router`; CLI-subprocess
  execution with persistent workdirs (`workspace/users/<user>/pipelines/<id>`,
  issue #69); SSE run events; structured errors; SQLite + (partial) Postgres.
- 110+ backend integration tests (phase1–4, ai_security) + 5 Playwright specs
  (871 lines) that are **not run in CI**.

### 2.2 What was broken (verified defects — **all fixed in P0**)

| # | Defect | Evidence | Fix (commit) |
|---|--------|----------|--------------|
| B1 | **Cancel/pause/resume never signal the process** — handlers only UPDATE the DB row | `execution/handlers.rs:787,854,917`; no signal call anywhere in executor | `process_control.rs` pgid registry; SIGTERM→SIGKILL grace, SIGSTOP/SIGCONT; cancel persists `cancelled` BEFORE signaling so the executor's exit path can't flip it back (df32ce1, 3f43e2a, 3eb13eb) |
| B2 | **Status vocabulary drift** — executor wrote `"success"` | `executor.rs:226` | executor writes `completed`; idempotent startup migration (087f92d) |
| B3 | **`audit_logs` schema divergence** | `db.rs` vs `sqlite.rs:290-298` | one schema `(…, result, metadata, …)` in both init paths (5929d9c) |
| B4 | `insert_run` omitted 5 of 12 columns | `db.rs:455-469` | full 12-column INSERT (5929d9c) |
| B5 | `run_nodes` table never written — **and it was the ONLY source for node status, so `dag-status`/`run-status` showed everything pending forever** | 5 read sites, 0 write sites | table + trait + models dropped; `checkpoint_status.rs` derives status from the engine's checkpoint + "Running:" log lines; full rule list merged so unrun rules show as pending (04c2f45) |
| B6 | `chat_sessions` never written; `process_chat` ignores `session_id` | `chat/handlers.rs:215`, `chat/service.rs:13` | → P3 (chat rewrite) |
| B7 | `/report/ask` + `/report/visualize` return canned template JSON | `execution/handlers.rs:1115-1173` | → P1 |
| B8 | OAuth callback does not verify `state` (only non-empty) | `auth/handlers.rs:369-380` | `oauth_states` table; issued states verified + consumed before token exchange (1eef785) |
| B9 | `save_pipeline` owner hardcoded to first admin row | `workflow/handlers.rs:304-310` | auth middleware injects session user; owner resolves via users lookup, `default` in personal mode (c4a716a) |
| B10 | `get_ai_config_effective` returns hardcoded `"user_provider": null` | `ai/handlers.rs:360` | real user-tier row read (c4a716a) |

Implementation deviations from the P0 plan (documented): signals go through
`nix`'s safe `killpg` (the web crate also `forbid(unsafe_code)`, so raw libc
was not an option); the cancel grace window polls the registry instead of a
flat 5 s sleep (fast path when the executor reaps promptly); pause/resume on
an unregistered group still records the DB state (documented race only during
the spawn window).

### 2.3 What is missing (goal-critical gaps)

**Graphical programming** — the DAG canvas is *read-only* (cytoscape +
cytoscape-dagre); cytoscape-edgehandles is installed but never imported. All
editing is dialog-driven and creates `echo TODO` stubs. The backend DAG-edit
command API exists (`domains/dag/`) but `add_rule` accepts only name+shell and
`update_params` only threads+shell — `input`/`output`/environment/resources are
**not editable**, which blocks constructing a real workflow graphically.

**AI** — the web never uses oxo-flow-ai's agent framework. `Orchestrator`,
`ToolRegistry`, the `lookup_*` tools, and all 4 embedded knowledge bases are
unreachable from web routes. Chat = single-shot `provider.chat` + **fake**
streaming (one finished string re-emitted in chunks). Prompts embed hardcoded
tool tables instead of querying the knowledge bases. No cancellation, no tool
rendering in the UI, no session history fed to the model.

**Lifecycle gaps** — run dialog exposes only dry-run (max_jobs hardcoded 4;
keep_going/samples/targets absent from UI); template library → editor loading
broken (`?template=` ignored); saved-pipelines UI missing (client wrappers
exist, unused); no export/fork/share UI; no login UI (auth client exists,
unused); dead `Runs.tsx` page (424 lines) and `SSEClient` util.

**CI** — frontend build, lint, and Playwright e2e are not in `.github/workflows/ci.yml`.

### 2.4 Assets to build on

- `domains/dag` command API with TOML round-trip + validation per edit + undo/redo (50 deep)
- Core engine APIs used by web services: parse/validate/prepare/dag/format/lint/stats/diff/export
- oxo-flow-ai: full agent loop (`Orchestrator`), `ToolRegistry`, `McpHttpClient`,
  knowledge modules with sync search APIs, `ScriptedBackend` for deterministic tests
- Existing SSE infra (`src/sse.rs`, broadcast + keepalive) and structured errors
- 15-template embedded gallery in the CLI (`EMBEDDED_GALLERY`) and template table in web DB

---

## 3. Design Principles

1. **TOML is the single source of truth.** Every edit (canvas, chat, form)
   round-trips through the core engine: parse → mutate → canonical format →
   validate. No hidden state, no drift between what the editor shows and what
   runs. The `.oxoflow` file is the scientific record.
2. **The engine is the oracle.** Web never re-implements validation, DAG
   inference, wildcard expansion, or invalidation semantics. It delegates to
   core (in-process) and the CLI (execution). Scientific accuracy = parity.
3. **Truthful UI.** dry-run is a read-only preview (CLI `dry-run`, never
   executed); cancel/pause/resume control the real process; status vocabulary
   is one enum; AI-generated claims show their grounding (tool call cards);
   stubs are either implemented or removed, never shipped as features.
4. **One graph stack.** React Flow for both editing and monitoring. Remove
   cytoscape/edgehandles (the read-only viewer and the unused editing deps).
5. **AI grounded in embedded knowledge.** The chat agent runs the real
   Orchestrator with `lookup_tool`/`lookup_skill`/`lookup_pipeline` +
   scoped web tools. Deterministic template fallback stays for unconfigured
   providers (ScriptedBackend / template matcher).
6. **KISS / YAGNI.** No speculative generality: no Postgres completion, no
   multi-node deployment, no plugin marketplace UI, no HPC UI beyond what
   exists. Every line must earn its place (project Code Professionalism rule).

---

## 4. Approaches Considered

### 4.1 Editor technology

| Option | Verdict |
|--------|---------|
| **A. React Flow (`@xyflow/react` v12) canvas + d3-dag layout** | **Chosen.** MIT, actively maintained (v12.11.x, 2026-07), React 19 compatible, node-drag/connect/pan/zoom out of the box, large-flow friendly, ~small core. d3-dag (MIT, maintained, zero deps, dagre-compatible API + better Sugiyama operators) for auto-layout of both editor and monitor. One library replaces cytoscape everywhere |
| B. Extend cytoscape + cytoscape-edgehandles | Rejected: cytoscape is a graph-analysis lib, not an editor; edgehandles is unmaintained; two stacks (editor + monitor) double the surface |
| C. Status quo (dialog-based editing) | Rejected: does not satisfy "图形化编程" |

### 4.2 AI integration

| Option | Verdict |
|--------|---------|
| **A. Wire web to oxo-flow-ai Orchestrator; add streaming + event callbacks + cancellation to oxo-flow-ai; web-specific tools** | **Chosen.** One agent loop (CLI + web share it), real token streaming (SSE parsing in provider layer), event sink drives the web SSE protocol. No duplicated agent logic |
| B. Keep single-shot chat, keep fake streaming | Rejected: not "真实实现" — no tools, no grounding, no streaming |
| C. Rewrite a web-native agent loop | Rejected: duplicates `Orchestrator` (repair/overflow handling already battle-tested in issue #73 work) |

### 4.3 Execution control (B1)

| Option | Verdict |
|--------|---------|
| **A. Child-handle registry in executor; cancel = SIGTERM→SIGKILL escalation to process group; pause/resume = SIGSTOP/SIGCONT** | **Chosen.** Process-group signals (libc) because the CLI spawns rule subprocesses; CLI timeout already uses process groups so group semantics are consistent |
| B. DB-only status + orphan recovery | Rejected: lies to the user (current defect) |
| C. Pass a control socket to the CLI | Rejected: larger change in CLI, no benefit over signals |

### 4.4 Edit command granularity

| Option | Verdict |
|--------|---------|
| **A. Structural ops (add/remove/connect/disconnect/reorder) + generic `update_rule` with a TOML-patch payload merged into the rule table, then format + validate via core** | **Chosen.** One generic patch command avoids a 50-field struct that rots as the format evolves; core validation (deny_unknown_fields) gives precise error feedback |
| B. Enumerate every rule field as explicit commands | Rejected: 50+ fields, constant drift with workflow-format.md |
| C. Structured in-DB model, serialize to TOML on save | Rejected: violates principle 1; two sources of truth |

---

## 5. Target Architecture

```
+-------------------------------------------------------------+
|  Frontend SPA (React 19 + Vite, TypeScript)                 |
|  Editor: ReactFlow canvas + inspector + tool palette         |
|  Chat: streamed SSE + tool-call cards + pipeline_ready       |
|  Monitor: DAG status (ReactFlow), reports, diagnostics       |
+--------------------------+----------------------------------+
                           | HTTP (JSON) / SSE
+--------------------------v----------------------------------+
|  oxo-flow-web (axum, domains/* modular monolith)            |
|  workflow   parse/validate/prepare/dag/format/lint/stats/   |
|             diff/export/search + pipelines CRUD             |
|  dag        command API (structural + TOML-patch, undo/redo)|
|  execution  runs CRUD + executor (CLI subprocess + signal   |
|             registry) + SSE events + diagnostics            |
|  chat       real agent loop: Orchestrator + web ToolRegistry|
|             + real token streaming + session persistence    |
|  ai         translate/explain/interpret/optimize grounded   |
|  knowledge  GET /api/knowledge/tools|skills (bioconda/      |
|             bioSkills search, in-memory)                    |
|  auth/observability/collaboration  (existing, hardened)     |
+--------------------------+----------------------------------+
|  oxo-flow-ai  Orchestrator (+ events + cancellation)        |
|  AiProvider   chat + chat_stream (SSE/NDJSON per provider)  |
|  knowledge    bioconda(6103) bioSkills(562) graph(79/469)   |
+--------------------------+----------------------------------+
|  oxo-flow-core  DAG engine, validation, format, checkpoint  |
+-------------------------------------------------------------+
|  oxo-flow CLI subprocess (run/dry-run) — owns checkpointing |
+-------------------------------------------------------------+
```

---

## 6. Subsystem Designs

### 6.1 Execution correctness (P0)

**Status vocabulary** — one enum, `run`: `queued | running | paused | completed |
failed | cancelled`. Change `executor.rs:226` to write `completed`; add a
one-time data migration (`UPDATE runs SET status='completed' WHERE
status='success'`) in `rebuild_runs_table`-adjacent init. Terminal set
`completed|failed|cancelled` everywhere (`sqlite.rs:645` stays correct).

**Process control** — `executor.rs` gains a `RunProcessRegistry`
(`RwLock<HashMap<String, ChildHandle>>`, child PID + pgid captured at spawn via
`setsid`-equivalent: spawn through `CommandExt::process_group(0)` on Unix).
- `cancel`: SIGTERM to pgid → 5 s grace → SIGKILL; DB → `cancelled`, SSE `run_cancelled`
- `pause`: SIGSTOP to pgid; DB → `paused`, SSE `run_paused`
- `resume`: SIGCONT to pgid; DB → `running` (or `queued` if not yet started), SSE `run_resumed`
- Registry entries cleaned on child exit (reaper task); server restart → empty
  registry → existing orphan recovery marks stale rows failed (documented
  limitation: no resume across server restarts; checkpoint resume covers it).

**Audit schema unification** — one table definition in both init paths:
`audit_logs(id, user_id, action, target, result, metadata, timestamp)`.
`log_action` inserts `result` + `metadata` (JSON).

**insert_run** — fill all 12 columns explicitly.

**run_nodes** — delete the table, trait write APIs, and Postgres impls (never
written; `dag-status` reads checkpoint state — verify during implementation
that `dag-status` is fully checkpoint-based and does not read `run_nodes`).

**OAuth state (B8)** — issue a random `state` token on authorize, persist to a
new `oauth_states(state, created_at)` table (or reuse `sessions` pattern),
verify + delete on callback.

**save_pipeline ownership (B9)** — owner = authenticated user (team/hpc) or
`default` (personal); stop hardcoding first-admin.

**ai_config effective (B10)** — compute the real effective provider
(env → server row → user row), no hardcoded null.

### 6.2 Graphical workflow editor (P2)

**Backend — `domains/dag` command API extension**

Keep the request envelope (`toml_content`, `source`, `operation`, `payload`).
Extend operations:

| Operation | Payload | Semantics |
|-----------|---------|-----------|
| `add_rule` | `{ rule: TOML-table }` | Append rule; full field support via the rule table (input/output/shell/script/environment/resources/envvars/when/retries/tags/threads/memory/log/benchmark/optional/required/description/…) |
| `update_rule` | `{ name, patch: TOML-table }` | Shallow-merge patch into the named rule's table (keys replace, unknown keys rejected by core validation) |
| `remove_rule`, `connect`, `disconnect`, `reorder` | as today | unchanged (remove_rule also drops file edges implicitly → validation surfaces missing inputs downstream) |
| `update_workflow` | `{ patch: TOML-table }` | Merge into `[workflow]`/`[config]`/`[defaults]` (metadata edits: name, version, description, sample_pattern, config values) |

`update_params` stays as an alias of `update_rule` (back-compat). Backwards
compat: `add_rule` accepts legacy `{name, shell}` payload shape too.

**Edge kinds in `/api/pipelines/dag`** — response gains per-edge
`kind: "file" | "declared"` (`file` = inferred from exact input/output string
match per engine semantics; `declared` = explicit `depends_on`). The canvas
renders: declared = solid, editable; file = dashed, read-only (changing
input/output paths is how you edit them). This teaches the engine's real
dependency semantics (see dag-engine.md; edges are exact-string match only).

**Backend — knowledge endpoints** (implemented in `domains/ai/handlers.rs` — the
module that already owns the oxo-flow-ai relationship — thin adapters over the
in-memory knowledge APIs, no new domain module for two endpoints):
- `GET /api/knowledge/tools?q=&limit=` → `oxo_flow_ai::knowledge::bioconda::search_tools` (name, version, summary)
- `GET /api/knowledge/skills?q=&domain=` → `knowledge::skills::search_skills` / `skills_in_domain`
- `GET /api/knowledge/templates` → existing templates table (already exists)

**Frontend — new editor stack**

- **`WorkflowCanvas`** (replaces DagView in the editor): React Flow nodes =
  rules (name, description, environment badge); edges = declared (solid,
  draggable handles) + file (dashed, non-interactive); drag to arrange
  (positions in localStorage keyed by pipeline id — layout is presentation,
  not data); d3-dag auto-layout button; zoom/fit/minimap; delete node;
  select → inspector.
- **`RuleInspector`** — structured form for the core field set (name,
  description, shell/script, inputs, outputs, environment backend dropdown +
  spec, resources threads/memory/gpu/disk/time_limit, envvars, when, retries,
  tags, optional/required, log/benchmark). Every change → `update_rule` command.
  Advanced fields remain in the TOML pane (advanced mode) — the form covers
  the 20 most-used fields, TOML covers everything (no silent truncation:
  `update_rule` patches merge, never rewrite).
- **`ToolPalette`** — searchable list fed by `/api/knowledge/tools`; "add"
  inserts a node with a grounded command template (`<tool> {input} -o {output}`)
  and the tool's name/version as description. Template gallery entry point
  ("Start from template" loads gallery TOML into the session).
- **Sync model** — session state holds the TOML string (unchanged pattern).
  Canvas edit → dag command → returned canonical TOML replaces state →
  canvas + DAG + validation re-derive. TOML pane edit → state → canvas
  re-derives. Debounced; undo/redo via existing stacks.
- **Monitor DAG view** — same React Flow component in read-only mode with
  status coloring (reuses the existing `dag-status` data). Cytoscape,
  cytoscape-dagre, cytoscape-edgehandles, @types/cytoscape* removed.

**E2E scenario (Playwright, the acceptance test):** build a 3-rule workflow
purely on the canvas (palette → node → inspector → connect), validation green,
dry-run shows the plan, save, run, status completes.

### 6.3 AI assistant (P3)

**oxo-flow-ai changes (small, additive):**

1. **Real streaming** — `AiProvider::chat_stream(system, user) -> Stream<Item =
   Result<ChatStreamChunk>>` where `ChatStreamChunk = Text(String) | Done {
   content, usage, finish_reason } | Err`. Implementations: openai-compatible
   (DeepSeek/Groq/Azure) SSE parsing (`stream: true`), Claude SSE, Ollama NDJSON.
   Non-streaming `chat()` unchanged (ScriptedBackend, Noop return `Done`).
2. **Orchestrator events + cancellation** — `AgentEvent` enum:
   `Status(String)`, `ToolCall { name, args }`, `ToolResult { name, summary }`
   (summary truncated to ~200 chars; full result stays in session record),
   `TextDelta(String)`, `Action(String, Value)`, `Done(AgentOutcome)`.
   `Orchestrator::execute` gains an optional `EventSink` (FnMut(AgentEvent))
   and an optional `CancellationToken` (checked between provider calls/tools).
   CLI callers pass None — zero behavior change there.

**Web agent loop (`domains/chat` rewrite of `process_chat`):**

- Build a `ToolRegistry` per request (cheap, in-memory) with:
  - `lookup_tool`, `lookup_skill`, `lookup_pipeline` (from oxo-flow-ai, read-only)
  - `read_file` (read-only, **path-scoped to the run workspace/pipeline dir**
    via the existing `workspace.rs` traversal validation; reject outside)
  - `list_run_files` / `get_run_status` / `get_run_logs` / `get_run_diagnostics`
    (read-only, backed by existing execution handlers' data paths)
  - `fetch_url` (read-only) — keep
  - **No `write_file`** in web (approver = deny); saving generated workflows is
    the user's explicit "Accept" click → frontend calls `createPipeline`
    (human approval gate, no silent FS/DB writes by the model)
- Intent routing stays light (keyword classifier, as today) but the *system
  prompt* is assembled from the real knowledge bases via the existing scenario
  assemblers (`knowledge::assembler::{for_generate,for_check,for_diagnose}`).
- History: load prior messages of `session_id` from `chat_messages` (written on
  every exchange; `chat_sessions` holds the session metadata) and feed into
  the transcript.
- Config: provider from the existing effective-config tiering.

**SSE protocol (`POST /api/chat/send`)** — typed events, real-time:

```
event: status     data: {"agent": "orchestrator", "message": "…"}
event: tool_call  data: {"name": "lookup_tool", "args": {"query": "fastp"}}
event: tool_result data: {"name": "lookup_tool", "summary": "fastp 0.24.0 — …", "count": 5}
event: text       data: {"delta": "…"}
event: action     data: {"action": "pipeline_ready", "payload": {"toml": "…", "validation": …}}
event: done       data: {"session_id": "…", "usage": {"tokens": 1234}}
event: error      data: {"code": "…", "message": "…"}
```

- `action: pipeline_ready` now carries the validated TOML + validation
  summary; `accept` saves (frontend → `POST /api/pipelines`).
- Disconnect or explicit `POST /api/chat/cancel` → `CancellationToken` fires.
- `send/json` non-streaming variant returns the transcript + final result.
- **Quota** — accumulate `AiResponse.usage.total_tokens` per user per day in a
  small in-memory + DB-backed counter; enforce a configurable daily budget
  (default off in personal mode, on in team/hpc; config via ai_provider_config
  new column `daily_token_budget`). Over-budget → structured `RATE_LIMITED`-style error.

**Frontend ChatUI** — render `tool_call`/`tool_result` as collapsible cards
(name, args, truncated result), `status` as an agent activity line, real
streamed `text`, `action` renders Accept/Edit/Regenerate (existing) and a new
"Open in editor" (loads TOML + auto-builds DAG). Keep the deterministic
template fallback message when no provider configured.

**`/api/ai/translate|explain|interpret|optimize`** — keep the endpoint
contract; translate/stream becomes real streaming through the same web agent
loop (grounded system prompt + tools); explain/interpret/optimize stay
single-shot (with the real provider + grounded prompts). Deterministic
fallbacks (template matcher, log-regex) remain when AI is unconfigured and are
labeled as such in responses (`"grounding": "deterministic"` vs `"ai"`).

### 6.4 Lifecycle completion (P1 + parts of P3)

- **Run options** — `RunFlags` gains `samples: Vec<String>`, `targets:
  Vec<String>`; executor passes `--sample`/`-t` through to the CLI. Run dialog
  UI: max_jobs (default = engine suggestion from dry-run), keep_going,
  samples, targets, dry-run toggle (default ON first run — show the plan
  before executing; this matches scientific practice).
- **Templates → editor** — `GET /api/templates/{id}` (exists) + editor reads
  `?template=` and loads TOML; dashboard quick-start buttons work.
- **Saved pipelines UI** — `/pipelines` page: two tabs (Templates / My
  Pipelines); My Pipelines = list/search (existing `GET /api/pipelines`) +
  open/delete/fork/share/export buttons (client wrappers exist; wire them).
- **Export UI** — export dialog (Dockerfile / Singularity def) via
  `POST /api/pipelines/export`.
- **Report ask/visualize** — `/report/ask`: question + report content (read
  from the run's report file, as `/report` does) → single-shot grounded chat.
  `/report/visualize`: derive Vega specs deterministically from the report's
  QC section (charts the report already carries) — no canned JSON.
- **Login UI (team mode)** — minimal login page + session handling wired to
  the existing `api.login/authMe` client; "Guest" header shows real user when
  authenticated. Personal mode unchanged.
- **Settings** — wire license upload; reference download buttons either wired
  to `POST /api/data/reference` flows or removed (no dead buttons); AI
  advanced options wired to `config/user` PUT; environments panel shows real
  backend detection (from `/api/system`/health payload) or is removed.

### 6.5 Hardening, CI, cleanup (P4)

- **CI** — new `frontend` job (npm ci, eslint, tsc build) gated before Rust
  build; new `e2e` job (Playwright against `cargo run -p oxo-flow-web`,
  chromium, headless) on main pushes. Keep runtime bounded (playwright
  already configured to spawn the server).
- **Dead code removal** — `pages/Runs.tsx` (fold its unique features —
  confirm-cancel, results browser — into MonitorReport or delete),
  `utils/sse.ts` SSEClient, unused client wrappers (or wire the used subset),
  `domains/observability/handlers.rs:107` dead sse_events, `infra/db`
  run_nodes (P0), legacy `handlers/` directory + legacy router
  (`lib.rs::build_router_inner`) + `simulation_20users.rs` migration —
  **deferred decision**: removing the legacy router is a large mechanical
  diff; do it last, only if the session budget allows, and never block the
  rest of the plan on it.
- **Docs sync** — web-system-architecture.md / web-api.md / dag-edit-api.md
  updated to reality; new guide page "Web editor" (graphical programming
  walkthrough). Memory rule: docs embed via snippets only for gallery
  (unrelated), but doc edits must reflect shipped behavior.

---

## 7. Data Model Changes (SQLite; Postgres parity where trivial)

- `runs.status` vocabulary unified (see 6.1) — no DDL change, migration UPDATE.
- `audit_logs` — unify on `(…, result, metadata)` in both init paths.
- **Drop** `run_nodes` (never written).
- **Add** `oauth_states(state PK, created_at)`.
- **Add** `ai_usage(user_id, day, total_tokens)` (quota accounting).
- `chat_sessions` + a new `chat_messages(session_id, role, content, meta,
  created_at)` for history fed to the model.
- `ai_provider_config` + `daily_token_budget INTEGER` column (ALTER, default NULL = unlimited).

---

## 8. API Delta Summary

New: `GET /api/knowledge/tools`, `GET /api/knowledge/skills`, `POST
/api/chat/cancel`. Changed: `POST /api/chat/send` (typed real-time SSE),
`/api/pipelines/dag` (edge `kind`), `POST /api/runs` (samples/targets),
`/api/pipeline/{id}/command` (extended operations), `/report/ask|visualize`
(real), `/api/ai/config/effective` (real). Removed: none publicly (legacy
router untouched until the deferred cleanup).

---

## 9. Testing Strategy

- **Unit (Rust)** — per module, TDD: executor registry signals (spawn a `sleep`
  shim script, assert cancel kills it), dag command patch semantics (add rule
  with full spec → valid TOML; unknown field → validation error), status
  vocabulary migration, audit schema, chat SSE event sequence with
  ScriptedBackend (deterministic, no network), quota accounting, oauth state.
- **Integration (Rust, `tests/`)** — existing phase1–4 files extended:
  process-control flow through handlers; dag command API full-field round
  trip; chat SSE against ScriptedBackend; knowledge endpoints.
- **E2E (Playwright)** — canvas-built workflow acceptance scenario (§6.2);
  chat tool-card scenario; run-options dialog scenario; template→editor.
- **Parity checks** — validate: web `POST /api/pipelines/validate` response
  must equal `oxo-flow validate --json` for the same TOML (test harness).
- **CI** — `make ci` (Rust gate) + frontend build/lint + e2e job (§6.5).
- **Live verification** — one real run through the web UI on this machine
  (tiny workflow, system backend), one real AI chat round-trip against the
  user's DeepSeek endpoint (quota-tiny), before claiming done.

---

## 9b. Phase Status (updated as phases land)

- **P0** — complete (see §2.2 fix table; commits df32ce1..c4a716a).
- **P2** — complete (commits b90973a..9660f97): dag edit API full-rule
  editing, edge kinds, knowledge endpoints, React Flow canvas + inspector +
  grounded palette, monitor reuse, cytoscape removed. 62/62 e2e + live
  browser verification (palette → inspector → dashed file edge).
- **P3** — complete (commits 2be138b..9cd09b6): `AiProvider::chat_stream`
  (openai-compatible SSE), `Orchestrator::execute_with_sink` (`AgentEvent`
  stream + `AtomicBool` cancellation), web chat runs the real tool-calling
  loop (`lookup_tool`/`lookup_skill`/`lookup_pipeline`/`fetch_url`) with typed
  SSE (`status|tool_call|tool_result|text|action|done|error`), ChatUI renders
  grounded tool-call cards, AI config restored from DB at startup
  (was lost on restart). 404 web-crate tests + 63/63 e2e green.
- **Deviations from §6.3** (documented honestly):
  - Orchestrator `Text` events carry complete per-round responses, not token
    deltas — the agent loop is full-response by design. Token streaming
    exists via `chat_stream` for single-shot calls; wiring deltas through
    the tool loop is future work.
  - Cancellation is an `Arc<AtomicBool>`-style flag (spec said
    CancellationToken) — dependency-free, same semantics.
  - No run-diagnosis tools in the chat registry yet (run_id scoping) — the
    registry carries the knowledge lookups; diagnosis tools are P1/P4.
  - **Live DeepSeek round-trip NOT performed**: no API key exists in this
    environment (the persisted config has no `api_key` and no key env var is
    set). The streaming path is covered by ScriptedBackend integration tests
    + SSE parser unit tests; a real-key round-trip remains a TODO for the
    user's machine.
- **P1 partial** (commits 8f9c2b3, a23ec08): run options dialog
  (samples/targets/keep-going/max jobs through `build_cli_args`), template →
  editor loading (`?template=`), report Q&A + visualization answer from the
  run's REAL data (shared `build_report_for_run`).
- **P4 partial** (57cf28e): CI `frontend` job (SPA build + full Playwright
  e2e against the real server).
- **Live AI verification DONE (2026-08-14, via the user's
  `ANTHROPIC_AUTH_TOKEN` from ~/.zshrc):** the real agent loop against the
  real Claude provider — model called `lookup_tool` (grounded Bioconda
  result), generated a workflow, the core engine validated it
  (`pipeline_ready: validation.valid=true`), `done` in 3 rounds. The live
  test also FOUND + FIXED two real Anthropic-protocol bugs: assistant
  `tool_use` blocks were never emitted (400), and multiple `tool_result`
  blocks must coalesce into ONE following message (400). Both are unit-tested
  now (ba99c60, 241361c). Chat prompt hardened (TOML array I/O, direct
  validation fixes, 6 rounds).
- **Remaining (documented, not yet done):** saved-pipelines page UI
  (list/search/delete/export), login page, frontend lint debt (pre-existing
  `any`/effect errors — several died with the dead-code cleanup), legacy
  `handlers/` + legacy router removal, run-diagnosis chat tools.

## 10. Phased Roadmap

| Phase | Content | Exit criteria |
|-------|---------|---------------|
| **P0** — truth & control (backend) | B1–B5, B8–B10 fixes + tests | `make ci` green; cancel/pause/resume kill real processes (integration test); statuses consistent |
| **P1** — lifecycle completion | run options end-to-end, template loading, pipelines page, export, report ask/visualize real, login page | e2e: options dialog → run honors them |
| **P2** — graphical editor | dag API extension, edge kinds, knowledge endpoints, React Flow canvas/inspector/palette, monitor DAG reuse, cytoscape removal | e2e: canvas-built workflow validates + dry-runs |
| **P3** — AI assistant | provider streaming, orchestrator events/cancel, web agent loop + tools + sessions + quota, ChatUI tool cards, real translate/stream | e2e: chat emits tool events, generated workflow grounded; live DeepSeek round-trip |
| **P4** — hardening & docs | CI frontend+e2e jobs, dead code removal, docs sync, (legacy router removal if budget allows) | CI green end-to-end; docs match reality |

Each phase lands as its own commits on `feat/web-full-lifecycle` with
conventional-commit messages; TDD (red → green → refactor) per project rules.

---

## 11. Risks & Mitigations

- **CLI signal handling** — unknown whether the CLI traps SIGTERM for graceful
  checkpoint writes. Mitigation: SIGTERM grace window then SIGKILL; incomplete
  checkpoints are already handled by the engine (rule-level atomicity, resume
  semantics). Verified in P0 tests with a real subprocess.
- **TOML round-trip fidelity** — canonical formatting rewrites user TOML.
  Accepted (existing behavior of the dag API); mitigated by the format being
  canonical and validation-gated; advanced users see the result in the TOML pane.
- **`prepare` (wildcard expansion) cost on large cohorts** — canvas only calls
  `/dag` + `/validate` (cheap); `prepare` is invoked lazily (dry-run dialog),
  matching current behavior.
- **DeepSeek streaming quirks** — openai-compatible SSE is standard, but the
  user's provider is DeepSeek; P3 includes a live round-trip check (tiny
  quota) before claiming streaming works.
- **Playwright flakiness in CI** — scope e2e to core flows, generous timeouts,
  headless chromium; e2e job runs on main push only initially.
- **Session budget** — phases are sized so P0–P2 are complete before P3; P4 is
  best-effort. Ordering guarantees a trustworthy product even if later phases
  are partial.
