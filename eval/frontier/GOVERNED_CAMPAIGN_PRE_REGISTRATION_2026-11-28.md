# Pre-registration — governed three-arm AI-synthesis ablation (issue #1395)

**Status: FROZEN before the first campaign generation.** This is a
research-harness pre-registration, not an independent reviewer sign-off
and not a governed Gate-3 pass (the #1163–#1166 pre-registration and its
PR-05 signatures are a separate process). No publication or peer-review
claim follows from it.

- **Freeze date:** 2026-11-28 (Asia/Shanghai)
- **Harness commit:** `f4b6c9793bf2f7c5932facef549efef2997baab6`
  (the commit that added the provider-native protocol adaptation and the
  exact McNemar analysis; parent `e7170492`)
- **Intent fixture:** `eval/frontier/intents.json`,
  SHA-256 `76136532e308464bed29827e001b6d894840c5d2e7bdd8925f24361bde75bd51`
- **Campaign lead:** single maintainer (no independent reviewer available)
- **Protocol-check exposure at freeze:** pre-freeze provider liveness
  probes only (a "reply OK" call and a `GET /models` listing). No
  campaign intent was generated and no primary outcome was examined.

## 1. Question and relationship to prior work

Issue #1315 built the instrument (`eval/frontier/run_eval.py`,
`statistical_report.py`) and the three arm definitions (plan:109–111).
Issue #1395 asks for the governed execution. A non-governed operational
run on 2026-11-26 used the same 36 intents once (Arm A 0/36, Arm B
28/36, Arm C 18/36; B vs C 12 vs 2 discordant, exact p=0.0129) and is
recorded in `docs/superpowers/audits/ai-synthesis-ablation-2026-11-26.md`.

This campaign executes the same three-arm design on the **same 36-intent
set**. The intents are **NOT held out** — they were used by the prior
run. The campaign is therefore a **pre-registered execution/confirmation
on a development intent set**, not a held-out confirmatory campaign. No
leakage-controlled or publication claim is made.

The frozen questions are:

1. Do all three arms actually execute their distinct code paths on the
   measured binary (Arm C passes `--ai-contract-feedback`)?
2. What is the paired success difference between the arms on the
   development intent set under the frozen provider, model, seeds, and
   deterministic gates?

## 2. Arms and exact toggles

| Arm | Harness variant | Path | Exact generation command |
| :--- | :--- | :--- | :--- |
| A `unconstrained` | `unconstrained` | Direct single-shot LLM call with the bare 7-rule system prompt (`NAIVE_SYSTEM`, unchanged) | protocol-native HTTP call; OpenAI chat-completions when the resolved provider is OpenAI-compatible |
| B `syntax-only` | `syntax-only` | `oxo-flow template --ai` (PipelineGenAgent + tools + syntax/structural feedback) | `[binary, "template", "--ai", "<intent>", "-o", "<run_dir>/", "--no-color"]` |
| C `full-contract` | `full-contract` | Same `template --ai` path plus the engine contract-feedback loop | Arm B argv **plus** `--ai-contract-feedback` |

Frozen invariants:

- `--profile compact` for every arm. `--profile full` is the Scientist
  Team product axis, **not** Arm C.
- `--ai-attempts 1` (the default) and no `--ai-max-retries` override; the
  binary's shipped default tool-loop budget applies.
- The only Arm B/Arm C difference is the `--ai-contract-feedback` flag;
  `eval/frontier/test_statistical_report.py` and
  `eval/frontier/test_run_eval_provider.py` pin this by argv equality.
- Each engine run writes `argv.json` beside its artifacts, so the
  executed toggle is direct evidence, never a label.

## 3. Intent set and judging

- 36 intents: 12 domains, 5 easy / 14 medium / 17 hard, 3 deliberately
  vague. The three vague intents are also counted in their base tier by
  the harness and are reported on their own row.
- All 36 are included; no intent-level filtering (`--filter` is empty).
- Judging is **only** the three deterministic gates — `validate --json`,
  `dry-run --json`, `lint --json`, each exit 0 — plus the harness's
  deterministic named-tool fidelity substring check. Success = all three
  gates pass AND fidelity passes. There is no LLM judge.
- Structured gate output (`.json`/`.err`) is retained per cell.

## 4. Provider, model, temperature, and protocol adaptation

- **Credential source:** `--creds-from config` — the operator's
  `~/.oxo-flow/ai_config.json` (`provider = openai`, an
  OpenAI-compatible endpoint). The endpoint host, URL, and API key are
  never printed or committed. The campaign environment exports only
  non-secret budget variables.
- **Model:** the operator-configured alias `DeepSeek`. The pre-freeze
  liveness probe returned `model: "DeepSeek"`; the provider does not
  reveal a backend version, so **the alias-to-backend mapping is opaque
  and is a frozen limitation**. Each run records the endpoint-reported
  `reported_model`; a different reported identity is recorded, never
  substituted.
- **Arm A protocol adaptation (frozen):** the bare prompt is unchanged,
  but its wire protocol follows the resolved provider. For this
  OpenAI-compatible provider, Arm A uses an OpenAI chat-completions
  request at `api_url` + `/chat/completions` (the same URL normalization
  as the CLI's OpenAI backend), `max_tokens = 8192` (the harness default,
  unchanged), `temperature = 0.0`. `--arm-a-protocol auto` resolves to
  `openai`; the adaptation is implemented in harness commit
  `f4b6c979`.
- **Arms B/C provider pinning:** the child environment sets
  `OXO_FLOW_AI_PROVIDER=openai` and the generic
  `OXO_FLOW_AI_API_KEY` / `OXO_FLOW_AI_API_URL` / `OXO_FLOW_AI_MODEL`
  trio from the resolved credentials. Every ambient provider credential
  (`ANTHROPIC_*`, `OPENAI_*`, `DEEPSEEK_*`, `OLLAMA_*`, and the generic
  `OXO_FLOW_AI_*` variables) is scrubbed first, so the host shell's
  unrelated agent token cannot be spent.
- **Temperature:** Arm A is pinned to 0.0 (where the provider honors it).
  Arms B/C inherit the engine's `[ai] temperature`, which is unset
  (provider default). This asymmetry is frozen and reported as a
  limitation; the harness does not silently pin B/C.
- **Budget:** campaign environment sets
  `OXO_FLOW_AI_MAX_TOKENS=16384`, `OXO_FLOW_AI_TIMEOUT_SECS=600`,
  `OXO_FLOW_AI_CONTEXT_TOKENS` unset. Harness flags: `--timeout 900`,
  `--sleep 1`. The endpoint is a thinking-style backend whose reasoning
  tokens count against `max_tokens` (observed in the liveness probe).
- **Cost:** derived from the harness price table; the alias falls to the
  `"*" = 0.28 / 1.10 USD per Mtok` entry. This is a static estimate, not
  a billing receipt.

## 5. Seeds, run order, and size

- **Seeds 1, 2, 3** (`--seeds 3`): every `(intent, arm, seed)` cell is
  attempted exactly once. Primary N = 36 × 3 = **108 cells per arm**
  (324 total).
- **Run order:** the harness's serial arm-major order (model → arm →
  intent → seed), no randomization. The order is part of the frozen
  design; any provider drift between arms is a limitation, not a
  post-hoc adjustment.
- **One host, one process, one provider.** No parallel campaign
  processes.

## 6. Exclusions, stopping, and missing data

- **Primary analysis is intention-to-treat over all attempted cells.** A
  cell that fails to produce a workflow because of provider
  transport/auth/timeout is a **failure** in the primary analysis; it is
  never silently dropped.
- **Per-protocol sensitivity** (secondary) excludes a cell only when it
  is mechanically classified as an infrastructure failure:
  `generated = false`, `input_tokens = 0`, `output_tokens = 0`, and the
  recorded error is a transport/timeout/HTTP-status string. Content
  failures (no extractable TOML, gate failures, fidelity misses) are
  never excluded.
- **No imputation.** No cell-level rerun is added to the primary
  dataset. If the campaign process is interrupted, it is restarted from
  scratch into a fresh results directory; partial datasets are never
  combined. Both attempts are recorded.
- **BLOCKED rule:** if no working provider/credentials exist (no
  successful generation in the pilot), the campaign stops and reports
  the exact error; no result is fabricated.
- No early stopping: all 324 cells are attempted regardless of interim
  results.

## 7. Outcomes and paired analysis

- **Primary outcome:** per-arm pass@1 over all attempted cells (success
  as defined in §3), N = 108 per arm.
- **Primary paired test:** exact two-sided McNemar (binomial sign test,
  p = 0.5) on matched `(intent, seed)` cells for **Arm B vs Arm C**, at
  alpha = 0.05, with Holm step-down correction over the fixed
  three-contrast family {B vs A, C vs B, C vs A}. The exact p-value is
  the pre-registered test; the asymptotic continuity-corrected
  chi-square is reported for continuity only.
- **Secondary analyses:** A vs B and A vs C within the same Holm family;
  per-tier and vague descriptive pass rates; marginal 95% percentile
  bootstrap CIs (2000 resamples); and a **seed-clustered intent-level
  sensitivity** analysis that collapses each intent to one outcome
  (`all` seeds pass / `any` seed passes / `majority` pass) and repeats
  the exact test at N = 36.
- **Direction:** two-sided. The prior non-governed run suggested C below
  B, but this campaign can support either direction or no difference; no
  directional hypothesis is privileged.
- **Reporting rule:** every arm and every contrast is reported regardless
  of direction, with N, seeds, provider/model, exclusions, and
  limitations. No POSITIVE/NEGATIVE publication decision and no
  peer-review claim are made.

## 8. Commands and data handling

Frozen campaign command (run from the repository root, after building
the pinned binary):

```bash
export OXO_FLOW_AI_MAX_TOKENS=16384
export OXO_FLOW_AI_TIMEOUT_SECS=600
python3 eval/frontier/run_eval.py \
  --creds-from config \
  --arm-a-protocol auto \
  --variants unconstrained,syntax-only,full-contract \
  --seeds 3 \
  --binary "$PWD/target/debug/oxo-flow" \
  --timeout 900 --sleep 1 \
  --campaign-note "governed pre-registered execution; intents not held out; R=3" \
  --out eval/frontier/results/governed-2026-11-28
```

- Raw artifacts (per-cell `workflow.oxoflow`, gate JSON/stderr,
  `cli.log`, `session.json`, `argv.json`, `result.json`; campaign
  `results.json` + `report.md`) live under `eval/frontier/results/`
  (gitignored).
- Committed evidence: a **sanitized** results manifest (absolute paths
  stripped, `binary` normalized to repo-relative, no API key/endpoint/
  proxy host) plus a compact per-cell manifest (generated-workflow
  SHA-256, gate exits, tokens, cost, wall time, error, provider,
  requested/reported model, argv) under
  `docs/superpowers/audits/data/`.
- Analysis command (same harness commit):

```bash
python3 eval/frontier/statistical_report.py \
  --results-dir eval/frontier/results/governed-2026-11-28 \
  --out-json docs/superpowers/audits/data/ai-synthesis-ablation-governed-2026-11-28-stats.json \
  --out-md docs/superpowers/audits/data/ai-synthesis-ablation-governed-2026-11-28-stats.md
```

## 9. Pilot (protocol check, not primary data)

After this note is committed, a **1-intent × 3-arm pilot** runs on
`qc-fastp-multiqc` into a separate results directory. It verifies that
Arm A receives content, Arms B/C execute and archive sessions, and Arm C
carries `--ai-contract-feedback`. The pilot is **excluded from the
primary dataset** and is reported only as a protocol check.

## 10. Amendments

- A **pre-data amendment** is permitted only if the protocol check
  reveals a hard provider constraint (for example, the alias cannot
  produce content at the frozen budget). The amendment records the exact
  failure, changes only the affected frozen parameter, and is made before
  any primary cell; outcome exposure is none. Any such amendment makes
  the affected analysis exploratory if outcomes were already seen.
- Any post-data change makes the affected analysis **exploratory**; it
  cannot be presented as pre-registered.
- No independent sign-off is claimed. This campaign is not the governed
  Gate-3 pass and does not resolve the #1163–#1166 pre-registration
  gaps.

## 11. Freeze record

| Item | Frozen value |
| :--- | :--- |
| Freeze date | 2026-11-28 |
| Harness commit | `f4b6c9793bf2f7c5932facef549efef2997baab6` |
| Intent fixture SHA-256 | `76136532e308464bed29827e001b6d894840c5d2e7bdd8925f24361bde75bd51` |
| Binary | `target/debug/oxo-flow` built from the freeze base; the campaign records its SHA-256 in `results.json` |
| Arms | A `unconstrained`, B `syntax-only`, C `full-contract` |
| Toggle | Arm C alone passes `--ai-contract-feedback` |
| Seeds | 1, 2, 3 |
| Cells | 36 × 3 × 3 = 324 |
| Primary test | Exact two-sided McNemar, B vs C, Holm over 3 contrasts, alpha 0.05 |
| Provider | operator `~/.oxo-flow/ai_config.json`, OpenAI-compatible, alias `DeepSeek` (backend opaque) |
| Temperature | A = 0.0; B/C = provider default (unpinned) |
| Exclusions | primary ITT; per-protocol infrastructure sensitivity only |

Signed: campaign maintainer (single operator). Independent review: **not
available / not claimed**.
