---
name: oxo-flow-scientific-analysis
description: >-
  Run reproducible bioinformatics analyses with the oxo-flow pipeline engine
  from natural-language tasks: plan workflows, author .oxoflow TOML projects,
  pass static gates (validate/dry-run/lint), execute on local or cluster
  backends, monitor and correct failures via checkpoint resume, QC outputs
  independently, and deliver reports plus a reproducibility receipt.
  Use when the user asks to analyze sequencing data with oxo-flow, design or
  fix an oxo-flow workflow, run/validate/debug oxo-flow, or produce a
  reproducible analysis report or receipt.
---

# oxo-flow Scientific Analysis

oxo-flow is a Rust-native bioinformatics workflow engine: DAG execution, 8
environment backends, sample wildcards, checkpoint/resume, provenance.
This skill is a compressed operating manual for agents — read it in full once
per task; follow the links for details on demand.

| Canonical entry points | URL |
|---|---|
| Documentation | https://traitome.github.io/oxo-flow/ |
| **This skill (site)** | https://traitome.github.io/oxo-flow/SKILL/ |
| This skill (raw) | https://raw.githubusercontent.com/Traitome/oxo-flow/main/docs/guide/src/SKILL.md |
| Repository (dev context: AGENTS.md) | https://github.com/Traitome/oxo-flow |
| Workflow JSON Schema | https://raw.githubusercontent.com/Traitome/oxo-flow/main/docs/schema/oxoflow-v1.schema.json |
| Community template library | https://oxo-flow-community.github.io/ |
| Reference workflows | https://github.com/Traitome/oxo-flow/tree/main/examples/gallery |

**GitHub unreachable?** Prefix any GitHub URL with a mirror proxy
(`https://ghfast.top/<url>` or `https://gh-proxy.com/<url>`), or web-search for
a current proxy; ecosystem mirrors: see `how-to/china-mirrors`.

## The Analysis Loop

Seven stages, three approval gates. Every stage ends with a
**Done when** condition — do not proceed without it.

### 0. Data & Task Contract

- Restate the natural-language task as: scientific question, inputs,
  expected outputs, success criteria, compute budget.
- **Verify every input path on the filesystem** (`ls`/`stat`); never invent
  or assume paths.
- Done when: all inputs exist, formats identified, PI role approves.

### 1. Plan the Workflow Shape

- Prefer an existing base: the docs gallery (16 live-tested workflows),
  the community library, `examples/gallery`. Write new only when no base
  matches.
- Choose tools via the knowledge bases (see Link Index); containers must be
  fully qualified (`quay.io/...`) — there is no implicit registry fallback.
- Decide sample dimensions (`sample_pattern`, `[[pairs]]`, `[[sample_groups]]`),
  one of the 8 environment backends, and explicit `depends_on` between rules.
- Done when: rule sketch + tool list + chosen base approved by PI.

### 2. Author the .oxoflow Project

- `oxo-flow init` scaffolds the project; write the workflow TOML (+ scripts);
  keep it in git — versioning is reproducibility.
- **Mandatory static gates, in order, before any compute:**
  1. `oxo-flow validate --json <workflow.oxoflow>` — structure, missing inputs, capacity
  2. `oxo-flow dry-run --json <workflow.oxoflow>` — expansion preview, resource audit, `-j` suggestion
  3. `oxo-flow lint <workflow.oxoflow>` — best practices
- Fix and re-run until clean; `oxo-flow graph` to eyeball the DAG.
  DAG edges match exact strings only — dir/glob inputs form no edges, so
  declare `depends_on` explicitly.
- Optional AI assistance: `[ai] enabled = true` in the workflow activates
  AI validate/lint/debug/recover.
- Done when: all gates pass; engine-QC role sign-off.

### 3. Submit & Execute

- Local: `oxo-flow run -j <N> --provenance` (always `--provenance`).
- Cluster: `oxo-flow run --profile <NAME>` with a `[cluster]` block;
  manual escape hatch `oxo-flow cluster submit|status|cancel|logs`.
- Web: `POST /api/runs` or the web UI.
- Done when: run id + checkpoint path recorded.

### 4. Monitor & Correct

- `oxo-flow status` (or `cluster status`) to completion.
- On failure: read structured diagnostics, diagnose, fix, re-run the static
  gates, then resume — `oxo-flow resume` / `run --resume-failed`
  (checkpoint avoids recompute). Optional AI recovery:
  `run --ai-recover --ai-max-retries 3`.
- Retry budget: max 3 correction loops, then escalate to the human.
- Done when: all rules complete, evidenced by checkpoint state.

### 5. QC Review (independent)

- Outputs exist, non-empty, plausible; `oxo-flow provenance verify
  .oxo-flow/checkpoint.json` passes; spot-check logs for silent errors.
  (On releases ≤ 0.14, verify mis-resolves relative output paths and reports
  existing files as missing — fall back to direct checksum comparison against
  the checkpoint, or upgrade oxo-flow. Chunk intermediates deleted by a
  transform rule's `cleanup = true` are expected to be reported missing —
  they were cleaned by design, not lost.)
- Judge against the Stage 0 success criteria. Performed by an evaluator
  role — **never the author of the workflow**.
- Fail → back to Stage 2 with written findings.

### 6. Reproducible Delivery (Receipt)

- `oxo-flow report -f md -o REPORT.md --ci` (execution truth: real exit
  codes, checksums, expanded commands) + `-o report.json` copy.
- **Receipt bundle:** `REPORT.md` + checkpoint JSON + provenance-verify
  output + workflow git SHA + `--versions-yml versions.yml` (newer releases;
  skip if the flag is rejected) + input manifest.
- Optional archival: `oxo-flow export -f toml|docker|singularity`.
- Done when: PI signs off against the original task.

## Guardrails

- **Evidence first** — never claim success without checkpoint/report evidence.
- **Real paths only** — verify inputs exist before referencing them.
- **Static gates before compute** — every workflow edit re-runs
  validate → dry-run → lint.
- **Correct, don't restart** — prefer checkpoint resume unless inputs were
  invalidated.
- **Explicit edges** — exact-string `depends_on`; wildcard inputs form no DAG
  edges.
- **Version everything** — workflow in git; its SHA lands in checkpoint and
  report.
- **Budget & stop-loss** — retry caps, human escalation, resource limits from
  dry-run.
- **Don't guess tools** — search the knowledge bases; prefer live-tested
  gallery/community workflows.

## Scientist Team Mode

Yes — one skill, 3–4 roles. Roles are stage owners in the loop above; the
handoff artifact is each stage's output. Roles can be played sequentially by
one agent or by parallel subagents; the independence rule is binding either way.

| Role | Stages | Responsibility |
|---|---|---|
| **PI** (science lead) | 0, 1 gate; 6 sign-off | Task contract, scope veto, final acceptance |
| **Computational biologist** | 1–4 | Workflow design, tools, TOML, fixes |
| **oxo-flow engine & QC expert** | 2 gates, 4, 5 | Static gates, failure diagnosis, verification |
| **Independent evaluator** (optional 4th; a community expert doubles as tool-choice advisor in Stage 1) | 5–6 | Adversarial review vs. the scientific question |

Minimal configuration (3 roles): PI doubles as evaluator — but must be a
**separate agent instance** (independence rule: the author never verifies
their own work).

## Link Index

Progressive disclosure — open only what the current stage needs.

| Topic | Links |
|---|---|
| Start | `tutorials/quickstart`, `tutorials/first-workflow`, `how-to/create-workflow` |
| Execute | `how-to/run-on-cluster`, `commands/run`, `commands/cluster`, `commands/resume`, `commands/status` |
| QC | `commands/validate`, `commands/dry-run`, `commands/lint`, `commands/debug`, `commands/graph`, `how-to/troubleshooting` |
| Delivery | `how-to/generate-reports`, `commands/report`, `commands/provenance`, `commands/export`, `reference/versioning`, `reference/reporting-system` |
| Reference | `reference/architecture`, `gallery/` (16 workflows), `tutorials/custom-scripts`, `tutorials/environment-management` |
| Knowledge bases (raw) | `crates/oxo-flow-ai/src/knowledge/bioconda_tools.jsonl`, `skills_index.jsonl`, `nfcore_modules.jsonl`, `pipeline_graph.jsonl` |

Docs links resolve under `https://traitome.github.io/oxo-flow/<path>`; raw
file links under
`https://raw.githubusercontent.com/Traitome/oxo-flow/main/<path>`.
