# Foundation remediation — measurement and benchmark audit synthesis (2026-11-28)

> **Status**: independent read-only audits of every measurement/comparison
> design in the repository, synthesized into the layered remediation tracked
> by [#1445](https://github.com/Traitome/oxo-flow/issues/1445). This document
> records the evidence baseline and the plan; it is not a claim that the
> remediation is complete. Each layer is implemented, tested (regression and
> negative controls), independently reviewed, and merged before the next
> layer starts. No layer is "done" without raw evidence and an honest
> residual list.

## 1. Scope and method

Two independent read-only audits (2026-11-28) covered:

- the research/measurement program: pre-registration governance (#1163–#1166),
  the AI synthesis ablation (#1395), fault-injection repair (#1164), import
  declared/execution equivalence (#1165/#1300/#1398), the BioType truth set,
  cross-architecture reproduction, gold set (#172), interpretation benchmark,
  claims gate, deployment/cloud campaigns;
- performance/benchmark designs: criterion micro-benchmarks, macro and
  comparative suites, `-j` suggestion, re-entry, resource ledger/accounting,
  `--acct` reconciliation, statistical/hardware harnesses, frontend frame
  budget, and user-facing performance numbers.

Both audits were produced by independent read-only agents on 2026-11-28
(repository at `550e2f64`; macOS host), under the controller's direction and
with the scope above. They re-ran the reproducible gates and recomputed the
headline statistics from committed evidence where possible. No files were
modified and no paid provider calls were made by the auditors.

## 2. Verdicts

### Research/measurement designs

| Layer / artifact | Verdict | Key evidence |
| :--- | :--- | :--- |
| PR-01…PR-06 governance protocol | **BROKEN (P0)** | `PR-05-reviewer-signoff.md` forbids confirmatory data before sign-off; `preregistration/README.md` and `AMENDMENTS.md` record #1163/#1166 as "confirmatory data" while PR-05 is PENDING |
| #1163 BioType governed truth set | WEAK | 0/83 FA, exact upper 4.3471% ≤ 0.05 is reproducible, but the author is the maintainer's delegated agent, 9/33 controls were pre-exposed, pilot-disjointness is a post-hoc judgment, and the 83 cases are clustered by TYPE/format while the CI assumes independence |
| #1166 cross-architecture governed campaign | WEAK | 24/24 artifacts identical; but 24 artifacts are pseudo-replicates of 2 workflows (workflow-level lower bound ≈ 0.158, class-level ≈ 0.631, both < τ = 0.80); the deterministic corpus has no subtle-difference positive control |
| AI synthesis three-arm ablation (#1395) | WEAK (baseline) | Prior operational run: A 0/36, B 28/36, C 18/36, one model/seed/proxy, no binary hash, raw workflows not committed; the governed campaign's reported counts (campaign-reported; the results artifact is pending in #1395) are B 92/108 vs C 49/108, with all 59 C failures being `template exited 1` |
| #1164 applied-repair campaign | WEAK | 6/13, 6/6 repairable, no CI, one run per case; raw logs are gitignored and the claims gate parses prose |
| #1165 import/execution equivalence | WEAK | Declared harness reproduces 8/0/19/0 over 27 fixtures, but the audit report headline is stale (4/14, 6/19); the execution triplet's as-run scripts/raw records are not preserved |
| Real-world import execution | WEAK (N = 2) | 2 matched, 0 drifted, 6 skipped with reasons; raw records committed; one tool family/one host |
| Gold set (`eval/`) | WEAK | 257 items hash-bound; reviewer is the owner (#172 open), `wf-004` passes vacuously, capture results not committed |
| `docs/claims.json` | WEAK | 17 claims/56 assertions pass, but they cover counts/maturity, not the FA CI, cross-arch, AI ablation, or import primary results; repair is present only as a prose-parsed headline with no CI/raw artifact |
| PostgreSQL ledger / two-instance campaign | SOUND (single host) | Live PG 16.15, N=16/32 exactly-one admission, non-vacuity faults, raw log, defects fixed |
| Kubernetes live / image render | SOUND (single cluster) | Real k3s, two VMs, raw transcripts, defects fixed |
| Tencent Batch live | WEAK | Named-env submit SUCCEED/poll/logs/cancel, single account/zone; no raw transcript, no shared-FS array |
| Cloud CLI static audit | SOUND (static only) | Vendor models/CLI sources compared; no live AWS/GCP and says so |
| userns/sandbox isolation (#1401) | SOUND (one host) | uid-1000 control-leak vs isolated denies, fail-closed, cancel teardown measured; raw key log committed |
| Scientific preflight catalog / metadata contracts | SOUND as regression harness | 51 rules with authority/fixtures; 18 metadata fixtures with exact tuples |
| Deployment/qualification docs | WEAK | Final 81/81 + 19/19 has no committed raw log; Tencent "blocked" line is stale |

### Performance/benchmark designs

| Design / claim | Verdict | Key evidence |
| :--- | :--- | :--- |
| Criterion regression tracking | OVERCLAIM | `make bench` only saves a baseline; no comparison, no CI step, baseline gitignored; `BENCHMARKS.md` inventory/counts inaccurate |
| `api_response_bench.rs` | BROKEN | Bench defines its own smaller response types; real fields missing; criterion reports a median (p50) but no p99, and the bench never asserts its stated 5 ms/50 ms goals |
| `diagnostics_bench.rs` | BROKEN | Reimplements a 17-pattern substring matcher, not the 31-pattern engine |
| Macro suite | WEAK | Debug binary by default, `--iterations 1`, no warm-up, no engine/git/binary identity, raw results gitignored |
| Comparative Nextflow/Snakemake | **BROKEN (P0)** | Snakemake runs reuse one output dir after hyperfine warm-up → measured runs are no-ops (~1.0 s startup); oxo-flow is dry-run while the others execute; reference numbers are 0.17.2 |
| `-j` suggestion | WEAK | One run per cell, old values hard-coded, no pinned old binary; unbounded control is a strength |
| Re-entry "283.3×" | OVERCLAIM | Cold executes 1609 rules, resume executes none; ratio scales with N (78.8× at N=50), N=1, debug binary; ROADMAP omits caveat |
| Range I/O staging | WEAK reproducibility | N=1, loopback, `rustfs:latest` (no digest), `os.urandom` object, raw JSON not committed; "resume" not byte-offset instrumented |
| Resource ledger core | SOUND design, **BROKEN edge cases (P0)** | Stale `resource_usage` after a pre-spawn failure; local CPU counts only the root process while RSS/I-O use the subtree; cost accrual inherits both |
| `--acct` reconciliation | WEAK/BROKEN | Prefers the batch row, but `MaxRSS` is only on step rows → memory reconciliation recorded as `skipped_metrics` (one side did not report), caused by the batch-row preference discarding step-row `MaxRSS`; retries are not attempt-matched |
| AI ablation statistics | WEAK/OVERCLAIM | Reports exact McNemar p = 0.0129 as primary; #1395 is not a PR-01 campaign, but the frozen method it adopts (PR-01 §0.5/§0.7) makes Edwards+Holm primary (p = 0.0162); A wall mean counts two null runs as 0 |
| Interpretation benchmark | WEAK | 12 single-run cells/provider, raw results gitignored; prompt already contains engine preflight findings |
| Statistical/hardware harnesses | WEAK | Missing token/cost becomes 0; zero denominators accepted; the hardware/projection harness has no declared baseline/control/paired analysis (the statistical report does implement paired McNemar+Holm) |
| Publication reports (cohort/hardware/statistical/SUS) | SOUND (labeling/gating only) | Explicit MODEL PROJECTION / SIMULATION ONLY / not-measurement labels; provenance and regen gates |
| User-facing unsourced numbers | OVERCLAIM | `cluster.md` 1.33× (commit message only), deploy-modes startup/memory table, README "fast/instant startup", GLM 145 s anecdote |

## 3. Cross-cutting findings

1. **Gate proxy validity is disclosed in the audits but leaks into user docs.**
   The three gates certify engine-acceptable structure, not scientific or
   execution correctness; fidelity is a lowercase substring check. The WGBS
   bwa-mem→Bismark counterexample (the committed basis is the intent text
   `eval/frontier/intents.json:213-219`, which already shows the fidelity
   blind spot; the campaign artifact is pending in #1395) demonstrates that a
   gate-passing workflow can be scientifically invalid.
2. **No campaign has an independent author or independent reanalysis.**
   PR-05 is PENDING for all four campaigns; engineering PR reviews are not
   scientific sign-off.
3. **Pseudo-replication is unaddressed.** #1166 treats 24 artifacts from 2
   workflows as independent; #1163 treats 83 author-generated, code-clustered
   cases as independent. The pre-registered units are protocol-valid, but the
   reports must show the clustering sensitivity and effective independent
   unit.
4. **The #1166 corpus is biased toward success.** Deterministic text/binary/
   gzip/float operations, one digest, no subtle-difference positive control.
5. **The AI ablation is operational, not attributable.** One model through a
   proxy that may alias the backing model; A vs B bundles prompt + tools + loop
   + feedback + temperature; only B vs C is a clean one-toggle comparison.
6. **Raw evidence retention is uneven.** Strong for #1163/#1166, import
   execution, PostgreSQL, Kubernetes, userns. Weak for repair (gitignored),
   AI ablation (no binary hash; raw workflows absent), interpretation,
   execution-equivalence as-run scripts, range I/O, re-entry, final deployment.
7. **The claims gate covers counts, not primary results.** Stale numbers
   survive in docs the gate does not read.
8. **No benchmark regression gate.** Criterion baselines are never compared;
   the comparative benchmark is invalid; macro is debug/single-run.
9. **Negative/inconclusive results are reported prominently** — a genuine
   strength to preserve.

## 4. Remediation layers (tracked in #1445)

Each layer: implement → regression/negative tests → raw evidence → independent
review → `make ci` (true exit status) → merge. The next layer does not start
until the current one is merged.

- **L0 Governance (P0)**: formalize PR-05 "governed pre-sign-off data
  collection" vs "confirmatory result"; relabel #1163/#1166 and all status
  ledgers. The amendment requires a dated `AMENDMENTS.md` entry and a
  `MANIFEST.sha256` recompute; no current gate verifies the six-artifact
  manifest. No confirmatory/publication vocabulary until sign-off.
- **L1 Measurement correctness (P0/P1)**: resource-ledger stale usage and CPU
  subtree accounting; `--acct` aggregation/attempt matching; comparative
  benchmark validity + raw JSON; criterion regression gate; real-type/real-
  engine API/diagnostics benches.
- **L2 Research evidence (P1)**: clustering/effective-unit sensitivities;
  #1166 positive control and broader corpus; biotype CI decision recomputation;
  claims registry for primary endpoints; raw evidence (repair, AI,
  interpretation, execution); re-entry/unsourced number corrections;
  non-vacuity modes.
- **L3 AI benchmark foundation**: intent v2 (FLAWED/AMBIGUOUS/GATE-UNFAIR/
  DUPLICATE, `requires`/text alignment, aliases, vague handling), judging
  semantics and negative controls, E017 `container`→`docker/singularity`
  suggestion, proprietary-tool knowledge, provider/binary identity and
  Edwards+Holm primary analysis.
- **L4 Arm C optimization and generalization**: diagnose C's `template exited
  1` failures from logs/session archives; one-variable-at-a-time matched-seed
  experiments on v2; held-out intents and multiple providers; report the
  reachable performance and reliability boundary honestly.
- **L5 Docs/claims and release readiness**: stale/unsourced numbers, cross-doc
  contradictions, final code/docs/performance/product audit. No tag is created
  by this epic; the release decision remains with the maintainer.

## 5. Acceptance

A layer is complete only when: regression and negative-control tests prove
the fix can fail; raw evidence is committed (sanitized, with binary/commit/
host/provider identity where applicable); an independent reviewer records
APPROVE; `make ci` passes with its true exit status; and the residual list is
updated. "Confirmatory" and "publication-ready" remain forbidden until PR-05
sign-off, regardless of a POSITIVE decision category.
