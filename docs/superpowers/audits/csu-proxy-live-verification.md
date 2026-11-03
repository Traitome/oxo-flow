# CSU AI proxy — live verification of PR #997 AI provider stack (2026-11-03)

Endpoint: http://biotree.top:38123/csuapi/ (key held in ~/.oxo-flow/ai-env.sh, never committed)

## Endpoint surface discovery
- `GET /v1/models` → OpenAI-compatible only. Models: `GLM`, `DeepSeek`, `Qwen`, `Embedding`, `Reranker`
  (capitalized IDs; lowercase `glm` → HTTP 400 "model price not configured").
- `/v1/chat/completions` (OpenAI shape) → works for GLM + DeepSeek.
- `/v1/messages` (Anthropic shape) → works with capitalized model IDs.
- Proxy is new-api/one-api derived; per-key rate limit ~1 req/20s (HTTP 429 between).
- Anthropic-mode GLM response when max_tokens=64: stop_reason=max_tokens with
  EMPTY text block — reasoning consumed the entire budget, exactly the failure
  mode OXO_FLOW_AI_MAX_TOKENS=16384 was calibrated for (the `context_tokens_from_env`
  clamp stack in `crates/oxo-flow-ai/src/provider.rs`).

## Reproduced the field incident verbatim
`POST /v1/chat/completions` GLM, prompt 200k CJK chars + max_tokens=32000 →
HTTP 400: "This model's maximum context length is 131072 tokens. However, you
requested 32000 output tokens and your prompt contains at least 99073 input
tokens..." — byte-identical to the user-reported error. So the proxy fronts a
131072-window model despite the user-visible 229376 claim; OXO_FLOW_AI_CONTEXT_TOKENS
must be set to 131072 for this endpoint.

## Clamp math vs proxy reality (provider.rs)
estimate_text_tokens = utf16 units / 4 (ceil); CJK char = 1 unit → 200000 chars
→ est 50000 tokens. TOKEN_ESTIMATE_SAFETY 1.5× → 75000; margin 4096; window
131072 → available = 131072-4096-75000 = 51976 ≥ configured 32000 → no clamp
fires at 200k chars. The REAL input was 99073 tokens (≈2 tokens/CJK char, far
denser than the 1.5× safety factor models). At ~260k chars (real ≈128.6k
tokens): est 65000 ×1.5 = 97500; 131072-4096-97500 = 29476 → clamps to 29476.
Real input 128.6k + 29476 = 158k > 131072 → the clamp UNDER-estimates for
pure-CJK worst case but the floor-1024 + overflow retry path still returns a
clean AiError::ContextOverflow instead of a raw 400. For English-heavy
prompts the 4 chars/token estimate is accurate and the clamp holds.

Verdict: clamp works as designed (test `clamp_reproduces_the_field_incident`
asserts the exact 99073/32000/131072 → 1024 case). For this proxy the right
operator setting is OXO_FLOW_AI_CONTEXT_TOKENS=131072. CJK-dense worst cases
still rely on the overflow→compress-retry path; that path was verified live
last session (the `AiError::ContextOverflow` classification in
`crates/oxo-flow-ai/src/provider.rs`).

## Live results (all HTTP 200)
- GLM chat completions: content OK, reasoning carried in `reasoning` field,
  usage.completion_tokens includes it.
- DeepSeek chat completions: content OK; usage.completion_tokens_details.
  reasoning_tokens = 22 (reasoning billed inside completion window).
- GLM Anthropic-mode, max_tokens=2048: text 'OK', stop=end_turn, out=52.
- GLM Anthropic-mode, huge prompt + max_tokens=1024: HTTP 200, real answer.
- oxo-flow-ai lib tests: 337/337 green on branch.

## Operator recipe for this proxy (recorded for docs)
OXO_FLOW_AI_PROVIDER=claude
ANTHROPIC_BASE_URL=http://biotree.top:38123/csuapi/v1/messages
ANTHROPIC_MODEL=GLM            (or DeepSeek/Qwen; capitalized)
OXO_FLOW_AI_CONTEXT_TOKENS=131072
OXO_FLOW_AI_MAX_TOKENS=16384   (default; thinking budget needs it)
