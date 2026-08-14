# oxo-flow Web: Simulated-User Evaluation Design

Date: 2026-08-14 · Status: approved · Predecessor: [2026-08-14-web-full-lifecycle-design.md](./2026-08-14-web-full-lifecycle-design.md)

## 1. Goal

Comprehensively evaluate the oxo-flow web system (merged to main today,
P0–P4 complete) by **simulating 12 users of different skill levels and
bioinformatics domains**, across five dimensions:

| Dimension | Question it answers |
|-----------|---------------------|
| 可用性 (usability) | Can users complete their task? Onboarding, discoverability, error messages, dead ends |
| 好用 (UX quality) | How pleasant/efficient is it? Fluency, feedback, visual consistency |
| 实用 (practicality) | Does it solve real bioinformatics work? Tool/template coverage, AI grounding, would a real lab use it |
| 功能完整性 (completeness) | Claim check: every feature claimed in §2.3/§9b of the design doc actually works |
| 可靠性 (reliability) | Stability under concurrency, restart, error input; data consistency; no silent failures |

**Deliverable**: one comprehensive Chinese report
(`docs/evaluation/2026-08-14-web-evaluation.md`) + a private Artifact for
reading. No issue filing, no fixes, no committed regression suite (all
evaluation scripts live outside the repo). All evidence (finding cards,
screenshots, run logs) kept under `/tmp/oxo-eval/`.

## 2. Persona Matrix (12)

| # | Persona | Background | Core task (journey) |
|---|---------|-----------|---------------------|
| 1 | 新手·RNA-seq | Wet-lab PhD, never used a pipeline engine | Build bulk RNA-seq flow (fastqc→trim→align→quantify) from template/AI; validate; dry-run; read DAG; one tiny real run; read report |
| 2 | 新手·scRNA | Wet-lab MSc, no CLI | Build scRNA count-matrix flow, GUI+AI only; judge concept barrier |
| 3 | 新手·宏基因组 | Microbiome researcher, Excel only | Build 16S classification flow; judge terminology barrier |
| 4 | 中级·RNA-seq | Routine analyst, some CLI | AI-chat-generated flow → edit params → samples/targets → real run → pause/resume → diagnostics → report Q&A → export |
| 5 | 中级·WGS | Variant-calling analyst | Graphically build BWA→dedup→HaplotypeCaller (canvas, palette, AI grounding) |
| 6 | 中级·宏基因组 | 16S/metagenomics analyst | Multi-tool workflow; tool coverage & domain knowledge quality |
| 7 | 高级·RNA-seq | Bioinfo engineer | **CLI migration**: import CLI-authored TOML, web vs `oxo-flow validate --json` parity, dry-run consistency |
| 8 | 高级·scRNA | Bioinfo engineer | Complex DAG (scatter-gather), deep canvas editing, undo/redo, canvas ceiling |
| 9 | 高级·表观 | ChIP-seq engineer | Peak-calling flow, param tuning, rerun/invalidation semantics |
| 10 | 管理员 | Platform admin | Login, create users, AI provider config, audit log, pipeline ownership, template management, export |
| 11 | 运维管理员 | Deployment owner | Deployment mode, security headers/rate limiting, data persistence across restart, log observability |
| 12 | 可靠性专职 | Test engineer | Concurrency, restart during in-flight run, malformed input, SSE disconnect, state consistency |

## 3. Three-Layer Execution Architecture

**L0 — Environment.** Build frontend (`npm run build`), start
`cargo run -p oxo-flow-web -- --port 3000` (personal mode, fresh DB),
health-check. Personas isolate via distinct pipeline-name prefixes.
Evidence root: `/tmp/oxo-eval/`.

**L1 — Scripted coverage (objective).** Standalone Playwright scripts
(outside repo) covering: full feature-matrix walkthrough (each §9b claim
verified), error-path variants, 10-way concurrent create/validate,
CLI parity (`web validate ≡ oxo-flow validate --json` on the same TOML),
SSE reconnect. Restart-during-run scenarios run **last** (phase C) so they
don't disrupt in-flight persona work.

**L2 — Persona exploration (subjective).** Each persona = one subagent with
persona card + task + Playwright access. Agents drive the real browser
(write/run node scripts against `http://localhost:3000`), complete the
journey, then explore freely (deliberate mistakes, edge probing). Output:
structured finding card (JSON schema below) + screenshots to
`/tmp/oxo-eval/personas/pNN/`. Batches: A = personas 1–6, B = 7–11,
C = persona 12 (alone, may restart the server).

**L3 — Review panel (verdict).** Five independent reviewers: UX expert
(可用性/好用), senior bioinfo engineer (实用/完整性/CLI parity), reliability
engineer, product manager (priorities), devil's advocate (adversarially
verify every finding). Each outputs dimension scores + severity-rated issue
list; findings that fail adversarial verification are dropped.

### Finding-card schema (per persona, JSON)

```json
{
  "persona": "p04-中级-rnaseq",
  "journey_completed": true,
  "scores": {"可用性": 4, "好用": 3, "实用": 4, "功能完整性": 4, "可靠性": 4},
  "findings": [
    {
      "id": "p04-01",
      "severity": "P1|P2|P3|P4",
      "dimension": "可用性|好用|实用|功能完整性|可靠性",
      "title": "…",
      "steps": "exact repro steps",
      "evidence": ["screenshots/…", "logs/…"],
      "impact": "who is affected, how"
    }
  ],
  "summary": "narrative of the experience, ≤300 words"
}
```

### Scoring

Per persona per dimension: 1–5, evidence-backed. Report shows a
persona × dimension heatmap; level-weighted totals (L1 weights 可用性/好用,
L3 weights 完整性/实用); final dimension scores are the panel's verdict
after reviewing all cards + scripted evidence.

## 4. Reliability Scenario Catalog (L1 + persona 12)

1. 10-way concurrent pipeline create + validate + status reads
2. Server restart with in-flight run → run state, monitor, resume ability
3. Malformed TOML variants (bad syntax, unknown keys, broken refs, empty)
4. CLI parity: same TOML through web validate and `oxo-flow validate --json`
5. SSE disconnect/reconnect mid-run; event ordering on reconnect
6. Rapid pause/resume/cancel toggling; double-cancel idempotence
7. Data consistency: DB run status vs CLI checkpoint status; logs vs DB
8. Rate limiting + security headers presence (baseline from existing e2e)

## 5. Constraints & Risks

- **Live AI**: chat evaluated against the user's real Anthropic-compatible
  endpoint (env `ANTHROPIC_*`); per persona ≤3 live chat rounds
  (quota-tiny). Admin persona configures the provider via Settings UI
  (this itself is an evaluated feature). If quota dies mid-run, degrade
  and mark it honestly in the report.
- **Real runs**: tiny system-backend workflows only (sleep/echo; + fastqc
  if installed). conda/docker/mamba backends out of scope — stated as a
  limitation in the report.
- **Single machine**: all personas share one server; batches cap
  concurrency; persona 12 runs alone.
- **Duration**: ~half a day. AI-call cost is the main expense.

## 6. Execution Plan

1. L0: build frontend, start server, smoke test (`/api/health`, pages load)
2. L1: author + run scripted scenarios (feature matrix, error paths, parity,
   concurrency) → evidence JSON + pass/fail table
3. L2 batch A: personas 1–6 (parallel subagents)
4. L2 batch B: personas 7–11 (parallel subagents)
5. L2 batch C: persona 12 + restart-during-run scenarios
6. L3: review panel (5 parallel reviewers over all evidence)
7. Synthesis: Chinese report → `docs/evaluation/2026-08-14-web-evaluation.md`
   + Artifact; commit report; session memory update

Success criteria: every §9b claim verified or falsified with evidence;
12 finding cards collected; every report finding carries repro steps and
severity; no claim in the report lacks evidence.
