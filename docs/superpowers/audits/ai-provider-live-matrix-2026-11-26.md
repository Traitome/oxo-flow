# AI provider/config live matrix — 2026-11-26

Endpoint privacy: the operator's proxy hostnames and keys are not committed; this
record refers to them as `<proxy-1>` (OpenAI-compatible `/csuapi/v1`) and
`<proxy-2>` (`/v1`, Anthropic + OpenAI). Keys were passed only via process
environment; temp `OXO_FLOW_AI_CONFIG` / `OXO_FLOW_AI_SESSION_DIR` were used.

Binary under test: branch `fix/ai-fallback-attribution-2026-11-26` at
`ebd8ec6a` (review-fix round, rebased onto main `2148f747`), debug build. The
pre-redaction run at `f541b006` observed the endpoint URL in the reason; the
final code redacts it, and the post-fix rerun below confirms the redaction.

## Fallback attribution (live)

Server: `OXO_FLOW_AI_PROVIDER=openai` with an intentionally unreachable primary
(`OPENAI_BASE_URL=http://127.0.0.1:9/v1`, model `unreachable-primary`) plus a
reachable Anthropic-compatible fallback (`<proxy-2>`, model
`qwen3.8-flash-next-nothink`), default `OXO_FLOW_AI_FALLBACK=auto`.

`POST /api/ai/translate` (simple one-rule intent) → HTTP 200 in 20.0 s (post-fix):

```json
{
  "provider": "claude",
  "model": "qwen3.8-flash-next-nothink",
  "fallback_used": true,
  "attempts": [
    {"provider": "openai", "model": "unreachable-primary", "success": false,
     "reason": "request failed after 3 attempts"},
    {"provider": "claude", "model": "qwen3.8-flash-next-nothink", "success": true, "reason": null}
  ],
  "explanation": {"steps": [{"rule": "ping", "purpose": "ping step (via claude (qwen3.8-flash-next-nothink))"}]}
}
```

The old silent fallback is now machine-readable and the explanation no longer
claims a stale `via claude (fallback)` label.

## Fallback opt-out (live, fail-closed)

Same primary/fallback with `OXO_FLOW_AI_FALLBACK=disabled`:
`POST /api/ai/translate` → HTTP 400 in 7.0 s (post-fix; no endpoint URL in the message):

```json
{"code":"AI_TRANSLATE_ERROR",
 "message":"AI generation failed on openai (unreachable-primary): request failed after 3 attempts",
 "detail":null,"suggestion":null}
```

Server log shows only the openai attempts — no claude call, so a disabled chain
cannot spend on another provider.

## `ai test` model reporting (live)

Against a recording mock OpenAI endpoint:
`POST /api/ai/test {}` → `{"success":true,"message":"Test successful: OK",
"provider":"openai","model":"mock"}`. The model is no longer `null`; no key is
returned.

## OpenAI `max_tokens` / context clamp (live, recording mock)

Mock endpoint recorded every `/v1/chat/completions` request body.

- With `OXO_FLOW_AI_MAX_TOKENS=8192` and `OXO_FLOW_AI_CONTEXT_TOKENS=32768`:
  body keys `["max_tokens","messages","model"]`, `max_tokens: 8192` (input
  small, no clamp).
- With both env vars unset: body keys `["messages","model"]` — no `max_tokens`,
  preserving the historical endpoint-default behavior.

This confirms the opt-in budget is honored on the OpenAI-compatible path and
does not change the unset contract.

## OpenAPI AI request bodies (live, head f541b006)

`GET /api/openapi.json` (96 paths / 111 operations):
`/api/ai/translate`, `/api/ai/translate/stream`, `/api/chat/send/json`,
`/api/chat/send` declare `application/json` request bodies. `/api/ai/test`
still does not (the handler requires a body in practice) — queued in the
web/API audit fix.

## Proxy/model capability notes (2026-11-25 baseline)

- `<proxy-1>` `GET /v1/models`: GLM, DeepSeek, Qwen (OpenAI-compatible only);
  a GLM request was served with response `model:"DeepSeek"` (proxy routing).
- `<proxy-2>`: qwen3.8-27b / qwen3.8-27b-nothink / qwen3.8-flash-next /
  qwen3.8-flash-next-nothink; both OpenAI and Anthropic surfaces work.
- All five models accepted `max_tokens=32000` on a short prompt.
- Claude backend via `<proxy-2>`: `ai test` passed 3/3 with the default 300 s
  timeout; a 120 s timeout failed the analysis stage (expected calibration).

## Honest limits

One host, one operator proxy, one seed of live requests; the proxy may route a
model name to a different backing model; no paid calls are made in CI. The
deterministic tests (provider body, clamp, fallback attribution, disabled mode)
are the CI gates; this matrix is recorded operational evidence.
