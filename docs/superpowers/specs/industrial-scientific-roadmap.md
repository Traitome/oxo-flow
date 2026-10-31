# Autonomous, Formally-Verified Scientific Workflows: Research Proposal & Industrial Roadmap

> **Target**: Dual attainment of an **industrial-grade production engine** (petabyte-scale, cloud-native, multi-tenant, zero silent errors) and a **scientific breakthrough publication in *Nature Biotechnology* or *Nature Methods***.
>
> **Core Premise**: Disrupting Nextflow and Snakemake cannot be achieved by minor scheduling speedups alone. It requires a fundamental paradigm shift: transitioning scientific workflow management from untyped, fragile glue scripts to **formally-verified, biologically-typed, AI-synthesized autonomous computational ecosystems**.
>
> **Execution Model**: This specification establishes the architectural foundation for a long-term, multi-stage engineering and research program. Progression across stages is governed by strict, verifiable engineering and scientific acceptance gates rather than fixed calendar schedules.
>
> **Companion Implementation Plan**: [`docs/superpowers/plans/industrial-scientific-implementation-plan.md`](../plans/industrial-scientific-implementation-plan.md)

---

## 1. Executive Summary & Scientific Vision

Bioinformatics workflow management has evolved through three distinct architectural generations:

- **First Generation (Ad-Hoc Scripting)**: Unstructured shell scripts and Makefiles lacking environment isolation, portable scheduling, and provenance guarantees.
- **Second Generation (Human-Authored Dataflow & Rule Engines)**: Declarative and reactive workflow managers (**Snakemake** [*Nature Biotechnology*; *Nature Methods*], **Nextflow** [*Nature Biotechnology*], **nf-core** [*Nature Biotechnology*], **CWL/WDL**). These systems standardized containerization and cluster dispatch, yet rely on untyped file path strings and embedded general-purpose scripting languages (Python/Groovy) that resist static formal verification.
- **Third Generation (AI-Native, Formally-Verified Scientific Workflows)**: Humans specify high-level scientific intent and experimental constraints; AI agents synthesize candidate pipelines into a canonical intermediate representation; a formal compiler proves biological and semantic soundness prior to execution; and an autonomous runtime adapts to data artifacts and infrastructure faults.

### The Central Paradox of AI in Computational Biology

While Large Language Models (LLMs) substantially accelerate pipeline authoring, unconstrained code generation introduces a severe scientific hazard: **plausible hallucinations and silent domain incompatibilities**. An LLM-generated workflow may parse cleanly and form an acyclic graph, yet fail catastrophically after hours of cluster compute—or worse, produce silently corrupted biological conclusions—due to domain-level violations (e.g., passing a queryname-sorted or unindexed BAM lacking `@RG` headers into `gatk HaplotypeCaller`, mixing `hg19` and `GRCh38` coordinates, or running `Mutect2` in paired mode without a matched normal).

`oxo-flow` resolves this contradiction by uniting four pillars:

1. **A Canonical Declarative Intermediate Representation (`.oxoflow` TOML)** in Rust, designed for deterministic static analysis and machine synthesis (`crates/oxo-flow-core/src/config/`, `dag.rs`).
2. **BioTypes**: A compile-time semantic biological type system with affine subtyping and fast binary header introspection.
3. **Causal Autonomous Self-Healing**: A closed-loop runtime monitor combining deterministic failure signatures, resource telemetry, and biological quality metrics to remediate faults with provenance guarantees.
4. **Cloud-Native Enterprise Substrate**: A decoupled, multi-tenant execution plane supporting native Kubernetes, cloud batch APIs, and zero-copy streaming object storage.

---

## 2. Scientific Innovation: The Nature-Caliber Contribution

To meet the methodological rigor of *Nature Methods* and the translational, community-scale requirements of *Nature Biotechnology*, `oxo-flow` advances four coupled innovations:

```text
┌───────────────────────────────────────────────────────────────────────────────┐
│                 AI-Native Verified Scientific Workflow Engine                 │
└───────────────────────────────────────┬───────────────────────────────────────┘
                                        │
        ┌───────────────────────────────┼───────────────────────────────┐
        ▼                               ▼                               ▼
┌───────────────────────────┐ ┌───────────────────────────┐ ┌───────────────────────────┐
│  Biological Semantic      │ │  Type-Directed AI         │ │  Autonomous Causal        │
│  Type System (BioTypes)   │ │  Synthesis & Feedback     │ │  Self-Healing Runtime     │
│  • Format & Build Enforce │ │  • Intent Contract        │ │  • Multilayer Diagnosis   │
│  • Affine Subtyping       │ │  • Formal Type Proof      │ │  • Dynamic Remediation    │
│  • Zero Silent Errors     │ │  • Zero-Shot Generation   │ │  • Bounded Auto-Recompute │
└───────────────────────────┘ └───────────────────────────┘ └───────────────────────────┘
                                        │
        ┌───────────────────────────────┴───────────────────────────────┐
        ▼                                                               ▼
┌───────────────────────────┐                               ┌───────────────────────────┐
│  nf-core / Snakemake      │                               │  Cryptographic Semantic   │
│  Universal Transpiler     │                               │  Provenance Engine        │
│  • AST-Based Translation  │                               │  • Hardware Invariance    │
│  • Automated Typing       │                               │  • Microarchitecture Proof│
│  • Zero-Cost Cold Start   │                               │  • Tamper-Evident Ledger  │
└───────────────────────────┘                               └───────────────────────────┘
```

### 2.1 BioTypes: A Formal Semantic Biological Type System

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

### 2.2 Type-Directed AI Synthesis with Bounded Soundness

Empirical evaluation in `eval/frontier/` demonstrates that unconstrained LLM prompting frequently hallucinates invalid DSL dialects (`E017`), whereas wiring `PipelineGenAgent` (`crates/oxo-flow-ai/src/agent/pipeline_gen.rs`) into the `Orchestrator` validation-feedback loop (`crates/oxo-flow-ai/src/agent/orchestrator.rs`) substantially raises pass rates. `oxo-flow` extends this architecture into formal **Type-Directed Synthesis**:

1. **Structured Task Contract & Curation**: Building on `TeamProfile` (`crates/oxo-flow-ai/src/agent/team.rs`), the deterministic Curator grounds candidate tools and versions against the embedded knowledge base (`bioconda_tools.jsonl`, `biotools_overlay.jsonl`, `nfcore_modules.jsonl`, `edam_terms.jsonl`).
2. **Compiler-in-the-Loop Verification**: Candidate `.oxoflow` artifacts must pass schema validation, DAG cycle/dead-node analysis, `deep_check` (`D001`–`D004`), `scientific_preflight` (`SCI-*`), and `BioTypes` unification (`TYPE-*`) inside the orchestrator loop.
3. **Generalized Hybrid DAG Re-Entry**: Extending `crates/oxo-flow-core/src/reentry.rs` (`checkpoint = true`, `ReentryRecord`, `MAX_REENTRY_ROUNDS`) from `sample` and `pairs` discovery to arbitrary typed `[[values]]` axes (e.g., dynamically assembled contigs, demultiplexed barcodes, or filtered cell clusters) while keeping every re-entry wave statically verifiable and deterministically replayable.

### 2.3 Causal Graph-Driven Autonomous Self-Healing

Rather than treating all non-zero exit codes with blind retries, `oxo-flow` unifies the CLI `--ai-recover` path and the Web `DiagnosticsEngine` (`crates/oxo-flow-web/src/domains/execution/diagnostics.rs`) into a four-layer causal fault-isolation and remediation runtime:

- **Layer 1: Infrastructure & Resource Faults** (SIGKILL/137 OOM, walltime expiration, scratch exhaustion, transient node loss) $\to$ Driven by `Executor` peak RSS/CPU sampling (`crates/oxo-flow-core/src/executor/rss.rs`) and disk-pressure guards, the runtime scales memory/time allocations or reroutes scratch (`tmp_dir`) within policy bounds.
- **Layer 2: Data & Format Anomalies** (truncated BGZF blocks, unsorted intervals, missing tabix/BAI indices, contig prefix mismatch `chr1` vs `1`) $\to$ Automated synthesis and execution of localized repair/indexing bridge rules.
- **Layer 3: Cohort & Assay Quality Drift** (low mapping rate, adapter contamination, insufficient cohort size for VQSR) $\to$ Guided branch switching (e.g., transitioning from GATK VQSR to hard-filtering when cohort size $<30$, consistent with `SCI-VQSR-COHORT`).
- **Layer 4: Provenance-Preserving Audit**: Every autonomous modification is recorded in `.oxo-flow/ai_sessions/` and `CheckpointState` (`crates/oxo-flow-core/src/executor/checkpoint.rs`), ensuring no silent mutation of scientific methodology.

### 2.4 Universal Ecosystem Transpiler (`oxo-flow import`)

To eliminate the ecosystem cold-start barrier against Nextflow's `nf-core` and Snakemake's workflow catalog:

- **Module & Workflow AST Transpiler**: Parses Nextflow DSL2 (`process`, `workflow`, `container`, `conda`, `input`/`output` qualifiers) and Snakemake (`rule`, `input`, `output`, `params`, `conda`, `container`, `shell`) into canonical `.oxoflow` rules and `[[include]]` modules.
- **Automated BioType Lifting**: Infers `[rules.types]` contracts from EDAM ontology annotations in `nfcore_modules.jsonl` and `biotools_overlay.jsonl`, turning untyped community modules into statically type-checked `oxo-flow` pipelines.

---

## 3. Industrial-Grade Production Architecture

To transition from single-node/HPC-script execution to enterprise multi-tenant production, `oxo-flow` targets four foundational architectural upgrades grounded in the current codebase:

| Production Dimension | Current Implementation Baseline | Target Industrial Architecture |
| :--- | :--- | :--- |
| **Execution State Store** | `StorageBackend` trait exists (`crates/oxo-flow-web/src/infra/db/mod.rs`), but `/api/runs*` handlers query `sqlite::try_pool()` directly and gate Postgres via `require_sqlite_for_runs` (Issue #207). | **Unified `StorageBackend` for Run Lifecycle**: Migrate all `/api/runs*` and `run_nodes` queries off `sqlite::try_pool()` onto the backend trait; add Postgres advisory-lock coordination and remove `require_sqlite_for_runs`. |
| **Compute Dispatch** | Local `tokio` process executor + CLI wrapper scripts (`sbatch`/`qsub`/`bsub` in `crates/oxo-flow-core/src/backend/cluster.rs`); `[plugins]` is parsed but inert (`plugin.rs`). | **Cloud-Native `ExecutorBackend` Drivers**: Implement native Kubernetes Job/Pod controller (`kube-rs`), AWS Batch, and Google Cloud Batch adapters under `crates/oxo-flow-core/src/backend/`, plus active plugin dispatch. |
| **Object Storage & I/O** | `StorageResolver` (`s3.rs`, `gcs.rs`) and `stage_remote_io` (`executor/staging.rs`) perform eager whole-file local downloads and post-run uploads. | **Streaming & Virtualized I/O**: Add FIFO/pipe streaming (`s3://` / `gs://` direct stream into tool stdin/stdout), range-fetched index probing, and pre-signed URL / FUSE mount pass-through for cloud workers. |
| **Security & Governance** | Scoped tool guards, wildcard injection checks, sensitive-value masking, basic role passwords + ORCID OAuth. | **Enterprise Compliance Plane**: Fine-grained project/workspace RBAC, OIDC/SAML SSO, and an append-only, hash-chained audit log satisfying regulated clinical environments (FDA 21 CFR Part 11 / CLIA). |

---

## 4. Multi-Stage Development Roadmap & Acceptance Gates

Because this roadmap initiates a comprehensive, multi-stage engineering effort, execution is structured into four sequential stages. Each stage is decomposed into self-contained waves that must keep `make ci` green and pass an explicit stage exit gate.

```text
Stage 1                         Stage 2                         Stage 3                         Stage 4
Cloud-Native Core &             BioTypes & Static               Ecosystem Transpiler &          Large-Scale Benchmark &
Decoupled Persistence           Biological Verification         Autonomous Self-Healing         Publication Package
  │                               │                               │                               │
  ├─► Wave 1.1: Postgres Runs     ├─► Wave 2.1: BioTypes Schema   ├─► Wave 3.1: nf-core Importer  ├─► Wave 4.1: Multi-Omics Suite
  ├─► Wave 1.2: Cloud Backends    ├─► Wave 2.2: Header Inspectors ├─► Wave 3.2: Causal Graph      ├─► Wave 4.2: Cohort Validation
  └─► Wave 1.3: Streaming Storage └─► Wave 2.3: 50+ SCI Rules     └─► Wave 3.3: Bounded Auto-Fix  └─► Wave 4.3: Paper & Artifacts
```

### Stage 1: Cloud-Native Core, Decoupled Persistence & Streaming I/O

- **Wave 1.1: Postgres-Native Run Lifecycle (Resolving Issue #207)**
  - Extend `StorageBackend` in `crates/oxo-flow-web/src/infra/db/mod.rs` to cover full run creation, cursor pagination, `run_nodes` state transitions, and checkpoint synchronization in both `sqlite.rs` and `postgres.rs`.
  - Refactor `crates/oxo-flow-web/src/domains/execution/handlers.rs` and `executor.rs` to operate exclusively through `StorageBackend`, eliminating direct `sqlite::try_pool()` coupling and removing the `require_sqlite_for_runs` middleware gate in `server.rs`.
- **Wave 1.2: Native Cloud & Kubernetes `ExecutorBackend` Implementations**
  - Implement Kubernetes, AWS Batch, and Google Cloud Batch drivers implementing `ExecutorBackend` (`crates/oxo-flow-core/src/backend/mod.rs`), reusing `ScheduledPlan` and `BackendDriver` (`driver.rs`).
  - Wire `[engine] tmp_dir` / `OXO_FLOW_TMPDIR` and container volume mounts consistently across local, HPC cluster, and cloud batch executors.
- **Wave 1.3: Streaming & Zero-Copy Object Storage**
  - Extend `StorageBackend` (`crates/oxo-flow-core/src/storage/mod.rs`) and `staging.rs` with `staging_mode = "eager" | "stream" | "PassThrough"`, supporting range-read header inspection and FIFO pipe streaming so large BAM/FASTQ objects do not exhaust local worker disks.
- **Stage 1 Exit Gate**:
  - `web_oauth_and_pg_contract` and full `/api/runs*` integration suite pass against both SQLite and PostgreSQL backends; mock and live container tests verify Kubernetes and cloud batch job lifecycle through `BackendDriver`.

### Stage 2: BioTypes Semantic Type System & Domain Verification

- **Wave 2.1: BioTypes Grammar, Schema & Type Unifier**
  - Add `[types.<name>]` and `[rules.types]` declarations to `crates/oxo-flow-core/src/config/model.rs`, `known_keys.rs`, and `docs/schema/oxoflow-v1.schema.json` (regenerating VS Code extension completions via `npm run generate`).
  - Implement `crates/oxo-flow-core/src/types/` for affine subtyping, attribute unification (`format`, `sort_order`, `index`, `reference_build`, `strandedness`, `read_groups`), and `TYPE-*` diagnostics across `WorkflowDag`.
- **Wave 2.2: Binary Genomic Header Introspection**
  - Implement lightweight header readers in `oxo-flow-core` to inspect local and remote (range-fetched) BAM/CRAM/VCF/FASTQ headers prior to execution, catching build mismatches (`GRCh37` vs `GRCh38`), missing `@RG` tags, and missing index files before compute starts.
- **Wave 2.3: Generalized Hybrid Re-Entry & Expanded `SCI-*` Preflight**
  - Generalize `crates/oxo-flow-core/src/reentry.rs` so `checkpoint = true` rules can emit arbitrary named `[[values]]` axes with full replay determinism.
  - Expand `crates/oxo-flow-core/src/scientific_preflight.rs` from 5 checks to 50+ literature-backed checks across genomics, bulk/single-cell transcriptomics, epigenomics, and metagenomics, with full documentation in `docs/guide/src/`.
- **Stage 2 Exit Gate**:
  - $>95\%$ of injected domain-mismatch faults (unsorted BAM, mismatched contig naming, missing indices, invalid cohort sizes) are caught statically during `oxo-flow validate` / `dry-run` before spawning a single child process.

### Stage 3: Universal Ecosystem Transpiler & Causal Self-Healing

- **Wave 3.1: Nextflow DSL2 & Snakemake Transpiler (`oxo-flow import`)**
  - Build a deterministic parser and transpiler converting `nf-core` modules/subworkflows and Snakemake rules into `.oxoflow` workflows with automatic `[rules.types]` annotations derived from the embedded knowledge base.
- **Wave 3.2: Causal Diagnostic Graph**
  - Unify `crates/oxo-flow-web/src/domains/execution/diagnostics.rs` and `oxo-flow-core` failure telemetry into a shared `oxo-flow-core::diagnostics` module that correlates exit codes, signals, stderr patterns, peak RSS/CPU (`BenchmarkRecord`), and input header metadata.
- **Wave 3.3: Bounded Autonomous Remediation**
  - Upgrade the AI recovery loop (`--ai-recover` and web auto-remediation) so infrastructure and data-format faults trigger deterministic, type-checked graph repairs (resource scaling, adapter rule insertion) capped by explicit budget and provenance policies.
- **Stage 3 Exit Gate**:
  - Automated transpilation and validation of a curated benchmark set of canonical `nf-core` modules and Snakemake workflows; $>70\%$ autonomous recovery rate on a fault-injection testbed without silent output corruption.

### Stage 4: Large-Scale Empirical Benchmark & Manuscript Package

- **Wave 4.1: Multi-Omics Synthesis & Verification Benchmark (`eval/frontier/`)**
  - Expand `eval/` and `eval/frontier/` into a comprehensive, multi-hundred-intent benchmark comparing unconstrained Nextflow/Snakemake LLM synthesis against `oxo-flow`'s type-directed compiler loop across pass@k, silent-error detection, and token cost.
- **Wave 4.2: Population-Scale Cohort Execution Study**
  - Execute end-to-end germline, somatic, and transcriptomic pipelines at cohort scale across HPC and cloud backends, quantifying scheduling overhead, peak memory accuracy, storage I/O savings, and bit-level/semantic provenance reproducibility.
- **Wave 4.3: Reproducible Manuscript & Artifact Release**
  - Package all benchmark datasets, fault-injection harnesses, transpiler outputs, and evaluation scripts as an open, one-command reproducible artifact accompanying the manuscript submissions.

---

## 5. Quantifiable Evaluation Targets

| Evaluation Dimension | Target Threshold | Baseline Reference (Nextflow / Snakemake) |
| :--- | :--- | :--- |
| **Pre-Execution Domain Error Detection** | $>95\%$ of format, index, sort-order, and reference-build mismatches caught at `validate` / `dry-run` time | Near $0\%$ (paths treated as untyped strings; failures surface mid-run) |
| **AI Workflow Synthesis First-Run Pass Rate** | $>85\%$ all-gates pass across multi-step multi-omics intents | $<25\%$ under unconstrained Groovy/Python generation |
| **Autonomous Fault Recovery Rate** | $>70\%$ of transient resource, scratch, and index/sort faults remediated automatically with full audit trail | Manual log inspection and script editing required |
| **Engine Scheduling & Plan Overhead** | $<50\text{ ms}$ static DAG + type-check compilation for $10^4$ expanded rule instances | Seconds to minutes of JVM startup or Python DAG resolution |
| **Storage Staging Overhead on Cloud Object Stores** | $50\text{--}80\%$ reduction in local scratch disk footprint via streaming and header range-probing | Full-file local staging unless specialized commercial plugins are configured |

---

## 6. Risk Analysis & Architectural Guardrails

- **Risk 1: Ecosystem Inertia Around `nf-core` and Snakemake**
  - *Mitigation*: Rather than requiring manual rewrites, Wave 3.1 provides automated transpilation (`oxo-flow import`) of existing `nf-core` modules and Snakemake rules, immediately upgrading legacy workflows with static `BioTypes` verification and self-healing.
- **Risk 2: False Positives in Static Biological Type Checking**
  - *Mitigation*: `BioTypes` adopts gradual typing—rules without `[rules.types]` retain existing path-matching semantics, while typed rules support explicit `--allow-type-coercion` overrides logged in the checkpoint provenance record.
- **Risk 3: Memory & Checkpoint Contention at $10^5$-Rule Scale**
  - *Mitigation*: Preserve `oxo-flow`'s compact `petgraph` representation, delta-append checkpoint journals before atomic compaction, and stream remote I/O rather than buffering whole artifacts in memory.
- **Risk 4: Non-Determinism from External LLM Providers**
  - *Mitigation*: The LLM never executes unverified code or mutates state directly; every synthesized or repaired `.oxoflow` artifact must pass the deterministic Rust compiler, `BioTypes` unifier, and shell-safety sanitizer before dispatch.

---

## 7. Governance & Development Workflow

All implementation work under this roadmap must strictly adhere to the repository's engineering invariants (`AGENTS.md`):

- **Incremental, Non-Breaking Waves**: Every wave lands as focused, reviewable PRs with unit and integration tests; `make ci` (`cargo fmt`, `cargo clippy -- -D warnings`, `cargo build`, `cargo test`, `extension-test`) must pass at every step.
- **Single Source of Truth**: Any schema or CLI surface addition must simultaneously update `docs/schema/oxoflow-v1.schema.json`, VS Code generated completion data, `AGENTS.md`, `README.md`, and `docs/guide/src/`.
- **Zero Silent Fallbacks**: Every new diagnostic (`TYPE-*`, `SCI-*`, `D00*`) ships with deterministic tests and user-facing remediation guidance.
