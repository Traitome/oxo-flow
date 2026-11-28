# AI Generation Frontier Benchmark (issue #342 / proposal P6)

Measures the **quality/cost frontier** of the AI pipeline-generation paths
against the engine's three deterministic static gates. Success is judged
**only** by the gates — never by an LLM judge — so numbers are cheap and
the judging is deterministic; the `unconstrained` arm is pinned to
temperature 0 where the provider honors it but is not fully reproducible
across providers or runs. The `syntax-only` and `full-contract` arms run
the real binary and inherit the engine's `[ai] temperature` config
(unset = provider default), so the arms may sample at different
temperatures.

This complements the gold-set evaluation in [`eval/`](../README.md) and
[docs/guide/src/reference/ai-eval.md](https://traitome.github.io/oxo-flow/latest/reference/ai-eval/):
that system scores coverage against reviewed gold rows on a single
generation path; this one compares **paths and models** (quality per token
spent) on a tiered intent set, which is the evidence base for the #342
unification proposal and its TeamProfile tiers.

## Method

| | variants (pre-registered three-arm ablation, plan:109-111) |
|---|---|
| `unconstrained` | **Arm A** — direct LLM generation from a single-shot naive prompt (7 generic rules, no tools, no knowledge injection) — the unconstrained baseline |
| `syntax-only` | **Arm B** — `oxo-flow template --ai` (PipelineGenAgent + knowledge tools + tool-calling loop) with TOML/DAG syntax + structural feedback only; BioTypes (`TYPE-*`) and scientific-preflight (`SCI-*`) / deep-check (`D00*`) feedback OFF |
| `full-contract` | **Arm C** — same as Arm B plus the engine's BioTypes + scientific-preflight + deep-check contract feedback loop (`--ai-contract-feedback`) |

Arm B and Arm C go through the SAME `template --ai` path and differ by an
actual engine feedback toggle, not a label: `full-contract` passes
`--ai-contract-feedback`, `syntax-only` does not (the CLI default).

Gates (exit 0 = pass, machine-readable JSON captured per run):

1. `oxo-flow validate --json` — structure / DAG / wildcard syntax
2. `oxo-flow dry-run --json` — expansion preview, resource audit
3. `oxo-flow lint --json` — best practices

Token accounting: `syntax-only` and `full-contract` read the AI session archive
(`~/.oxo-flow/ai_sessions/*-template-*.json`); `unconstrained` reads the
provider response usage. Cost is derived from the price table in
`run_eval.py` (defaults mirror the repo constant in `oxo-flow-ai/src/types.rs`).

## Usage

```bash
# credentials: ANTHROPIC_BASE_URL / ANTHROPIC_AUTH_TOKEN / ANTHROPIC_MODEL
# from env, or from ~/.oxo-flow/ai_config.json (never from shell rc files;
# values are never printed)

python3 run_eval.py                          # all 36 intents x unconstrained,syntax-only,full-contract
python3 run_eval.py --filter rnaseq-star     # subset
python3 run_eval.py --models deepseek-chat   # model axis (quality/cost frontier)
python3 run_eval.py --variants unconstrained # cheapest sweep
python3 run_eval.py --seeds 2                # 2 seeds per cell (variance)
python3 run_eval.py --profile full           # Scientist Team roles (compact is default)
python3 run_eval.py --binary /path/oxo-flow  # pin the binary for a whole campaign
python3 run_eval.py --creds-from config      # spend only the operator's oxo-flow provider
python3 run_eval.py --arm-a-protocol openai  # bare Arm A prompt over OpenAI chat-completions
```

### Provider identity and protocol adaptation (issue #1395)

Arm A's prompt is fixed, but its wire protocol follows the resolved
provider: `claude` uses the Anthropic Messages protocol, while
`openai` / `deepseek` / `ollama` use an OpenAI-compatible
chat-completions call (`--arm-a-protocol` can pin either explicitly).
Arms B/C run the real CLI under a child environment that pins
`OXO_FLOW_AI_PROVIDER` plus the generic `OXO_FLOW_AI_API_KEY` /
`OXO_FLOW_AI_API_URL` / `OXO_FLOW_AI_MODEL` trio from the resolved
source; every ambient provider credential (`ANTHROPIC_*`, `OPENAI_*`,
`DEEPSEEK_*`, `OLLAMA_*`) is scrubbed first, so a host shell's unrelated
agent token is never spent. `--creds-from config` makes the operator's
`~/.oxo-flow/ai_config.json` authoritative; `auto` keeps the historical
env-then-config precedence.

Each engine run writes `argv.json`, and `results.json` records the
provider kind, requested and reported model (an endpoint alias that
routes to a different backend model is visible, never substituted), the
per-run argv, and content hashes of the binary, harness, and intent
file. `statistical_report.py` reports the frozen instrument's primary
(continuity-corrected McNemar + Holm) and, as a sensitivity, the exact
two-sided McNemar test, plus a seed-clustered intent-level sensitivity
table.

> Correction (2026-11-28): the frozen instrument's PRIMARY significance
> path is the continuity-corrected chi-square (Edwards) with Holm; the
> exact two-sided McNemar test is the #1395 SENSITIVITY. The renderer
> labels them accordingly. The pre-registration text named exact as
> primary; both are reported for every contrast.

### Governed execution (2026-11-28, issue #1395)

The first pre-registered three-seed execution (36 intents × 3 arms ×
3 seeds = 324 cells) is reported in
[`docs/superpowers/audits/ai-synthesis-ablation-governed-2026-11-28.md`](../../docs/superpowers/audits/ai-synthesis-ablation-governed-2026-11-28.md).
Two operational lessons are pinned there:

- The C arm lost 59/108 cells to one contiguous provider transport
  outage. The harness records only `error: "template exited 1"`; the real
  `error sending request` cause is in `session.json` / `cli.log`. Until
  the harness classifies provider transport failures separately, treat
  an exit 1 as opaque and inspect the session archive.
- Because the arms ran in serial arm-major blocks, the outage correlated
  with the C block. The intention-to-treat B > C result is therefore
  **not** a contract-feedback effect: the per-protocol matched subset is
  null (B 47/48 vs C 48/48). Prefer interleaved or randomized run orders
  for any future campaign.

The intent set (`intents.json`) spans 12 domains across three complexity
tiers (5 easy / 14 medium / 17 hard), plus three deliberately
under-specified intents (`"vague": true`) used to measure how well a
profile standardizes ambiguous requests. Vague cells are reported
on their own row and are never compared against the specified tiers. The
counts in this paragraph are gated against `intents.json` by
`test_intents_contract.py`.

Thinking-style backends (e.g. DeepSeek behind an Anthropic-compatible
endpoint) truncate before the TOML is written if the output budget is too
small — reasoning blocks consume it. The default is 16384 (raised from the
old 4096, which thinking backends consumed entirely); raise it further only
for extreme reasoning lengths.

Results land in `results/<stamp>/` (gitignored): one directory per run
(generated workflow, gate JSON/stderr, CLI log, session snapshot), plus
`results.json` (machine-readable) and `report.md` (aggregated pass@1,
tokens, cost, latency).

## Interpreting

- `all-gates pass@1` is the headline quality metric (deterministic).
- `unconstrained` vs `syntax-only` vs `full-contract` is the pre-registered
  matched three-arm ablation (plan:109-111). Arm B vs Arm A isolates the
  rich prompt + tool loop over a bare prompt (the #342 prompt/tool-loop
  axis); Arm C vs Arm B isolates the engine contract feedback loop
  (BioTypes + scientific-preflight + deep-check). (Historical note: the
  unconstrained arm was named `minimal` and described as the web chat
  prompt before #948 P2-4; since #342 every product surface, web chat
  included, runs the unified `PipelineGenAgent`, so no product path uses
  the naive prompt anymore — the rows in PILOT_FINDINGS.md labelled
  `minimal` are this arm under its old name, and the old `cli` label is
  the `syntax-only` arm.)
- The `model` axis gives a quality/cost frontier for the `TeamProfile`
  tiers (Full vs Compact) proposed in #342.
- Gate failures carry structured error codes (e.g. `E017` unknown key),
  which classify failure modes without any LLM involvement.

## Known limitations

- Intent set is 36 (3 of them deliberately vague); still small enough that
  single-seed absolute numbers wobble — quote the multi-seed aggregates.
- Judging is deterministic (the three gates + substring fidelity), but
  only the unconstrained arm is pinned to temperature 0 — the
  `syntax-only`/`full-contract` arms inherit the engine's `[ai]
  temperature` (unset = provider default), and an Anthropic-compatible
  endpoint may still sample non-deterministically, so identical reruns can
  produce different workflows. Unconstrained numbers in PILOT_FINDINGS.md
  were drawn at provider default temperature (pre-pinning), so they are
  not strictly comparable to post-pinning runs.
- The resolved model may be an endpoint ALIAS: the provider can route
  `DeepSeek` (or any other alias) to a different backend model. The
  reported identity is recorded per run (`reported_model`), and cost is a
  static price-table estimate, not a billing receipt.
- The harness runs cells serially in a fixed arm-major order (model →
  arm → intent → seed). A time-varying provider or a warm-cache effect can
  therefore confound arms; a governed campaign should record the order and
  treat run-order randomization as an unmeasured limitation.
- Since the #342 unification, `--ai-max-retries` sizes the orchestrator's
  combined tool + correction-round budget, and engine validation feeds the
  loop; numbers above the unification marker in PILOT_FINDINGS.md were
  produced by the pre-unification paths.
- `--profile full` measures the Scientist Team roles (contract, curator,
  review); each role's marginal value is only interpretable against the
  compact profile on the SAME model, seeds, and ceilings. It is a product
  feature, NOT an ablation arm — it does not select Arm C. The Arm B / Arm
  C split is the `--ai-contract-feedback` engine toggle, independent of
  `--profile`.

## Fault-injection repair benchmark (`fault_injection.py`, issue #1164)

A sibling harness measures the fault pipeline end to end — not just static
detection — across 10 seeded fault cases (4 BioTypes contract violations,
3 scientific-preflight guardrail violations, 3 infrastructure resource
faults). It quantifies six independent stages per case:

1. Pre-execution static detection (`lint --json` / `dry-run` text).
2. Diagnostic attribution to the affected rule.
3. Repair-proposal availability.
4. Repair application.
5. Repair verification.
6. Successful workflow execution after repair.

### The executable-repair arm

Stages 4-6 are MEASURED, not declared. For every case the harness writes the
seeded workflow plus an `[ai]`-enabled copy (`auto_fix = "always"`), runs the
REAL CLI:

```bash
oxo-flow run workflow_ai.oxoflow --ai-recover --ai-max-retries 1
```

then parses the engine's repair evidence — the `REPAIR_APPLIED` /
`REPAIR_VERIFIED` narration prefixes and the checkpoint's
`repair_record.applied` / `.verified` when present, with a documented legacy
fallback to the pre-#1164 narration (`✓ Fix applied to …`, `AI fix not
auto-applied: …`) — and finally re-runs the (possibly patched) workflow.
Every false stage carries the exact WHY (which gate rejected the patch, the
provider error, or the rerun exit code).

The AI provider is read from the environment, never hardcoded:
`OXO_FLOW_AI_PROVIDER` + `OPENAI_BASE_URL` / `OPENAI_API_KEY` /
`OPENAI_MODEL` / `OXO_FLOW_AI_TIMEOUT_SECS` (or the Anthropic/DeepSeek
equivalents). Without a provider the arm runs in DRY mode: the provider is
pinned to `disabled` in the child environment (an explicit `disabled`
always wins, so a host's saved `~/.oxo-flow/ai_config.json` cannot fire a
live call), and stages 4-6 report false with reason "provider not
configured" — never fabricated. Dry mode is deterministic: repeated runs
produce byte-identical measured fields.

### Stub tool design

The repaired workflow must be able to genuinely re-run to success on the
measurement host WITHOUT real bioinformatics tools. The harness materializes
a `stubbin/` directory prepended to `PATH` (rule shells receive `PATH` via
the engine's allowlist) whose scripts (`samtools`, `gatk`, `bcftools`,
`bwa`, `vep`, `tabix`, `featureCounts`, …) exit 0, print plausible stdout,
and CREATE every file a real invocation would write — required because the
engine validates declared outputs post-rule. Detection is unaffected: the
TYPE-*/SCI-* checks pattern-match the workflow TOML text and command
strings, not the binaries.

Infrastructure faults use a `simtool` fault simulator whose failure is
PARAMETERIZED by a declared `[config]` key (e.g. `simtool memhog
--need-mb 8192 --have-mb {config.mem_mb}`): the seeded allocation (2048)
is below the workload's stated requirement (8192), so the rule fails with
the exact exit code / stderr the diagnostics engine classifies (137 OOM,
152 SIGXCPU, ENOSPC), and a repair that raises the allocation genuinely
flips the rerun to success. The subcommand names deliberately avoid the
diagnostic code tokens so stage-1 static detection stays engine-grounded
(infrastructure faults are runtime-only and correctly measure
`static_detected = false` there; the gate checks their runtime attribution
instead).

### What the measured counts mean — and what they do NOT

- `executability_verified` proves each case ships a reference repair that
  runs to success with stubs. It says nothing about whether the ENGINE's
  own recovery produced that repair.
- `repair_applied` / `repair_verified` are evidence-traceable (narration
  prefix and/or checkpoint field, recorded per case in `applied_evidence`).
- `workflow_success_on_rerun` is judged by rerun EXIT STATUS only — repair
  quality is NOT measured for biological correctness (stubs, not real
  tools, produce the outputs). A rerun that "succeeds" with a scientifically
  wrong patch still counts; this is a mechanical fix-and-rerun measure.
- Class limits are measured honestly: BioTypes faults abort at the type
  admission gate BEFORE any rule executes, and the rule-failure AI recovery
  path never engages for them; SCI preflight findings are advisory
  (warn, never abort), so the seeded workflow executes successfully and
  there is no failure for recovery to repair. The per-case
  `repair_stage_why` records exactly this.

### Usage

```bash
python3 eval/frontier/fault_injection.py --binary target/release/oxo-flow
python3 eval/frontier/fault_injection.py --skip-repair        # stages 1-3 only
python3 eval/frontier/fault_injection.py --out-json report.json
```

Raw per-case logs (seeded/patched/repaired workflows, lint JSON, fault-run
and rerun output, checkpoint) land in
`eval/frontier/results/fault_repair_logs/<fault_id>/` (gitignored); every
record references them, so every measured number traces to its evidence.
The exit gate fails on: a static-detection regression for the static fault
classes, a runtime-attribution regression for the infrastructure class, or
any case whose reference repair does not execute to success.
