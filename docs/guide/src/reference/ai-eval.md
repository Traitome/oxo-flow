# Evaluate the AI Features

oxo-flow's AI surfaces (tool lookup, `template --ai` workflow generation,
`ai explain`, `--ai-recover`) are evaluated with a self-contained
benchmark in the `eval/` directory of the repository. The judge is
oxo-flow's own fresh knowledge base plus its own validators
(`validate`, `lint`) — a closed loop an external MCQ benchmark cannot
provide (see [issue #167](https://github.com/Traitome/oxo-flow/issues/167)
for the design rationale).

> Terminology: **pass@1 / pass@k**, **fidelity**, and **seed** are defined
> in the [Glossary](glossary.md#benchmark-terms); diagnostic codes in
> gate verdicts are listed under
> [Diagnostic Codes](glossary.md#diagnostic-codes).

## Three layers

| Layer | What is evaluated | Example metric |
|---|---|---|
| **Tool** | Knowledge grounding: does the AI name the right tool, the right version, and refuse fake tools? | `name_match`, `version_match`, `no_hallucination` |
| **Rule** | Single-step generation: right tool, pinned version that exists in the knowledge base, key parameters, normalized I/O declarations, explicit resources | `tool_present`, `version_pinned`, `io_declared`, `resources_in_range` |
| **Workflow** | End-to-end generation: structural validity, step/tool coverage, DAG edges, final outputs across repeated trials | `validate_pass`, `lint_pass`, `step_coverage`, `edge_coverage` |

## The loop

```bash
# 1. Capture AI outputs for the approved gold rows
python3 eval/scripts/capture_tool.py --out outputs/tool_answers.csv --trials 5
python3 eval/scripts/capture_workflow.py workflow --out outputs/workflows --oxo-flow target/debug/oxo-flow --trials 5

# 2. Judge them
python3 eval/scripts/runner.py tool --captures outputs/tool_answers.csv
python3 eval/scripts/runner.py workflow --captures outputs/workflows --oxo-flow target/debug/oxo-flow
```

Results land in `eval/results.csv`; each row is one judged trial with per-metric scores and an `overall` mean. The runner also writes `eval/results.items.csv` (per-item aggregates with `pass@k`) and `eval/results.summary.json` (dataset-level summary, breakdowns, and capture manifest).

Capture uses the same provider resolution order as oxo-flow itself: `OXO_FLOW_AI_PROVIDER`, generic `OXO_FLOW_AI_*` overrides, provider-specific env vars (Anthropic/OpenAI/DeepSeek/Ollama), then `~/.oxo-flow/ai_config.json`. This keeps benchmark captures aligned with the real CLI/web AI surfaces instead of assuming only one wire protocol.

## Gold set and human review

The gold answers live in `eval/gold/*.csv`. `tool.csv` is generated
deterministically from the embedded knowledge base
(`python3 eval/scripts/gen_tool_csv.py`); `workflow.csv` can be regenerated
with `python3 eval/scripts/build_workflow_gold.py` (defaulting to the in-repo
CSV path); `rule.csv` and `workflow.csv` are drafted from the gallery and the
[oxo-flow-community](https://github.com/oxo-flow-community) workflows.
Every row carries a `provenance_url` pointing at its primary source.

Rows start at `review_status = draft` and are **skipped** by capture and runner until a human reviewer approves them — the benchmark only runs on human-verified gold. If no approved rows exist, the harness now fails fast instead of silently emitting an empty report. See `eval/schema.md` for the full column contracts and the publication-track review workflow.

The deterministic grounding half of the tool layer is additionally
guarded in CI by `crates/oxo-flow-ai/tests/knowledge_grounding.rs` —
no API key required, runs on every push.

Capture manifests record git SHA, gold/knowledge hashes, provider/model identity, sampling settings, timestamps, and per-trial metadata so later analysis can be audited and reproduced.

## Frontier benchmark (cross-path quality/cost)

`eval/frontier/` is a complementary, self-contained benchmark that measures
the **quality/cost frontier across generation paths** rather than
gold-set coverage on a single path. It runs a tiered intent set (easy /
medium / hard) through two variants — the full CLI path
(`template --ai`: rich prompt, knowledge tools, tool loop) and a faithful
replication of the weakest embedded prompt (no tools, single shot) — and
judges every artifact with the three deterministic static gates
(`validate`, `dry-run`, `lint`; exit code only, no LLM judge).

Per-run token usage comes from the AI session archives (CLI variant) or
the provider response (minimal variant), so each cell of the
{path × model} matrix reports gate pass@1 alongside tokens, estimated
cost, and wall time. This is the evidence base for the unified
generation harness (see [Architecture → AI Subsystem](architecture.md))
and for sizing its cost tiers.

```bash
python3 eval/frontier/run_eval.py --variants minimal,cli
```

See `eval/frontier/README.md` for the full method, price-table
configuration, and known limitations.
