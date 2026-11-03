# Toward Verified, Human-Centered Scientific Workflows: Research Proposal & Industrial Roadmap

> **Target**: a defensible path toward an industrial-grade production engine and a publication-tier contribution suitable for evaluation by *Nature Biotechnology* or *Nature Methods*. Target journal fit and acceptance remain hypotheses, not claims.
>
> **Core Premise**: Competing with Nextflow and Snakemake cannot be achieved by minor scheduling speedups alone. It requires a measurable advance from untyped path-based execution toward **biologically contract-checked, provenance-preserving, AI-assisted workflows** co-designed across **Hardware, Software, Design, and User Experience**. The word "verified" always refers to a declared check whose scope, assumptions, and false-positive rate must be tested; it does not mean mathematical proof of biological validity.
>
> **Execution Model**: This specification defines a long-term, multi-stage engineering and research program. Progression is governed by verifiable engineering and scientific acceptance gates, not fixed calendar schedules or prose completion claims.
>
> **Companion Implementation Plan**: [`docs/superpowers/plans/industrial-scientific-implementation-plan.md`](../plans/industrial-scientific-implementation-plan.md)
>
> **Audit Ledger**: [`docs/superpowers/audits/industrial-scientific-audit-ledger.md`](../audits/industrial-scientific-audit-ledger.md)

---

## 0. Current Status and Evidence Boundary

**Program status**: research and staged engineering. No stage exit gate has passed. The current system is not yet suitable for an industrial multi-tenant or clinical claim.

Maturity labels are used consistently:

- **Implemented and verified**: exercised by deterministic tests or measured evidence.
- **Implemented but unproven**: code exists, but the claimed production or scientific property is not demonstrated.
- **Prototype**: a usable path exists with known security, semantic, or operational gaps.
- **Simulation only**: model, fixture, seeded generator, or mock scheduler.
- **Planned / blocked**: not implemented or waiting on a release blocker.

| Area | Current maturity | Evidence boundary |
| :--- | :--- | :--- |
| Local workflow parsing, expansion, and BioTypes checks | Implemented but unproven | Focused local tests pass; broad domain truth-set validation is incomplete |
| Local execution and checkpointing | Implemented but unproven | Useful engine evidence exists; web run truth and remote artifact truth remain incomplete |
| Team/HPC web execution | Prototype / blocked | Authorization exists, but no OS-level tenant execution boundary or immutable per-run snapshot |
| PostgreSQL run lifecycle | Implemented but unproven | Trait and code paths exist; live parity, concurrency, crash recovery, and horizontal tests are incomplete |
| Kubernetes, AWS Batch, and Google Cloud Batch | Prototype | Executors shell out to CLIs rather than using native APIs; live campaigns are absent |
| NUMA and storage-tier co-design | Prototype | Discovery and routing code exist; performance and fallback claims are unmeasured |
| AI synthesis and diagnosis | Implemented but unproven | Current fault simulation detects and attributes most tested cases but applies and verifies zero repairs |
| Metro-DAG and Metropolis UX | Prototype | Accessibility, semantic safety, mobile, and scale evidence are incomplete |
| Benchmarks and human factors | Simulation or smoke test only | No matched experimental campaign or human-subjects study is complete |
| Manuscript package | Planned / blocked | No submission, acceptance, or independent replication exists |

Complete findings and release blockers are maintained in the audit ledger. No section below overrides that ledger.

---

## 1. Executive Summary & Scientific Vision

Bioinformatics workflow management has evolved through three distinct architectural generations:

- **First Generation (Ad-Hoc Scripting)**: Unstructured shell scripts and Makefiles lacking environment isolation, portable scheduling, and provenance guarantees.
- **Second Generation (Human-Authored Dataflow & Rule Engines)**: Declarative and reactive workflow managers (**Snakemake** [*Nature Biotechnology*; *Nature Methods*], **Nextflow** [*Nature Biotechnology*], **nf-core** [*Nature Biotechnology*], **CWL/WDL**). These systems standardized containerization and cluster dispatch, but their path-centric interfaces and embedded general-purpose languages limit domain-specific static checking. They generally expose hardware topology indirectly and leave cross-surface workflow state to integrations. These are design tensions to test empirically, not defects that every deployment experiences.
- **Third Generation (AI-Assisted, Contract-Checked, Quad-Co-Designed Scientific Workflows)**: Humans specify high-level scientific intent and experimental constraints; AI agents synthesize candidate pipelines into a canonical intermediate representation; a compiler checks declared structural and biological contracts prior to execution; a hardware-aware engine tests whether physical throughput improves; and a cohesive visual design system guides the user from authoring to review. Biological soundness still requires validation against external truth.

### The Central Paradox of AI in Computational Biology

While Large Language Models (LLMs) substantially accelerate pipeline authoring, unconstrained code generation introduces a severe scientific hazard: **plausible hallucinations and silent domain incompatibilities**. An LLM-generated workflow may parse cleanly and form an acyclic graph, yet fail catastrophically after hours of cluster compute—or worse, produce silently corrupted biological conclusions—due to domain-level violations (e.g., passing a queryname-sorted or unindexed BAM lacking `@RG` headers into `gatk HaplotypeCaller`, mixing `hg19` and `GRCh38` coordinates, or running `Mutect2` in paired mode without a matched normal).

`oxo-flow` investigates whether a **Quad-Pillar Co-Design Architecture** can reduce this contradiction by integrating **Hardware**, **Software**, **Design**, and **User Experience** into one scientific computing workflow. The causal benefit of that integration remains a research hypothesis.

The primary confirmatory claim should be narrow enough to falsify: declared
BioType contracts plus compiler-in-the-loop synthesis reduce silent semantic
errors and improve first-run validity relative to unconstrained workflow
generation. Hardware efficiency and cross-surface UX are supporting studies
unless the pre-registration gives them co-primary status with adequate power.
Replacing Nextflow or Snakemake additionally requires direct execution
comparisons on the same workflow, data, host, resource limits, and pinned
software environment.

---

## 2. The Quad-Pillar Co-Design Architecture

One hypothesis motivating this program is that fragmentation among execution, hardware topology, user interfaces, and AI assistance creates measurable costs. The magnitude and prevalence of those costs must be established through matched studies rather than assumed from architecture alone.

`oxo-flow` re-architects scientific workflow execution as a four-way organic synthesis:

```text
┌─────────────────────────────────────────────────────────────────────────────────────────────┐
│                 The Quad-Pillar Co-Designed Scientific Computing Engine                      │
└──────────────────────────────────────────────┬──────────────────────────────────────────────┘
                                               │
         ┌────────────────────────┬────────────┴────────────┬────────────────────────┐
         ▼                        ▼                         ▼                        ▼
┌───────────────────┐    ┌───────────────────┐    ┌───────────────────┐    ┌───────────────────┐
│    1. HARDWARE    │    │    2. SOFTWARE    │    │     3. DESIGN     │    │      4. UX        │
│  Systems Synergy  │    │ Verified Runtime  │    │ Visual Grammar    │    │ Cognitive Flow    │
├───────────────────┤    ├───────────────────┤    ├───────────────────┤    ├───────────────────┤
│ • NUMA Affinity   │    │ • Modern Rust Core  │    │ • Metro-DAG Map   │    │ • IDE ↔ CLI ↔ Web │
│ • NVMe Scratch    │    │ • BioTypes Engine │    │ • Metropolis UI   │    │ • 1-Click Repair  │
│ • GPU Profile Spec│    │ • Decoupled DB    │    │ • Multi-Scale Zoom│    │ • <10ms Telemetry │
│ • io_uring Stream │    │ • K8s & Slurm     │    │ • Diagnostic Glow │    │ • Zero-Code View  │
└───────────────────┘    └───────────────────┘    └───────────────────┘    └───────────────────┘
         ▲                        ▲                         ▲                        ▲
         └────────────────────────┴────────────┬────────────┴────────────────────────┘
                                               │
                                 Organic Feedback Loops
                 • Telemetry drives Visual Heatmaps and Causal Healing
                 • Declared BioTypes drive Visual Connection Grammar and Warnings
                 • NUMA Pinning and Tiered Scratch prevent UI Freezes & OOMs
```

### 2.1 Pillar I: Hardware-Aware Systems Co-Design

Workflow managers commonly expose compute as abstract CPU and memory counts. On multi-socket servers, cross-NUMA access and storage spill can affect memory- or I/O-bound genomics tools, but the direction and size of the effect are workload-specific. The current repository contains prototype topology discovery and scratch-tier selection; no performance claim has been established.

The target hardware layer (`crates/oxo-flow-core/src/hardware/`) includes:

- **NUMA & Cache-Aware Affinity Pinning (prototype)**: The scheduler can query topology from sysfs and emit `numactl` wrappers. Correct behavior across cgroups, containers, schedulers, and single-socket hosts still requires named-host tests and measured performance.
- **Hierarchical Storage Tiering (Tier 0 to Tier 3)**:
  - **Tier 0 (In-Memory / tmpfs)**: Zero-copy FIFO pipes for pure streaming conversions (`samtools view -b`).
  - **Tier 1 (High-Speed Local NVMe Scratch)**: Dynamically designated via `[engine] tmp_dir` / `OXO_FLOW_TMPDIR` for sorting buffers and intermediate FASTQs.
  - **Tier 2 (Parallel Shared Filesystem)**: For durable inputs, final checkpoints, and indexed alignments.
  - **Tier 3 (Cloud Object Stores)**: S3/GCS range-probed header inspection and streaming ingestion.
- **Heterogeneous Accelerator Topology (`GpuSpec`)**: The declaration and scheduler-directive plumbing exist. Runtime capability matching, fast-fail behavior, and Apple Silicon Unified Memory behavior require backend-specific validation.
- **Kernel-Bypass & Zero-Copy Streaming (target)**: Investigate Linux `io_uring` and pipe multiplexing to reduce disk round-trips. Any throughput or copy-elimination claim requires measured evidence on named tools, filesystems, and storage tiers.

### 2.2 Pillar II: Verification-Oriented Software Core

The software layer targets memory safety, auditable execution, and checked domain contracts. It does not guarantee biological correctness or bitwise reproducibility without environment, input, and tool-level controls:

- **Modern Rust Core**: memory safety without garbage collection; deterministic serialization using `BTreeMap`/`BTreeSet`. Compilation latency at $10^4$ rules is a benchmark hypothesis, not a current guarantee.
- **BioTypes Declared Semantic Contracts**: Backwards-compatible annotations (`[types.<name>]`, `[rules.types]`) that check declared format, index, sort-order, and reference-build constraints at DAG build time. The contract is only as reliable as its rules and data inspection.
- **Backend-Routed Run Persistence**: `/api/runs*` and run-node lifecycle paths route through a unified `StorageBackend` trait with SQLite and PostgreSQL implementations. Live PostgreSQL parity, advisory-lock behavior, crash recovery, and multi-instance operation remain unproven.
- **Environment Backend Isolation (prototype)**: Native `pixi`, rootless `apptainer`/`singularity`, and an opt-in shared environment store are integrated. This is not yet a hermetic security boundary: the audit ledger records ambient credential inheritance and other isolation gaps.
- **Type-Directed AI Orchestrator (target guardrail)**: LLMs act as proposers, and candidate pipelines are intended to pass schema validation, DAG analysis, `deep_check`, `scientific_preflight`, and BioType checks before dispatch. Fail-open tool-argument handling and partial-file replacement mean this is not yet a universal guarantee.

### 2.3 Pillar III: Metro-DAG Visual Grammar & "Metropolis" Design System

Complex bioinformatic pipelines (e.g., GATK Best Practices, single-cell multi-omics) generate massive directed graphs. Rendered as standard node-link hairballs, they become visually impenetrable and cognitively exhausting.

`oxo-flow` introduces the **Metro-DAG Visual Grammar** and the cross-platform **Metropolis Design System**:

- **Metro / Subway Map Visual Grammar**:
  - Data streams and sample cohorts are rendered as distinct, color-coded **Transit Lines**.
  - Computational rules are rendered as **Stations**. Interchange stations represent synchronization points (merges, joint variant calling); branching stations represent demultiplexing or scatter steps.
  - Multi-scale semantic collapse (`MetroGranularity::Rule`, `Process`, `Module`) allows users to switch between high-level conceptual workflows and low-level command details in a single click.
- **The Metropolis Design System**:
  - A shared tokenized visual language across CLI Terminal (TUI), Native Desktop App (Wry/Tao), Web SPA (React 19), and VS Code Extension.
  - **Intentional Biological Palettes**: Modality-specific color coding (Genomics = deep indigo/cyan; Transcriptomics = emerald/teal; Epigenomics = amber/coral; Metagenomics = violet/magenta; Infrastructure = slate/zinc).
  - **Dynamic Diagnostic Glow**: Rules in execution pulse with real-time hardware telemetry; failed nodes emit localized diagnostic halos that visually trace upstream causal inputs.

### 2.4 Pillar IV: Continuous Human-Centered User Experience (UX)

Modern scientists oscillate between command-line exploration, code editors, and graphical dashboards. Incumbent tools treat these as disjoint environments.

`oxo-flow` targets a continuous user experience:

- **Unbroken Workspace Continuum (CLI <-> IDE <-> Desktop <-> Web)**:
  - Author pipelines with AI assistance and schema completion in VS Code.
  - Preview the Metro-DAG locally with interactive hover diagnostics.
  - Launch runs from the terminal or web portal with shared session state, streaming live events over SSE.
- **Measured Interactive Feedback**: define p50/p95/p99 budgets for CLI invocation, language-server diagnostics, canvas frames, and repair review. Current target numbers are hypotheses until measured on declared hardware and graph sizes.
- **Causal 1-Click Error Remediation (target)**: The intended flow is to diagnose a failure, present an exact proposed patch, require acceptance where scientific semantics may change, and resume only after validation. Current simulations produce proposals but zero applied, verified, or successfully rerun repairs.
- **Assisted Ecosystem Migration (`oxo-flow import`)**: Deterministic translation of supported Nextflow DSL2 and Snakemake constructs into `.oxoflow`, with explicit unsupported-construct diagnostics and human review requirements. Migration cost and semantic equivalence remain hypotheses.

---

## 3. Potential Scientific Contribution

The program tests four coupled hypotheses. None is yet a publication result.

### 3.1 BioTypes: Declared Biological Contracts

**Hypothesis**: explicit biological contracts reduce silent domain mismatches and failed compute compared with untyped path matching.

Existing workflow engines primarily treat inputs and outputs as file paths. Building upon the static DAG engine (`crates/oxo-flow-core/src/dag.rs`) and preflight analyzers (`crates/oxo-flow-core/src/scientific_preflight.rs`, `deep_check.rs`), **BioTypes** adds backwards-compatible annotations without changing the meaning of workflows that omit them:

```toml
# Reusable biological type contract in .oxoflow
[config]
reference = "/data/references/GRCh38/genome.fa"

[types.BAM_Germline_Indexed]
format = "bam"
index = "bai"
sort_order = "coordinate"
reference_build = "GRCh38"
read_groups = { required = true, fields = ["ID", "SM", "PL"] }

[[rules]]
name = "variant_calling"
input = ["aligned/{sample}.sorted.bam"]
output = ["variants/{sample}.g.vcf.gz"]
shell = "gatk HaplotypeCaller -R {config.reference} -I {input[0]} -O {output[0]} -ERC GVCF"

[rules.types]
inputs = { default = "types.BAM_Germline_Indexed" }
outputs = { default = "GVCF[compression=bgzip]" }
```

- **Static Contract Checking**: The checker evaluates producer/consumer compatibility across inferred DAG edges and emits structured `TYPE-*` diagnostics. A coercion suggestion is not an applied repair until it is separately approved, inserted, and verified.
- **Bounded Header Inspection**: Header readers inspect BAM/CRAM/VCF/FASTQ metadata before dispatch without reading full payloads. Latency, false positives, false negatives, malformed inputs, remote range failures, and stale-index behavior must be measured across a defined corpus.

**Required evidence**:

- A curated positive/negative corpus across germline, somatic, transcriptomic, epigenomic, and metagenomic workflows.
- Confusion matrices for static detection and header inspection.
- Proof that existing untyped workflows retain byte-equivalent semantics.
- Independent biological review of every invariant and diagnostic.

### 3.2 Type-Directed AI Synthesis with Explicit Bounds

**Hypothesis**: compiler feedback and biological contracts improve valid first-run synthesis and reduce silent errors relative to unconstrained generation.

The current evaluation harness can compare unconstrained prompting, syntax-only feedback, and full validation feedback. Existing fixtures exercise the plumbing, but they do not establish an effect:

1. **Structured Task Contract & Curation**: Building on `TeamProfile` (`crates/oxo-flow-ai/src/agent/team.rs`), the deterministic Curator grounds candidate tools and versions against the embedded knowledge base (`bioconda_tools.jsonl`, `biotools_overlay.jsonl`, `nfcore_modules.jsonl`, `edam_terms.jsonl`).
2. **Compiler-in-the-Loop Checks**: Candidate `.oxoflow` artifacts pass schema validation, DAG analysis, `deep_check`, `scientific_preflight`, and `BioTypes` checks inside the orchestrator loop.
3. **Generalized Hybrid Re-Entry**: `reentry.rs` supports named `[[values]]` axes while preserving deterministic replay and bounded re-entry rounds.

**Required evidence**:

- Pre-registered held-out intents across modalities and workflow complexity.
- Fixed models, prompts, tool availability, budgets, and seeds.
- Matched Arms A/B/C with per-item raw results.
- McNemar and effect-size analysis only after matched data exists.
- Prompt-injection, tool-poisoning, leakage, and unsupported-claim tests.

### 3.3 Causal Diagnosis and Bounded Repair

**Hypothesis**: structured diagnosis can safely propose or apply a bounded subset of infrastructure and data repairs while preserving scientific intent.

The current CLI simulation reports 8/10 detections, 7/10 attributed diagnoses, 7/10 proposed repairs, and zero applied, verified, or successfully rerun repairs. This is evidence for the test harness and against a self-healing completion claim.

- **Layer 1: Infrastructure & Resource Faults** (SIGKILL/137 OOM, walltime expiration, scratch exhaustion, transient node loss) $\to$ Driven by `Executor` peak RSS/CPU sampling (`crates/oxo-flow-core/src/executor/rss.rs`) and disk-pressure guards, the runtime scales memory/time allocations or reroutes scratch (`tmp_dir`) within policy bounds.
- **Layer 2: Data & Format Anomalies** (truncated BGZF blocks, unsorted intervals, missing tabix/BAI indices, contig prefix mismatch `chr1` vs `1`) $\to$ Proposed localized repair or indexing rules, subject to approval policy.
- **Layer 3: Cohort & Assay Quality Drift** (low mapping rate, adapter contamination, insufficient cohort size for VQSR) $\to$ Guided branch switching (e.g., transitioning from GATK VQSR to hard-filtering when cohort size $<30$, consistent with `SCI-VQSR-COHORT`).
- **Layer 4: Provenance-Preserving Audit**: Every proposed or applied modification is recorded in `.oxo-flow/ai_sessions/` and `CheckpointState` (`crates/oxo-flow-core/src/executor/checkpoint.rs`). Auditability does not by itself make a repair scientifically valid.

**Required evidence**:

- Exact patch application, not merely display of a repair proposal.
- Re-validation of the resulting workflow and outputs.
- Successful rerun and output verification after repair.
- Explicit refusal for repairs that alter filtering thresholds, drop samples, or change biological assumptions without human approval.

### 3.4 Ecosystem Interoperability (`oxo-flow import`)

**Hypothesis**: deterministic translation can reduce migration cost without changing workflow semantics or hiding unsupported constructs.

- **Module & Workflow AST Transpiler**: Parses Nextflow DSL2 (`process`, `workflow`, `container`, `conda`, `input`/`output` qualifiers) and Snakemake (`rule`, `input`, `output`, `params`, `conda`, `container`, `shell`) into canonical `.oxoflow` rules and `[[include]]` modules.
- **Assisted BioType Lifting**: Infers candidate contracts from EDAM ontology annotations and requires review where confidence is low.

**Required evidence**:

- A versioned benchmark corpus drawn from canonical modules and edge cases.
- Explicit unsupported-construct diagnostics rather than lossy translation.
- Semantic equivalence tests for parameters, channels, resources, containers, and failure behavior.
- Human review of translated biological intent.

---

## 4. Production Architecture Requirements

Industrial readiness is an evidence claim, not an architecture label. The current baseline and required proof are:

| Production Dimension | Current maturity | Required proof before an industrial claim |
| :--- | :--- | :--- |
| **Hardware topology and affinity** | Prototype | Topology discovery on named Linux hosts, tested affinity behavior, fallback on single-socket hosts, and measured effects on representative tools |
| **Execution state** | Implemented but unproven | Full run lifecycle through one storage interface, live SQLite/PostgreSQL parity, immutable run snapshots, concurrency, crash recovery, and horizontal-instance tests |
| **Compute dispatch** | Prototype | Live Kubernetes/AWS/GCP campaigns, truthful per-element outcomes, cancellation reconciliation, restart recovery, and artifact integrity |
| **Object storage and I/O** | Implemented but unproven | Range-read correctness, streaming backpressure, interrupted-stream handling, scratch-footprint measurements, and credential-scope tests |
| **Visual design and UX** | Prototype | Semantic round-trip preservation, diff acceptance, keyboard operation, WCAG evidence, mobile coverage, and measured graph performance |
| **Security and governance** | Prototype / blocked | OS/container tenant isolation, minimal environment, enforced resources, durable audit, tenant-scoped state, and fail-closed auth |
| **Scientific validity** | Planned / blocked | Truth-set validation, matched ablations, external review, negative results, and independent reproduction |

An architecture diagram is not evidence of any row above.

### 4.1 Integrated Quad-Pillar Acceptance

Hardware, software, design, and user experience are accepted only through
end-to-end journeys that exercise all four together. A component-level pass
cannot substitute for an integrated gate.

| Journey | Hardware evidence | Software evidence | Design evidence | UX evidence |
| :--- | :--- | :--- | :--- | :--- |
| Author and validate a typed workflow | Declared resource profile matches the target host class | Parsed DAG, BioType diagnostics, and complete-file validation | Diagnostic hierarchy and graph grammar remain readable | First-time user can explain the failure and reach a valid repair |
| Execute under resource pressure | Measured CPU, memory, storage, and affinity behavior | Truthful checkpoint, provenance, cancellation, and recovery | Pressure state is visible without implying false health | The user can choose a safe recovery action and verify its outcome |
| Resume an interrupted cohort | Inputs and intermediate storage tiers remain consistent | Replay is deterministic and state transitions are monotonic | Before/after graph state is visually distinguishable | The user can identify what reran, what was reused, and why |
| Repair a failed run | Repair does not exceed declared physical limits | Exact patch, validation, rerun, and output verification share one identity | Diff and applied change are visually the same object | Scientific-semantic changes require explicit acceptance |
| Migrate a Nextflow or Snakemake workflow | Resource intent and container assumptions survive translation | Unsupported constructs fail explicitly and typed contracts remain reviewable | Source and translated structure are comparable | The user can inspect every inferred or dropped construct |

An integrated gate passes only when the same recorded scenario, software
commit, dataset, host profile, and user task evidence are linked. Mock,
synthetic, and model-projection results may exercise a journey but cannot
qualify it.

### 4.2 Genomic Privacy and Data Governance

Human genomic and clinical data require a governance boundary independent of
workflow correctness:

- classify data before ingestion and enforce tenant, role, and data-use limits;
- do not equate identifier removal with anonymization;
- treat model prompts, retrieved context, logs, webhooks, and MCP calls as
  possible data-egress events;
- define encryption, key ownership, backup, retention, deletion, and incident
  response before team/HPC deployment;
- require documented ethics approval and data-access terms for every
  human-subjects or controlled-access dataset used in a study;
- publish only data that the governing agreement permits and state the limits
  of any controlled-access reproduction package.

Privacy compliance is not established by the engine's type checker,
reproducibility record, or audit log.

---

## 5. Multi-Stage Development Roadmap & Acceptance Gates

Execution is structured into four sequential stages. No stage is entered as a completion claim. Gate 0 in the audit ledger must pass before team/HPC expansion, and every wave must keep the repository CI suite green.

```text
Stage 1                           Stage 2                           Stage 3                           Stage 4
Hardware-Aware Cloud Core &       BioTypes & Metro-DAG              Interoperability & Bounded        Large-Scale Benchmark,
Decoupled Persistence             Visual Design System              Repair Continuum                  HCI Study & Publication
  │                                 │                                 │                                 │
  ├─► Wave 1.1: Postgres Runs       ├─► Wave 2.1: BioTypes Lattice    ├─► Wave 3.1: nf-core Transpiler  ├─► Wave 4.1: Multi-Omics Bench
  ├─► Wave 1.2: Cloud Backends      ├─► Wave 2.2: Header Inspectors   ├─► Wave 3.2: Causal Graph        ├─► Wave 4.2: Cohort & HW Study
  ├─► Wave 1.3: Streaming I/O       ├─► Wave 2.3: 50+ SCI Preflight   ├─► Wave 3.3: Bounded Repair      ├─► Wave 4.3: User Study & SUS
  └─► Wave 1.4: NUMA & NVMe Tier    └─► Wave 2.4: Metropolis Design   └─► Wave 3.4: IDE ↔ Web Continuum └─► Wave 4.4: Submission Package
```

### Stage 1: Truthful Execution, Persistence & Safe Operations

- **Wave 1.0: Truthful Run Ledger and Isolation Boundary**
  - Make create, stage, persist, and spawn atomic.
  - Give each run an immutable artifact snapshot.
  - Enforce monotonic lifecycle transitions, atomic upload replacement, and verified output handling.
  - Introduce an OS/container runner with a minimal environment and enforced resources before allowing untrusted team tenants.
- **Wave 1.1: PostgreSQL Run Lifecycle Parity**
  - Complete and test the existing `StorageBackend` run lifecycle through `sqlite.rs` and `postgres.rs`.
  - Rename the historical `require_sqlite_for_runs` capability gate and error naming after live backend evidence exists.
  - Add concurrency, crash-recovery, and multi-instance tests.
- **Wave 1.2: Cloud and Kubernetes Execution**
  - Define whether each outbound adapter is API-native or an explicit CLI-wrapper prototype.
  - Implement API-native Kubernetes, AWS Batch, and Google Cloud Batch drivers only when their support and credential models are justified.
  - Wire `[engine] tmp_dir` / `OXO_FLOW_TMPDIR` and container volume mounts consistently across local, HPC cluster, and cloud batch executors.
- **Wave 1.3: Streaming & Zero-Copy Object Storage**
  - Complete `staging_mode = "eager" | "stream" | "pass_through"` with range-read correctness, backpressure, interruption recovery, and credential scoping.
  - Measure scratch reduction and failure behavior on large BAM/FASTQ objects.
- **Wave 1.4: NUMA-Aware Affinity & Multi-Tier Scratch Management**
  - Validate topology discovery against `lscpu`, `numactl`, and sysfs on named hosts.
  - Test memory-node affinity binding and the 4-tier scratch ladder under pressure, fallback, and single-socket conditions.
- **Stage 1 Exit Gate**:
  - Gate 0 and Gate 1 pass.
  - Full `/api/runs*` contract tests pass against SQLite and PostgreSQL, including concurrency and crash recovery.
  - Live backend campaigns cover submit, queue, run, cancel, fail, retry, and artifact retrieval.
  - NUMA performance is reported from measured confidence intervals. A 25% latency improvement is a pre-registered hypothesis, not an acceptance assumption.

### Stage 2: BioTypes Semantic Typing & Metro-DAG Visual Design System

- **Wave 2.1: BioTypes Grammar, Schema & Type Unifier**
  - Harden the existing `[types.<name>]` and `[rules.types]` model, known-key coverage, schema, and generated editor data as a single contract.
  - Expand and validate `crates/oxo-flow-core/src/types/` subtyping, attribute unification (`format`, `sort_order`, `index`, `reference_build`, `strandedness`, `read_groups`), and `TYPE-*` diagnostics against independent positive and negative corpora.
- **Wave 2.2: Binary Genomic Header Introspection**
  - Harden the existing bounded header readers in `oxo-flow-core` for local and remote range-fetched BAM/CRAM/VCF/FASTQ inputs, including malformed data, stale indices, truncation, and interrupted range requests.
- **Wave 2.3: Generalized Hybrid Re-Entry & Expanded `SCI-*` Preflight**
  - Stress-test the existing named `[[values]]` re-entry path (`crates/oxo-flow-core/src/reentry.rs`) for replay determinism, collision handling, and failure recovery.
  - Validate and harden the 51 literature-backed `SCI-*` checks already implemented in `crates/oxo-flow-core/src/scientific_preflight.rs` (this branch expanded the original 5-check baseline) across genomics, bulk/single-cell transcriptomics, epigenomics, and metagenomics, with full documentation in `docs/guide/src/`.
- **Wave 2.4: Metropolis Design System & Metro-DAG Studio**
  - Formalize design tokens, typography, and biological color hierarchies shared across Web, Desktop, Terminal TUI, and VS Code.
  - Overhaul the React Flow canvas into a high-performance Metro transit renderer with dynamic station collapsing and telemetry-driven transit-line pulsing.
- **Stage 2 Exit Gate**:
  - The fault corpus has a pre-registered composition and reports sensitivity, specificity, false positives, false negatives, and confidence intervals. A 95% target is not a result.
  - Biological experts review every rule and diagnostic.
  - Metro-DAG performance is measured on declared hardware for representative graphs, including interaction latency and accessibility.
  - Round-trip editing preserves all supported semantics or blocks the operation.

### Stage 3: Ecosystem Interoperability & Bounded Repair Continuum

- **Wave 3.1: Nextflow DSL2 & Snakemake Transpiler (`oxo-flow import`)**
  - Build a deterministic parser and transpiler converting `nf-core` modules/subworkflows and Snakemake rules into `.oxoflow` workflows with automatic `[rules.types]` annotations derived from the embedded knowledge base.
- **Wave 3.2: Causal Diagnostic Graph**
  - Unify `crates/oxo-flow-web/src/domains/execution/diagnostics.rs` and `oxo-flow-core` failure telemetry into a shared `oxo-flow-core::diagnostics` module that correlates exit codes, signals, stderr patterns, peak RSS/CPU (`BenchmarkRecord`), and input header metadata.
- **Wave 3.3: Bounded Autonomous Remediation & 1-Click Fix Actions**
  - Upgrade the recovery runtime so infrastructure, memory, and formatting faults produce verified graph repairs (resource scaling, adapter rule insertion) invokable via 1-click in the UI or 1-key in the CLI.
- **Wave 3.4: Seamless Cross-Platform Workflow Continuum**
  - Unify project context, live SSE event streams, and checkpoint state between VS Code, CLI, Desktop, and Web Server, allowing seamless handover between local drafting and cluster monitoring.
- **Stage 3 Exit Gate**:
  - Curated transpilation covers declared supported constructs and fails explicitly for unsupported ones.
  - Repair application, verification, and successful rerun are measured separately. The current baseline is zero applied and zero verified repairs.
  - No repair may alter biological assumptions without explicit human approval.

### Stage 4: Large-Scale Empirical Benchmark, Human-Factors Evaluation & Manuscript Package

- **Wave 4.1: Multi-Omics Synthesis & Verification Benchmark (`eval/frontier/`)**
  - Expand `eval/` and `eval/frontier/` into a comprehensive, multi-hundred-intent benchmark comparing unconstrained Nextflow/Snakemake LLM synthesis against `oxo-flow`'s type-directed compiler loop across pass@k, silent-error detection, and token cost.
- **Wave 4.2: Direct Systems Equivalence & Performance Comparison**
  - Translate one pre-registered workflow into Nextflow, Snakemake, and `oxo-flow`; pin engine versions, container digests, data, CPU/memory limits, scheduler, and host. Compare declared-output equivalence, semantic error detection, wall time, peak memory, disk I/O, restart behavior, and failure semantics across repeated randomized orderings.
  - Missing or unsupported constructs must be reported as coverage limits, not silently excluded. This direct comparison is the minimum evidence for a replacement claim; AI-authoring results alone cannot establish it.
- **Wave 4.3: Population-Scale Cohort & Hardware Scalability Study**
  - Execute end-to-end germline, somatic, and transcriptomic pipelines at cohort scale across HPC and cloud backends, quantifying scheduling overhead, NUMA scaling efficiency, peak memory accuracy, storage I/O savings, and bit-level/semantic provenance reproducibility.
- **Wave 4.4: Quantitative Human-Factors & User Experience Study**
  - Obtain ethics approval before recruitment. Conduct randomized, counterbalanced usability evaluations with academic and clinical bioinformatics practitioners measuring task completion, debugging efficiency, NASA-TLX, and SUS against Nextflow and Snakemake.
- **Wave 4.5: Reproducible Submission & Artifact Release**
  - Package datasets, exclusion rules, raw run-level results, fault-injection harnesses, transpiler outputs, and analysis scripts as an open artifact.
  - Journal submission occurs only after Gate 4 passes. Acceptance is not a planning input.

---

## 6. Pre-Registered Evaluation Hypotheses

The thresholds below are falsifiable design targets. They must be registered before the confirmatory campaign and are not current measurements. Exploratory data may refine them, but cannot retroactively convert an exploratory threshold into a confirmatory result.

| Evaluation Dimension | Hypothesized Threshold | Comparator to Measure |
| :--- | :--- | :--- |
| **Pre-Execution Domain Error Detection** | $>95\%$ sensitivity with reported specificity and confidence intervals | Matched error corpus under Nextflow, Snakemake, and `oxo-flow` |
| **AI Workflow Synthesis First-Run Pass Rate** | $>85\%$ all-gates pass on held-out intents | Matched Arms A/B/C under fixed model, prompts, seeds, and budgets |
| **Direct Execution Equivalence** | Pre-registered non-inferiority margin for declared outputs and completion | Same workflow, data, host, resources, engine/runtime versions, and container digests |
| **Direct Execution Overhead** | Pre-registered practical margin for wall time and peak memory | Repeated randomized Nextflow, Snakemake, and `oxo-flow` runs |
| **Autonomous Fault Recovery Rate** | $>70\%$ applied, verified, and successfully rerun repairs | The same fault corpus with the same recovery budget |
| **Hardware Scheduling Efficiency** | $>25\%$ improvement in the declared primary metric | Repeated measured runs on named multi-socket hardware |
| **DAG Compilation** | $<50\text{ ms}$ for $10^4$ rules | Identical workflows and hardware across engines |
| **Storage Staging Footprint** | $50\text{--}80\%$ reduction in local scratch | Controlled object sizes, filesystems, and cache states |
| **User Experience** | $>80$ SUS after a real study; measured CLI/IDE latency | Recruited participants and the same task set across tools |

Every hypothesis requires:

- a primary outcome and direction;
- inclusion, exclusion, stopping, and outlier rules;
- matched units of analysis;
- software, model, prompt, hardware, and dataset provenance;
- confidence intervals and effect sizes;
- negative and null results;
- an independent analysis check.

---

## 7. Risk Analysis & Architectural Guardrails

- **Risk 1: Ecosystem Inertia Around `nf-core` and Snakemake**
  - *Mitigation*: Wave 3.1 tests deterministic migration (`oxo-flow import`) of supported `nf-core` modules and Snakemake constructs. Translation must report unsupported constructs explicitly, and inferred BioTypes require review. Repair remains experimental rather than an automatic migration promise.
- **Risk 2: False Positives in Static Biological Type Checking**
  - *Mitigation*: `BioTypes` adopts gradual typing—rules without `[rules.types]` retain existing path-matching semantics. Any future coercion policy must be explicit, reviewable, and provenance-recorded; no coercion override is currently exposed.
- **Risk 3: Memory & Checkpoint Contention at $10^5$-Rule Scale**
  - *Mitigation*: Preserve `oxo-flow`'s compact `petgraph` representation, delta-append checkpoint journals before atomic compaction, and stream remote I/O rather than buffering whole artifacts in memory.
- **Risk 4: Non-Determinism from External LLM Providers**
  - *Target guardrail*: synthesized or repaired `.oxoflow` artifacts must pass deterministic parsing, complete-file validation, BioType checks, and an explicit diff review before dispatch. This is not yet a current guarantee: the audit ledger records model-approved partial replacement and fail-open tool-argument paths that block the claim.

---

## 8. Governance & Development Workflow

All implementation work under this roadmap must strictly adhere to the repository's engineering invariants (`AGENTS.md`):

- **Incremental, Non-Breaking Waves**: Every wave lands as focused, reviewable PRs with unit and integration tests; the complete `make ci` suite defined in `AGENTS.md` must pass at every step, including formatting, Clippy with warnings denied, build, tests, schema and version drift, security scans, documentation, frontend checks, evaluation checks, and the VS Code extension gate.
- **Single Source of Truth**: Any schema or CLI surface addition must simultaneously update `docs/schema/oxoflow-v1.schema.json`, VS Code generated completion data, `AGENTS.md`, `README.md`, and `docs/guide/src/`.
- **Zero Silent Fallbacks**: Every new diagnostic (`TYPE-*`, `SCI-*`, `D00*`) ships with deterministic tests and user-facing remediation guidance.
