# P3: AI Assistant — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** The web chat runs oxo-flow-ai's REAL agent loop — tool-calling Orchestrator grounded in the embedded knowledge bases, real token streaming, typed SSE events, persisted sessions, cancellation — replacing the single-shot + fake-streaming implementation.

**Architecture:** Add `chat_stream` + an event sink + a cancellation token to oxo-flow-ai (additive, CLI unaffected). The web chat handler drives the Orchestrator with a web ToolRegistry (read-only knowledge + run-scoped tools), forwards `AgentEvent`s as typed SSE, persists messages to `chat_messages`, and cancels on client disconnect. The ChatUI renders tool-call cards.

**Tech Stack:** reqwest SSE parsing (openai-compatible `stream: true`), `tokio_util::sync::CancellationToken`, ScriptedBackend for deterministic tests.

**Spec:** `docs/superpowers/specs/2026-08-14-web-full-lifecycle-design.md` §6.3.

## Global Constraints

- `make ci` per task; TDD red→green; conventional commits.
- The ScriptedBackend must keep every existing AI test deterministic — new streaming tests must NOT hit the network.
- Web SSE events: `status | tool_call | tool_result | text | action | done | error` (spec §6.3 protocol).
- No `write_file` tool in the web agent (approver = deny); saving generated workflows is the user's Accept click.

---

### Task 1: `AiProvider::chat_stream` (oxo-flow-ai)

**Files:**
- Modify: `crates/oxo-flow-ai/src/provider.rs`
- Test: inline unit tests (SSE line parser, no network)

**Interfaces:**
- Consumes: existing backend config (api_url/model/api_key), reqwest client.
- Produces: `pub async fn chat_stream(&self, system: &str, user: &str) -> Result<ChatStream, AiError>` where `pub type ChatStream = std::pin::Pin<Box<dyn futures::Stream<Item = Result<ChatStreamChunk, AiError>> + Send>>;` and `pub enum ChatStreamChunk { Text(String), Done { content: String, usage: Option<AiUsage> } }`.

- [ ] **Step 1: Write the failing tests** (pure-parser tests):

```rust
    #[test]
    fn parse_sse_chunks_extracts_deltas_and_done() {
        let body = "data: {\"choices\":[{\"delta\":{\"content\":\"fast\"}}]}\n\ndata: {\"choices\":[{\"delta\":{\"content\":\"p\"}}]}\n\ndata: [DONE]\n\n";
        let chunks = parse_openai_sse(body);
        assert_eq!(chunks.len(), 3);
        assert!(matches!(&chunks[0], SseEvent::Delta(s) if s == "fast"));
        assert!(matches!(&chunks[2], SseEvent::Done));
    }

    #[test]
    fn parse_sse_chunks_handles_empty_and_usage_lines() {
        let body = "data: {\"choices\":[{\"delta\":{\"content\":\"x\"}}],\"usage\":{\"total_tokens\":7}}\n\ndata: [DONE]\n\n";
        let chunks = parse_openai_sse(body);
        assert!(chunks.iter().any(|c| matches!(c, SseEvent::Delta(_))));
        assert!(chunks.iter().any(|c| matches!(c, SseEvent::Done)));
    }
```

  with a package-private `enum SseEvent { Delta(String), Done, Other }` + `fn parse_openai_sse(body: &str) -> Vec<SseEvent>` splitting on `\n\n`, taking lines starting `data: `, skipping `[DONE]` and unparseable JSON.

- [ ] **Step 2: Run to verify failure** — functions missing.
- [ ] **Step 3: Implement** — parser above; then in `impl AiProvider`:

```rust
    /// Stream a completion token-by-token (openai-compatible providers;
    /// other backends return a single `Done` chunk).
    pub async fn chat_stream(&self, system: &str, user: &str) -> Result<ChatStream, AiError> {
        match self {
            AiProvider::OpenAi(b) | AiProvider::DeepSeek(b) => b.chat_stream(system, user).await,
            // Single-shot fallbacks: the whole response as one Done chunk.
            other => {
                let text = other.chat(system, user).await?;
                Ok(Box::pin(futures::stream::iter(vec![Ok(ChatStreamChunk::Done { content: text, usage: None })])))
            }
        }
    }
```

  `OpenAiBackend::chat_stream`: POST with `"stream": true`, `resp.bytes_stream()`, accumulate lines → `parse_openai_sse` on buffered `\n\n` blocks (keep a leftover buffer), emit `Text`/`Done { content: accumulated, usage: last parsed usage }`. Map non-2xx via the existing `classify_http_error`.

- [ ] **Step 4: verify** (cargo test -p oxo-flow-ai) + clippy.
- [ ] **Step 5: Commit** `feat(ai): streaming chat for openai-compatible providers`.

---

### Task 2: Orchestrator event sink + cancellation (oxo-flow-ai)

**Files:**
- Modify: `crates/oxo-flow-ai/src/agent/orchestrator.rs`, `crates/oxo-flow-ai/src/agent/mod.rs` (or a new `agent/events.rs` — prefer the new file, orchestrator.rs is already large)
- Test: inline test with ScriptedBackend + a Vec<AgentEvent> sink asserting the event sequence (status → tool_call → tool_result → …).

**Interfaces:**
- Produces (in `agent/events.rs`):

```rust
#[derive(Debug, Clone, Serialize)]
pub enum AgentEvent {
    Status(String),
    ToolCall { name: String, args: String },
    ToolResult { name: String, summary: String },
    Text(String),
    Action(String, serde_json::Value),
    Done,
}
pub type AgentEventSink = dyn FnMut(AgentEvent) + Send;
```

- `Orchestrator::execute(&self, agent, ctx)` keeps its signature; a new `execute_with_sink(&self, agent, ctx, sink: Option<&mut AgentEventSink>, cancel: Option<CancellationToken>)` does the loop and the existing `execute` delegates with None/None. Emit: `Status("Planning")` on loop start, `ToolCall`/`ToolResult` around each tool execution (summary = first ~200 chars of the result), `Text` when no tools are called (full response), `Done` at the end. Check the cancellation token between provider calls and before each tool.

- [ ] **Step 1: failing test** — ScriptedBackend that answers one tool call then a final text; assert the recorded events contain a ToolCall for the expected tool name and a Done.
- [ ] **Step 2–5:** implement, verify, commit `feat(ai): orchestrator event sink and cancellation`.

---

### Task 3: Web chat runs the real agent loop (backend)

**Files:**
- Modify: `crates/oxo-flow-web/src/domains/chat/service.rs`, `crates/oxo-flow-web/src/domains/chat/handlers.rs` (typed SSE), new `crates/oxo-flow-web/src/domains/chat/tools.rs` (web ToolRegistry builder)
- Modify: `crates/oxo-flow-web/src/infra/db/sqlite.rs` (+`chat_messages` DDL; INSERT helper)
- Test: `crates/oxo-flow-web/tests/chat_agent_integration.rs` — ScriptedBackend configured via env, POST /api/chat/send, assert the SSE body contains `event: tool_call` and `event: done`.

**Interfaces:**
- Consumes: `oxo_flow_ai::{agent::{Agent, AgentContext, Orchestrator, AgentOutcome}, tools::{ToolRegistry, builtin::lookup_tool…}, provider::{AiProvider, ChatStreamChunk}, events::AgentEvent}`.
- Produces: `/api/chat/send` SSE stream with the spec's event names; `chat_messages(session_id, role, content, meta, created_at)` rows.

- [ ] **Step 1: failing test** — as above (ScriptedBackend returns a scripted tool call to `lookup_tool` then "Here is the pipeline" + pipeline TOML).
- [ ] **Step 2: run to verify failure** — today the SSE is the fake event stream.
- [ ] **Step 3: implement**:
  - `chat/tools.rs`: `build_chat_tool_registry(run_id: Option<String>) -> ToolRegistry` registering `lookup_tool`, `lookup_skill`, `lookup_pipeline` (from oxo-flow-ai builtins), `read_file` scoped via `workspace.rs` traversal validation (limit to run/pipeline dirs; reject otherwise), `get_run_status`, `get_run_logs`, `get_run_diagnostics` (read-only, backed by execution handler data paths).
  - `chat/service.rs`: `process_chat` → run the Orchestrator with an Agent whose system prompt uses `knowledge::assembler::for_generate`; history loaded from `chat_messages` by `session_id`; persist assistant/tool messages after Done.
  - `chat/handlers.rs`: `chat_send` builds `axum::response::sse::Sse` from an async-stream that drives the orchestrator and forwards events; cancellation via the `Sse`'s drop (wrap the token in a guard that cancels when the stream is dropped).
- [ ] **Step 4: verify** — new integration test green + full web crate + clippy.
- [ ] **Step 5: Commit** `feat(web): chat runs the grounded agent loop with real streaming`.

---

### Task 4: ChatUI tool-call cards (frontend)

**Files:**
- Modify: `frontend/src/components/ChatUI.tsx` (parse `tool_call`/`tool_result` events → collapsible cards: tool name, args (truncated), result summary; keep agent/status line, text streaming, pipeline_ready actions)
- Modify: `frontend/src/index.css` (+`.chat-tool-card` styles)
- Test: `frontend/e2e/chat-tool-cards.spec.ts` — with the ScriptedBackend server env (reuse the chat integration's scripted setup via env vars the test server inherits), assert the UI renders a tool card and the final text.

- [ ] **Steps:** implement component-wise, `npm run build`, run the new spec + full e2e, commit `feat(frontend): chat renders grounded tool calls`.

---

### Task 5: P3 final gate

- [ ] Full `make ci` + `npx playwright test` (all specs).
- [ ] Live round-trip against the user's DeepSeek endpoint (one tiny message; verify streamed deltas + a `lookup_tool` card when prompted about a tool).
- [ ] Update `docs/guide/src/reference/web-api.md` chat section (typed events) + memory.
