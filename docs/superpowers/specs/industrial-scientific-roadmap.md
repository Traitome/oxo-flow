# Autonomous, Formally-Verified Scientific Workflows: Research Proposal & Industrial Roadmap

> **Target**: Dual attainment of an **industrial-grade production engine** (petabyte-scale, cloud-native, multi-tenant, zero silent errors) and a **scientific breakthrough publication in *Nature Biotechnology* or *Nature Methods***.
>
> **Core Premise**: Disrupting Nextflow and Snakemake cannot be achieved by minor scheduling speedups alone. It requires a fundamental paradigm shift: transitioning scientific workflow management from untyped, fragile glue scripts to **formally-verified, biologically-typed, AI-synthesized autonomous computational ecosystems** through the organic co-design of **Hardware, Software, Design, and User Experience**.
>
> **Execution Model**: This specification establishes the architectural foundation for a long-term, multi-stage engineering and research program. Progression across stages is governed by strict, verifiable engineering and scientific acceptance gates rather than fixed calendar schedules.
>
> **Companion Implementation Plan**: [`docs/superpowers/plans/industrial-scientific-implementation-plan.md`](../plans/industrial-scientific-implementation-plan.md)

---

## 1. Executive Summary & Scientific Vision

Bioinformatics workflow management has evolved through three distinct architectural generations:

- **First Generation (Ad-Hoc Scripting)**: Unstructured shell scripts and Makefiles lacking environment isolation, portable scheduling, and provenance guarantees.
- **Second Generation (Human-Authored Dataflow & Rule Engines)**: Declarative and reactive workflow managers (**Snakemake** [*Nature Biotechnology*; *Nature Methods*], **Nextflow** [*Nature Biotechnology*], **nf-core** [*Nature Biotechnology*], **CWL/WDL**). These systems standardized containerization and cluster dispatch, yet rely on untyped file path strings and embedded general-purpose scripting languages (Python/Groovy) that resist static formal verification, remain blind to underlying hardware architectures (NUMA, NVMe hierarchies, GPU compute profiles), and offer fragmented, developer-hostile user interfaces.
- **Third Generation (AI-Native, Formally-Verified, Quad-Co-Designed Scientific Workflows)**: Humans specify high-level scientific intent and experimental constraints; AI agents synthesize candidate pipelines into a canonical intermediate representation; a formal compiler proves biological and semantic soundness prior to execution; a hardware-aware engine extracts optimal physical throughput; and a cohesive visual design system guides the user from authoring to clinical reporting.

### The Central Paradox of AI in Computational Biology

While Large Language Models (LLMs) substantially accelerate pipeline authoring, unconstrained code generation introduces a severe scientific hazard: **plausible hallucinations and silent domain incompatibilities**. An LLM-generated workflow may parse cleanly and form an acyclic graph, yet fail catastrophically after hours of cluster compute—or worse, produce silently corrupted biological conclusions—due to domain-level violations (e.g., passing a queryname-sorted or unindexed BAM lacking `@RG` headers into `gatk HaplotypeCaller`, mixing `hg19` and `GRCh38` coordinates, or running `Mutect2` in paired mode without a matched normal).

`oxo-flow` resolves this contradiction by pioneering the **Quad-Pillar Co-Design Architecture**: uniting **Hardware**, **Software**, **Design**, and **User Experience** into an unbroken scientific computing system.

---

## 2. The Quad-Pillar Co-Design Architecture

The defining failure mode of incumbent workflow managers is architectural fragmentation: software engines ignore modern hardware topology, user interfaces are treated as superficial web dashboards tacked onto shell scripts, and AI integration is reduced to unverified chat wrappers.

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
                 • Formal BioTypes drive Visual Connection Grammar & Proofs
                 • NUMA Pinning and Tiered Scratch prevent UI Freezes & OOMs
```

### 2.1 Pillar I: Hardware-Aware Systems Co-Design

Incumbent workflow engines treat physical compute as a generic pool of abstract CPU cores and RAM bytes. On modern multi-socket server architectures (dual AMD EPYC / Intel Xeon with 128+ cores), cross-NUMA memory latency degrades memory-bound genomics tools (`bwa-mem`, `samtools sort`, `STAR`) by 30% to 50%. Furthermore, uncontrolled I/O spills crash cluster shared filesystems (Lustre/GPFS).

`oxo-flow` builds hardware awareness directly into the core engine (`crates/oxo-flow-core/src/hardware/`):

- **NUMA & Cache-Aware Affinity Pinning**: The scheduler queries physical hardware topology (`hwloc` / `/sys/devices/system/node`) and pins thread pools and memory allocations to local NUMA domains, avoiding cross-interconnect bandwidth penalties.
- **Hierarchical Storage Tiering (Tier 0 to Tier 3)**:
  - **Tier 0 (In-Memory / tmpfs)**: Zero-copy FIFO pipes for pure streaming conversions (`samtools view -b`).
  - **Tier 1 (High-Speed Local NVMe Scratch)**: Dynamically designated via `[engine] tmp_dir` / `OXO_FLOW_TMPDIR` for sorting buffers and intermediate FASTQs.
  - **Tier 2 (Parallel Shared Filesystem)**: For durable inputs, final checkpoints, and indexed alignments.
  - **Tier 3 (Cloud Object Stores)**: S3/GCS range-probed header inspection and streaming ingestion.
- **Heterogeneous Accelerator Topology (`GpuSpec`)**: Rather than naive integer counts (`--gpus 1`), `GpuSpec` declares minimum VRAM capacity, TensorCore generation, precision profile (FP16/BF16/TF32 for deep learning variant callers like DeepVariant or Clair3), and transparently supports Apple Silicon Unified Memory architectures.
- **Kernel-Bypass & Zero-Copy Streaming**: Leverages Linux `io_uring` and pipe multiplexing to stream data directly between tool processes, eliminating unnecessary disk round-trips.

### 2.2 Pillar II: Formally-Verified Software Core

The software layer guarantees memory safety, reproducible execution, and formal domain correctness:

- **Modern Rust High-Performance Core**: Memory safety without garbage collection jitter; compilation of $10^4$ rules in $<50\text{ ms}$; deterministic serialization using `BTreeMap`/`BTreeSet`.
- **BioTypes Formal Semantic Type System**: Backwards-compatible semantic annotations (`[types.<name>]`, `[rules.types]`) that guarantee biological format, index synchronization, sort order, and reference genome compatibility at DAG build time.
- **Decoupled Persistence (Issue #207 Resolution)**: Full migration of `/api/runs*` and run node lifecycle management to a unified `StorageBackend` trait, enabling enterprise multi-tenant PostgreSQL clusters with advisory-lock coordination.
- **Hermetic Environment Isolation**: Native `pixi` (sub-second conda solver in Rust), rootless `apptainer`/`singularity` for HPC security, and an opt-in shared deduplicated store (`OXO_FLOW_ENV_STORE`).
- **Type-Directed AI Orchestrator**: LLMs act as intuitive proposers; candidate pipelines must pass the static Rust type unifier, `deep_check`, and `scientific_preflight` before scheduling.

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

`oxo-flow` provides a continuous, frictionless user experience:

- **Unbroken Workspace Continuum (CLI <-> IDE <-> Desktop <-> Web)**:
  - Author pipelines with AI assistance and schema completion in VS Code.
  - Preview the Metro-DAG locally with interactive hover diagnostics.
  - Launch runs from the terminal or web portal with shared session state, streaming live events over SSE.
- **Sub-10ms Interactive Cognitive Feedback**: Instant CLI invocation (<10ms), sub-20ms language server diagnostics, and 60fps hardware-accelerated canvas interactions.
- **Causal 1-Click Error Remediation**: When a run encounters an error, the user is never abandoned to opaque stack traces. The system diagnoses the failure (e.g., "OOM on rule `bwa_mem`: peak RSS 16.1 GB exceeded 16.0 GB allocation") and provides a **1-Click / 1-Key action** to auto-scale resources and resume from the exact failure point.
- **Zero-Cost Ecosystem Migration (`oxo-flow import`)**: Deterministic AST transpilation of Nextflow DSL2 and Snakemake pipelines into typed `.oxoflow` files, guaranteeing that labs do not lose decades of accumulated pipeline IP.

---

## 3. Scientific Innovation: The Nature-Caliber Contribution

To meet the methodological rigor of *Nature Methods* and the translational impact of *Nature Biotechnology*, `oxo-flow` advances four coupled scientific innovations:

### 3.1 BioTypes: A Formal Semantic Biological Type System

Existing workflow engines treat inputs and outputs as opaque file paths. Building upon `oxo-flow`'s static DAG engine (`crates/oxo-flow-core/src/dag.rs`) and preflight analyzers (`crates/oxo-flow-core/src/scientific_preflight.rs`, `deep_check.rs`), **BioTypes** introduces backwards-compatible semantic type annotations that attach domain invariants to rule I/O without breaking existing `FilePatterns` path matching:

```toml
# Reusable biological type contract in .oxoflow
[types.BAM_Germline_Indexed]
format = "BAM"
index = "BAI"
sort_order = "coordinate"
reference_build = "GRCh38"
read_groups = { required = true, fields = ["ID", "SM", "PL"] }

[[rules]]
name = "variant_calling"
input = ["aligned/{sample}.sorted.bam"]
output = ["variants/{sample}.g.vcf.gz"]
shell = "gatk HaplotypeCaller -R {config.reference} -I {input[0]} -O {output[0]} -ERC GVCF"

[rules.types]
input = { "aligned/{sample}.sorted.bam" = "types.BAM_Germline_Indexed" }
output = { "variants/{sample}.g.vcf.gz" = "types.GVCF[variant_type=germline, compression=bgzip]" }
```

- **Compile-Time Subtyping & Adapter Synthesis**: The type checker enforces structural subtyping ($T_{\text{producer}} \preceq T_{\text{consumer}}$) across every inferred DAG edge. If an aligner rule emits `BAM[sort_order=queryname, index=none]` into a consumer requiring `BAM_Germline_Indexed`, compilation halts with a structured `TYPE-*` diagnostic and offers an automated coercion rule (`samtools sort` + `samtools index`).
- **Sub-Millisecond Header Introspection**: Before dispatching source rules or resuming from checkpoints, zero-allocation Rust header readers (`noodles`-backed parsers for BAM/CRAM/VCF/BCF/FASTQ) inspect `@HD`/`@SQ`/`@RG` records and contig dictionaries to verify that on-disk files satisfy declared `reference_build`, `sort_order`, and index freshness invariants without scanning full payloads.

### 3.2 Type-Directed AI Synthesis with Bounded Soundness

Empirical evaluation in `eval/frontier/` demonstrates that unconstrained LLM prompting frequently hallucinates invalid DSL dialects (`E017`), whereas wiring `PipelineGenAgent` (`crates/oxo-flow-ai/src/agent/pipeline_gen.rs`) into the `Orchestrator` validation-feedback loop (`crates/oxo-flow-ai/src/agent/orchestrator.rs`) substantially raises pass rates. `oxo-flow` extends this architecture into formal **Type-Directed Synthesis**:

1. **Structured Task Contract & Curation**: Building on `TeamProfile` (`crates/oxo-flow-ai/src/agent/team.rs`), the deterministic Curator grounds candidate tools and versions against the embedded knowledge base (`bioconda_tools.jsonl`, `biotools_overlay.jsonl`, `nfcore_modules.jsonl`, `edam_terms.jsonl`).
2. **Compiler-in-the-Loop Verification**: Candidate `.oxoflow` artifacts must pass schema validation, DAG cycle/dead-node analysis, `deep_check` (`D001`–`D004`), `scientific_preflight` (`SCI-*`), and `BioTypes` unification (`TYPE-*`) inside the orchestrator loop.
3. **Generalized Hybrid DAG Re-Entry**: Extending `crates/oxo-flow-core/src/reentry.rs` (`checkpoint = true`, `ReentryRecord`, `MAX_REENTRY_ROUNDS`) from `sample` and `pairs` discovery to arbitrary typed `[[values]]` axes (e.g., dynamically assembled contigs, demultiplexed barcodes, or filtered cell clusters) while keeping every re-entry wave statically verifiable and deterministically replayable.

### 3.3 Causal Graph-Driven Autonomous Self-Healing

Rather than treating all non-zero exit codes with blind retries, `oxo-flow` unifies the CLI `--ai-recover` path and the Web `DiagnosticsEngine` (`crates/oxo-flow-web/src/domains/execution/diagnostics.rs`) into a four-layer causal fault-isolation and remediation runtime:

- **Layer 1: Infrastructure & Resource Faults** (SIGKILL/137 OOM, walltime expiration, scratch exhaustion, transient node loss) $\to$ Driven by `Executor` peak RSS/CPU sampling (`crates/oxo-flow-core/src/executor/rss.rs`) and disk-pressure guards, the runtime scales memory/time allocations or reroutes scratch (`tmp_dir`) within policy bounds.
- **Layer 2: Data & Format Anomalies** (truncated BGZF blocks, unsorted intervals, missing tabix/BAI indices, contig prefix mismatch `chr1` vs `1`) $\to$ Automated synthesis and execution of localized repair/indexing bridge rules.
- **Layer 3: Cohort & Assay Quality Drift** (low mapping rate, adapter contamination, insufficient cohort size for VQSR) $\to$ Guided branch switching (e.g., transitioning from GATK VQSR to hard-filtering when cohort size $<30$, consistent with `SCI-VQSR-COHORT`).
- **Layer 4: Provenance-Preserving Audit**: Every autonomous modification is recorded in `.oxo-flow/ai_sessions/` and `CheckpointState` (`crates/oxo-flow-core/src/executor/checkpoint.rs`), ensuring no silent mutation of scientific methodology.

### 3.4 Universal Ecosystem Transpiler (`oxo-flow import`)

To eliminate the ecosystem cold-start barrier against Nextflow's `nf-core` and Snakemake's workflow catalog:

- **Module & Workflow AST Transpiler**: Parses Nextflow DSL2 (`process`, `workflow`, `container`, `conda`, `input`/`output` qualifiers) and Snakemake (`rule`, `input`, `output`, `params`, `conda`, `container`, `shell`) into canonical `.oxoflow` rules and `[[include]]` modules.
- **Automated BioType Lifting**: Infers `[rules.types]` contracts from EDAM ontology annotations in `nfcore_modules.jsonl` and `biotools_overlay.jsonl`, turning untyped community modules into statically type-checked `oxo-flow` pipelines.

---

## 4. Industrial-Grade Production Architecture

To transition from single-node/HPC-script execution to enterprise multi-tenant production, `oxo-flow` targets four foundational architectural upgrades grounded in the current codebase:

| Production Dimension | Current Implementation Baseline | Target Industrial Architecture |
| :--- | :--- | :--- |
| **Hardware Topology & Affinity** | OS thread scheduler default; no NUMA awareness; single scratch mount (`tmp_dir`). | **Hardware-Aware NUMA & Tiered Scratch**: Automatic NUMA domain discovery and core pinning; multi-tier scratch (RAM disk to NVMe to Lustre to S3) managed by dynamic pressure ladder. |
| **Execution State Store** | `StorageBackend` trait exists (`crates/oxo-flow-web/src/infra/db/mod.rs`), but `/api/runs*` handlers query `sqlite::try_pool()` directly and gate Postgres via `require_sqlite_for_runs` (Issue #207). | **Unified `StorageBackend` for Run Lifecycle**: Migrate all `/api/runs*` and `run_nodes` queries off `sqlite::try_pool()` onto the backend trait; add Postgres advisory-lock coordination and remove `require_sqlite_for_runs`. |
| **Compute Dispatch** | Local `tokio` process executor + CLI wrapper scripts (`sbatch`/`qsub`/`bsub` in `crates/oxo-flow-core/src/backend/cluster.rs`); `[plugins]` is parsed but inert (`plugin.rs`). | **Cloud-Native `ExecutorBackend` Drivers**: Implement native Kubernetes Job/Pod controller (`kube-rs`), AWS Batch, and Google Cloud Batch adapters under `crates/oxo-flow-core/src/backend/`, plus active plugin dispatch. |
| **Object Storage & I/O** | `StorageResolver` (`s3.rs`, `gcs.rs`) and `stage_remote_io` (`executor/staging.rs`) perform eager whole-file local downloads and post-run uploads. | **Streaming & Virtualized I/O**: Add FIFO/pipe streaming (`s3://` / `gs://` direct stream into tool stdin/stdout), range-fetched index probing, and pre-signed URL / FUSE mount pass-through for cloud workers. |
| **Visual Design & UX** | Disjoint CLI text, basic SVG reports, and separated web pages. | **Metropolis Cross-Platform Design System**: Metro-DAG interactive subway map, unified typography and color tokens across CLI TUI, Desktop, Web SPA, and VS Code. |
| **Security & Governance** | Scoped tool guards, wildcard injection checks, sensitive-value masking, basic role passwords + ORCID OAuth. | **Enterprise Compliance Plane**: Fine-grained project/workspace RBAC, OIDC/SAML SSO, and an append-only, hash-chained audit log satisfying regulated clinical environments (FDA 21 CFR Part 11 / CLIA). |

---

## 5. Multi-Stage Development Roadmap & Acceptance Gates

Because this roadmap initiates a comprehensive, multi-stage engineering effort, execution is structured into four sequential stages. Each stage is decomposed into self-contained waves that must keep `make ci` green and pass an explicit stage exit gate.

```text
Stage 1                           Stage 2                           Stage 3                           Stage 4
Hardware-Aware Cloud Core &       BioTypes & Metro-DAG              Transpiler & Autonomous           Large-Scale Benchmark,
Decoupled Persistence             Visual Design System              Self-Healing Continuum            HCI Study & Publication
  │                                 │                                 │                                 │
  ├─► Wave 1.1: Postgres Runs       ├─► Wave 2.1: BioTypes Lattice    ├─► Wave 3.1: nf-core Transpiler  ├─► Wave 4.1: Multi-Omics Bench
  ├─► Wave 1.2: Cloud Backends      ├─► Wave 2.2: Header Inspectors   ├─► Wave 3.2: Causal Graph        ├─► Wave 4.2: Cohort & HW Study
  ├─► Wave 1.3: Streaming I/O       ├─► Wave 2.3: 50+ SCI Preflight   ├─► Wave 3.3: 1-Click Self-Heal   ├─► Wave 4.3: User Study & SUS
  └─► Wave 1.4: NUMA & NVMe Tier    └─► Wave 2.4: Metropolis Design   └─► Wave 3.4: IDE ↔ Web Continuum └─► Wave 4.4: Nature Package
```

### Stage 1: Hardware-Aware Cloud Core, Decoupled Persistence & Streaming I/O

- **Wave 1.1: Postgres-Native Run Lifecycle (Resolving Issue #207)**
  - Extend `StorageBackend` in `crates/oxo-flow-web/src/infra/db/mod.rs` to cover full run creation, cursor pagination, `run_nodes` state transitions, and checkpoint synchronization in both `sqlite.rs` and `postgres.rs`.
  - Refactor `crates/oxo-flow-web/src/domains/execution/handlers.rs` and `executor.rs` to operate exclusively through `StorageBackend`, eliminating direct `sqlite::try_pool()` coupling and removing the `require_sqlite_for_runs` middleware gate in `server.rs`.
- **Wave 1.2: Native Cloud & Kubernetes `ExecutorBackend` Implementations**
  - Implement Kubernetes, AWS Batch, and Google Cloud Batch drivers implementing `ExecutorBackend` (`crates/oxo-flow-core/src/backend/mod.rs`), reusing `ScheduledPlan` and `BackendDriver` (`driver.rs`).
  - Wire `[engine] tmp_dir` / `OXO_FLOW_TMPDIR` and container volume mounts consistently across local, HPC cluster, and cloud batch executors.
- **Wave 1.3: Streaming & Zero-Copy Object Storage**
  - Extend `StorageBackend` (`crates/oxo-flow-core/src/storage/mod.rs`) and `staging.rs` with `staging_mode = "eager" | "stream" | "PassThrough"`, supporting range-read header inspection and FIFO pipe streaming so large BAM/FASTQ objects do not exhaust local worker disks.
- **Wave 1.4: NUMA-Aware Affinity & Multi-Tier Scratch Management**
  - Introduce `crates/oxo-flow-core/src/hardware/` to detect physical NUMA nodes, socket layouts, and memory buses.
  - Implement memory-node affinity binding and an automated 4-tier scratch ladder (RAM to NVMe to Parallel FS to Cloud) integrated with the runtime disk-pressure ladder.
- **Stage 1 Exit Gate**:
  - `web_oauth_and_pg_contract` and full `/api/runs*` integration suite pass against both SQLite and PostgreSQL backends; mock and live container tests verify Kubernetes and cloud batch job lifecycle through `BackendDriver`; NUMA binding benchmarks demonstrate $>25\%$ latency reduction on multi-socket alignment tasks.

### Stage 2: BioTypes Semantic Typing & Metro-DAG Visual Design System

- **Wave 2.1: BioTypes Grammar, Schema & Type Unifier**
  - Add `[types.<name>]` and `[rules.types]` declarations to `crates/oxo-flow-core/src/config/model.rs`, `known_keys.rs`, and `docs/schema/oxoflow-v1.schema.json` (regenerating VS Code extension completions via `npm run generate`).
  - Implement `crates/oxo-flow-core/src/types/` for affine subtyping, attribute unification (`format`, `sort_order`, `index`, `reference_build`, `strandedness`, `read_groups`), and `TYPE-*` diagnostics across `WorkflowDag`.
- **Wave 2.2: Binary Genomic Header Introspection**
  - Implement lightweight header readers in `oxo-flow-core` to inspect local and remote (range-fetched) BAM/CRAM/VCF/FASTQ headers prior to execution, catching build mismatches (`GRCh37` vs `GRCh38`), missing `@RG` tags, and missing index files before compute starts.
- **Wave 2.3: Generalized Hybrid Re-Entry & Expanded `SCI-*` Preflight**
  - Generalize `crates/oxo-flow-core/src/reentry.rs` so `checkpoint = true` rules can emit arbitrary named `[[values]]` axes with full replay determinism.
  - Expand `crates/oxo-flow-core/src/scientific_preflight.rs` from 5 checks to 50+ literature-backed checks across genomics, bulk/single-cell transcriptomics, epigenomics, and metagenomics, with full documentation in `docs/guide/src/`.
- **Wave 2.4: Metropolis Design System & Metro-DAG Studio**
  - Formalize design tokens, typography, and biological color hierarchies shared across Web, Desktop, Terminal TUI, and VS Code.
  - Overhaul the React Flow canvas into a high-performance Metro transit renderer with dynamic station collapsing and telemetry-driven transit-line pulsing.
- **Stage 2 Exit Gate**:
  - $>95\%$ of injected domain-mismatch faults (unsorted BAM, mismatched contig naming, missing indices, invalid cohort sizes) are caught statically during `oxo-flow validate` / `dry-run` before spawning a single child process; Metro-DAG renders $10^4$ nodes at 60fps with zero layout stutter.

### Stage 3: Universal Ecosystem Transpiler & Autonomous Self-Healing Continuum

- **Wave 3.1: Nextflow DSL2 & Snakemake Transpiler (`oxo-flow import`)**
  - Build a deterministic parser and transpiler converting `nf-core` modules/subworkflows and Snakemake rules into `.oxoflow` workflows with automatic `[rules.types]` annotations derived from the embedded knowledge base.
- **Wave 3.2: Causal Diagnostic Graph**
  - Unify `crates/oxo-flow-web/src/domains/execution/diagnostics.rs` and `oxo-flow-core` failure telemetry into a shared `oxo-flow-core::diagnostics` module that correlates exit codes, signals, stderr patterns, peak RSS/CPU (`BenchmarkRecord`), and input header metadata.
- **Wave 3.3: Bounded Autonomous Remediation & 1-Click Fix Actions**
  - Upgrade the recovery runtime so infrastructure, memory, and formatting faults produce verified graph repairs (resource scaling, adapter rule insertion) invokable via 1-click in the UI or 1-key in the CLI.
- **Wave 3.4: Seamless Cross-Platform Workflow Continuum**
  - Unify project context, live SSE event streams, and checkpoint state between VS Code, CLI, Desktop, and Web Server, allowing seamless handover between local drafting and cluster monitoring.
- **Stage 3 Exit Gate**:
  - Automated transpilation and validation of a curated benchmark set of canonical `nf-core` modules and Snakemake workflows; $>70\%$ autonomous recovery rate on a fault-injection testbed without silent output corruption; sub-second live handover between CLI and Web UI.

### Stage 4: Large-Scale Empirical Benchmark, Human-Factors Evaluation & Manuscript Package

- **Wave 4.1: Multi-Omics Synthesis & Verification Benchmark (`eval/frontier/`)**
  - Expand `eval/` and `eval/frontier/` into a comprehensive, multi-hundred-intent benchmark comparing unconstrained Nextflow/Snakemake LLM synthesis against `oxo-flow`'s type-directed compiler loop across pass@k, silent-error detection, and token cost.
- **Wave 4.2: Population-Scale Cohort & Hardware Scalability Study**
  - Execute end-to-end germline, somatic, and transcriptomic pipelines at cohort scale across HPC and cloud backends, quantifying scheduling overhead, NUMA scaling efficiency, peak memory accuracy, storage I/O savings, and bit-level/semantic provenance reproducibility.
- **Wave 4.3: Quantitative Human-Factors & User Experience Study**
  - Conduct formal usability evaluations with academic and clinical bioinformatics practitioners measuring cognitive task completion times, debugging efficiency, and System Usability Scale (SUS) scores against Nextflow and Snakemake.
- **Wave 4.4: Reproducible Manuscript & Artifact Release**
  - Package all benchmark datasets, fault-injection harnesses, transpiler outputs, and evaluation scripts as an open, one-command reproducible artifact accompanying the manuscript submissions.

---

## 6. Quantifiable Evaluation Targets

| Evaluation Dimension | Target Threshold | Baseline Reference (Nextflow / Snakemake) |
| :--- | :--- | :--- |
| **Pre-Execution Domain Error Detection** | $>95\%$ of format, index, sort-order, and reference-build mismatches caught at `validate` / `dry-run` time | Near $0\%$ (paths treated as untyped strings; failures surface mid-run) |
| **AI Workflow Synthesis First-Run Pass Rate** | $>85\%$ all-gates pass across multi-step multi-omics intents | $<25\%$ under unconstrained Groovy/Python generation |
| **Autonomous Fault Recovery Rate** | $>70\%$ of transient resource, scratch, and index/sort faults remediated automatically with full audit trail | Manual log inspection and script editing required |
| **Hardware Scheduling & Memory Efficiency** | $>25\%$ lower memory bus latency via NUMA pinning; $<50\text{ ms}$ DAG compilation for $10^4$ rules | High cross-socket thrashing; seconds to minutes of JVM/Python resolution |
| **Storage Staging Footprint on Cloud Objects** | $50\text{--}80\%$ reduction in local scratch disk footprint via streaming and header range-probing | Full-file local staging unless specialized commercial plugins are configured |
| **User Experience & Cognitive Task Latency** | $<10\text{ ms}$ CLI startup; $<20\text{ ms}$ IDE diagnostic hovers; $>80$ SUS usability score | CLI lag $>500\text{ ms}$; manual text log digging; no unified design system |

---

## 7. Risk Analysis & Architectural Guardrails

- **Risk 1: Ecosystem Inertia Around `nf-core` and Snakemake**
  - *Mitigation*: Rather than requiring manual rewrites, Wave 3.1 provides automated transpilation (`oxo-flow import`) of existing `nf-core` modules and Snakemake rules, immediately upgrading legacy workflows with static `BioTypes` verification and self-healing.
- **Risk 2: False Positives in Static Biological Type Checking**
  - *Mitigation*: `BioTypes` adopts gradual typing—rules without `[rules.types]` retain existing path-matching semantics, while typed rules support explicit `--allow-type-coercion` overrides logged in the checkpoint provenance record.
- **Risk 3: Memory & Checkpoint Contention at $10^5$-Rule Scale**
  - *Mitigation*: Preserve `oxo-flow`'s compact `petgraph` representation, delta-append checkpoint journals before atomic compaction, and stream remote I/O rather than buffering whole artifacts in memory.
- **Risk 4: Non-Determinism from External LLM Providers**
  - *Mitigation*: The LLM never executes unverified code or mutates state directly; every synthesized or repaired `.oxoflow` artifact must pass the deterministic Rust compiler, `BioTypes` unifier, and shell-safety sanitizer before dispatch.

---

## 8. Governance & Development Workflow

All implementation work under this roadmap must strictly adhere to the repository's engineering invariants (`AGENTS.md`):

- **Incremental, Non-Breaking Waves**: Every wave lands as focused, reviewable PRs with unit and integration tests; `make ci` (`cargo fmt`, `cargo clippy -- -D warnings`, `cargo build`, `cargo test`, `extension-test`) must pass at every step.
- **Single Source of Truth**: Any schema or CLI surface addition must simultaneously update `docs/schema/oxoflow-v1.schema.json`, VS Code generated completion data, `AGENTS.md`, `README.md`, and `docs/guide/src/`.
- **Zero Silent Fallbacks**: Every new diagnostic (`TYPE-*`, `SCI-*`, `D00*`) ships with deterministic tests and user-facing remediation guidance.
