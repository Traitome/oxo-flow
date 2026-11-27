# Vision: Verifiable, AI-Native Scientific Workflows

> **Working thesis** — *Formal biological typing and autonomous self-healing for verifiable, AI-native scientific workflows.*
>
> oxo-flow's long-term scientific program: make bioinformatics pipelines machine-checkable artifacts that an AI can synthesize, an engine can verify before compute, and evidence systems can reproduce years later — with explicit, honest maturity labels for everything not yet shipped.

This page is the narrative source for the project's scientific direction. Every claim is anchored to something that exists today (a command, a diagnostic family, a test suite) or carries an explicit maturity label from [ROADMAP Phase 12](https://github.com/Traitome/oxo-flow/blob/main/ROADMAP.md). Nothing on this page should be read as peer-reviewed scientific evidence; the bioRxiv preprint cited at the end has not been peer reviewed.

---

## 1. The problem: script-driven pipelines meet AI-generated code

The dominant workflow engines were designed for humans writing code by hand. Snakemake embeds arbitrary Python; Nextflow wires data through Groovy channel operators (`.mix()`, `.combine()`, closures). That expressiveness is a genuine strength for human authors — and it becomes a liability when the author is a model:

- **Unknowable graphs.** In both systems the DAG is whatever the script computes at runtime. Nothing can fully verify dependencies, resource needs, or data contracts before the first job starts — so an LLM hallucination surfaces as a 3 a.m. failure on node 400, not as a rejected file.
- **Generation difficulty.** Channel-operator reasoning and Turing-complete embedded code give models room to invent deadlocks, races, and non-deterministic wiring that only manifest at runtime.
- **Opaque repair loops.** Runtime failures surface as deep stack traces; neither engine ships a structured, machine-readable failure taxonomy a repair agent can act on deterministically.

| Dimension | Nextflow (Groovy DSL2) | Snakemake (Python-embedded) | oxo-flow (declarative TOML + Rust) |
|---|---|---|---|
| **LLM generation difficulty** | High — closures and channel operators, runtime deadlocks possible | Medium–high — embedded Python, hidden side effects | Low — schema-validated key-value data, hard syntax boundary |
| **Static verification before compute** | Hard — graph is whatever Groovy evaluates at runtime | Hard — input/output sets require executing Python | Native — static DAG compiled and checked (`validate` → `dry-run` → `lint`) before any job starts |
| **Machine-readable failure contract** | Runtime stack traces | Python tracebacks | Stable `E`/`W`/`SCI-*`/`TYPE-*` codes, `--json` on every gate, structured `{code, message, detail, suggestion}` errors |

The table is an architectural comparison, not a benchmark; measured generation quality and cost live in the deterministic `eval/frontier/` harness.

The point is not that dynamic DSLs are bad — they remain unmatched for free-form programmatic logic, and the nf-core / Snakemake ecosystem's accumulated domain knowledge is a real moat we address separately (see innovation 3). The point is a deliberate bet: **for AI-authored, cohort-scale production pipelines, verifiability matters more than expressiveness**, and the ecosystem lacks an engine designed around that bet.

## 2. The path: from script-driven to semantic-driven

Three moves turn that bet into an architecture:

1. **Collapse the input side.** Humans (and agents) stop hand-maintaining pipeline code. Natural language and normalized protocols enter `oxo-flow template --ai` or the web copilot, run the shared `PipelineGenAgent` through an orchestrator, and the output must pass the engine's own schema/validate gates before it can ever execute. Generation is engine-validated, not trust-me.
2. **Standardize the intermediate representation.** A `.oxoflow` file is not configuration sugar — it is a deterministic, schema-gated intermediate representation of the pipeline: pure declarative data from which the static DAG, the environment plan, and the evidence record are all derived. One IR, four surfaces (CLI, Web Studio, VS Code IDE, desktop), zero shadow APIs.
3. **Raise the output side.** The engine, not the user, carries correctness and resilience: biological type contracts across DAG edges, curated scientific preflight rules, structured diagnostics, checkpoint/provenance evidence, and — as the research agenda matures — bounded autonomous repair.

## 3. Four methodological innovations

Each innovation states the claim, what ships today, and what evidence is still required. Maturity labels follow ROADMAP Phase 12.

### 3.1 BioTypes — biological semantics on DAG edges

**Claim.** Path-text matching is the wrong substrate for pipeline correctness. Files flowing between stages should carry biological contracts — format, sort order, index accompaniment, reference build, read groups, extensible attributes — that the compiler checks statically.

**Shipped today.** The optional `[types.*]` system: named contracts referenced by rules, checked in `oxo-flow validate` and in run preflight. Violations produce `TYPE-001`–`TYPE-007` diagnostics with remediation hints (plus advisory `TYPE-008`/`TYPE-009`/`TYPE-010` for unverifiable edges — a typed consumer fed by an untyped producer, a build-incomplete pair, or a typed pair whose channels do not line up); where the fix is mechanical the diagnostic synthesizes the command (e.g. `samtools sort` for a queryname-sorted BAM feeding a coordinate-indexed consumer; `samtools index` for a missing companion index), surfaced verbatim as the diagnostic's `coercion_rule` field and a labelled `coercion:` line (issue #1322). — *Implemented; broad truth-set validation outstanding.*

**Exit evidence.** False-positive analysis against a representative corpus of real pipelines; literature-backed rule review (ROADMAP Stage 12.2).

### 3.2 Autonomous self-healing runtime

**Claim.** Long genomic computations fail for four stacked reasons — system resources, data defects, algorithm parameters, biology-level surprises. A workflow engine should diagnose across those layers and apply *bounded* remediation, not just pattern-match stderr.

**Shipped today.** 31 domain diagnostic patterns with stable codes; curated scientific preflight (`SCI-*`: e.g. VQSR cohort-size, BQSR known-sites, Mutect2 tumor-only pitfalls) executed in `dry-run`; `--ai-recover` diagnosis with safety-gated repair proposals; a separate seeded-failure harness (`eval/interpretation/`) that scores `report --ai` interpretation across three providers. — *Diagnosis implemented; interpretation is measured; repair now runs a verified-repair loop (`REPAIR_RERUN_SUCCEEDED` on the run path), and ROADMAP Stage 12.3 records the measured 13-case campaign at 6/13 applied, 6/13 verified, and 6/13 rerun successfully — all 6 repairable seeded classes (the 3 original infra seeds plus cpu_limit, ulimit, and shared_library added by #1278); the other 7 cases are blocked upstream of the loop — stated plainly, not hidden.*

**Exit evidence.** Probabilistic fault-attribution model beyond deterministic patterns; applied-repair verified by successful reruns on a seeded fault-injection benchmark; pre-registered analysis.

### 3.3 Universal ecosystem transpiler

**Claim.** A new engine cannot ask the field to abandon nf-core modules and community Snakemake rules; it must ingest them — and the translation should itself be an analysis artifact (typed, reviewed, diffable), not a lossy copy-paste.

**Shipped today.** `oxo-flow import` translates a deterministic Nextflow DSL2 / Snakemake subset to `.oxoflow`; every construct outside the documented subset is a hard error that names the construct, and the generated workflow is parsed and statically validated before it is written. Coverage is prototype-grade — complex channel logic and embedded code require review plus the static gates (see [LIMITATIONS](https://github.com/Traitome/oxo-flow/blob/main/LIMITATIONS.md)). — *Prototype.*

**Exit evidence.** Representative-corpus coverage, semantic-equivalence tests, unsupported-construct reporting, round-trip review (ROADMAP Stage 12.3).

### 3.4 Cryptographic provenance

**Claim.** Reproduction requires evidence, not timestamp conventions. Run identity should be anchored on content hashes across outputs, configuration, and protected inputs.

**Shipped today.** `--provenance` records SHA-256 checksums of declared outputs into the checkpoint — on both execution paths: the local executor hashes at task completion and the cluster path hashes as each job settles. Configuration fingerprints, environment identities, and `protected_data_trees` drift audits ride alongside; `oxo-flow provenance verify --json` audits recorded checksums and is consumed by the web API (`GET /api/runs/{id}/provenance-verify`). Recorded checksums take precedence over `mtime`, which is demoted to a bounded fallback.

**Exit evidence.** Cross-architecture reproduction campaigns that distinguish floating-point drift from logic drift (x86 → ARM/RISC-V) — a measurement program, not a code feature. A first **governed native x86_64 ↔ aarch64** campaign now exists ([report](https://github.com/Traitome/oxo-flow/blob/main/docs/superpowers/audits/cross-arch-provenance-governed-2026-11-27.md)): two deterministic workflows, 24 declared artifacts in eight classes, one pinned multi-arch manifest-list digest, native `linux/amd64` and native `linux/arm64` (no qemu), and an excluded `uname -m` negative control. 24/24 artifacts were SHA-256 identical (100%, exact 95% CI [85.75%, 100%]); under the frozen rule the category is **POSITIVE**. The claim is bounded to this corpus and these tool versions; PR-05 independent sign-off, independent replication, RISC-V, other environment backends, and the environment-lock matrix with differing tool versions remain outstanding.

## 4. The empirical program

Methodological claims are nothing without measurement infrastructure:

- **Production deployments.** The cluster path is exercised by live production Slurm deployments as well as CI; crash-restart, cluster observability, dispatch-order, and CPU-oversubscription failure modes found there were fixed upstream with regression tests on `main` (#1083, #1085, #1089, #1080). This is operational evidence — it does not substitute for controlled benchmarks, and no measured throughput, cost data, or completed cohort-scale campaign exists yet. Deployment specifics are operational records and are not part of this repository.
- **Deterministic AI evaluations.** `eval/frontier/`, `eval/gold/`, and `eval/interpretation/` score generation and interpretation quality with deterministic checks and negative controls; provider-backed runs record git SHA, gold/knowledge hashes, and per-trial metadata in capture manifests so results are auditable. The CI gate runs the deterministic harness and gold-consistency checks, not paid provider calls.
- **Controlled benchmark campaigns.** The remaining exit evidence for publication-tier claims (matched arms, held-out intents, multiple seeds, run-level cost/failure data, human-factors study) is specified in ROADMAP Stage 12.4 — and none of it has run yet. Production deployments are the intended substrate for a future pre-registered scalability campaign.

## 5. Verification snapshot (2026-11-26)

The quantitative claims on this page were re-checked against `main@99bb8b56` on 2026-11-26, after the audit-chain merge `dedc1121` (which added the `audit` subcommand and its two endpoints) plus the metadata-contract and Kubernetes-image-render merges; the cluster `--provenance` behavior was re-verified after #1096 (`main@3406208f`); the mechanism-level claims were first checked against `main@55dddf3c` on 2026-11-17. The numeric claims that have a machine-readable source are enforced by `make claims-check` against [`docs/claims.json`](https://github.com/Traitome/oxo-flow/blob/main/docs/claims.json), so this snapshot cannot drift silently:

- 40 CLI subcommands; 96 OpenAPI paths / 111 operations served by `GET /api/openapi.json` (default build, non-HPC mode).
- 31 built-in diagnostic patterns; biological type contracts firing on a deliberately mismatched workflow (`TYPE-001`–`TYPE-003` observed live, `TYPE-004`–`TYPE-010` covered by unit tests; `TYPE-008`/`TYPE-009`/`TYPE-010` are advisory unverifiable-edge codes).
- 33 prebuilt cluster and re-entry integration tests passing on a Linux host.
- Live `[engine] dispatch_order` (local execution), run-aware `cluster status --json` / `cluster logs`, interrupted-rule re-queue, `provenance verify`, and the strict-placeholder gate exercised on a mock-Slurm Linux host.

A check that contradicted a documented claim was treated as a documentation defect and corrected on this page and in the command reference.

## 6. What we do not claim

The honesty charter, so the above can be trusted:

- No peer-reviewed claim is made; a bioRxiv preprint is cited on this page, but there is no journal acceptance, independent replication, or journal-ready artifact package (ROADMAP Stage 12.4: *Planned / blocked*).
- Type contracts detect declared state mismatches; they do not prove biological correctness.
- Provenance anchors reproducibility evidence; bitwise reproduction across all tools and hardware is not guaranteed.
- Repair proposals are experimental until independently verified by a successful rerun — in the enlarged 13-case seeded fault-injection campaign the loop closes 6/13 cases end-to-end (6/6 of the repairable classes: the 3 original infra faults plus 3 held-out classes added by #1278; the other 7 cases are blocked upstream of the loop — measured evidence in `eval/frontier/PILOT_FINDINGS.md`).
- PostgreSQL storage parity, cloud-batch executors, and tenant isolation carry explicit unproven/prototype labels (ROADMAP Stage 12.1). The tenant-isolation label is now dimensional, not wholesale: the environment allowlist and the CPU/memory/process rlimits are enforced on every web-spawned run (Linux rlimits where available), and — behind the opt-in kernel Landlock sandbox (`OXO_FLOW_WEB_SANDBOX=landlock`) on Linux 5.13+ — filesystem isolation is kernel-enforced (TCP denial on kernel 6.7+), with the enforced dimensions adversarially tested through the real spawn path (`rb01_sandbox_adversarial.rs`). UDP/ICMP, PID namespaces, per-run uid identities, per-tenant containers, and the boundary itself when the operator leaves the sandbox off remain unenforced — the residual matrix in the web API reference is the source of truth.

## Citing

> **Shixiang Wang**, *oxo-flow: compiled, memory-safe bioinformatics workflow orchestration*, bioRxiv, 2026, [https://doi.org/10.64898/2026.06.11.731578](https://doi.org/10.64898/2026.06.11.731578)
