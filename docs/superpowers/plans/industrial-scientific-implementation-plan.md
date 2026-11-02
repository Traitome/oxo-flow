# Rigorous Implementation Plan & Scientific Execution Protocol

> **For Engineers & Autonomous Agents**: This document defines the concrete implementation tasks, module boundaries, verification commands, and scientific evaluation standards required to execute the specification in [`docs/superpowers/specs/industrial-scientific-roadmap.md`](../specs/industrial-scientific-roadmap.md).
>
> **Execution Governance**: Work proceeds sequentially by Stage and Wave across the **Hardware, Software, Design, and User Experience Quad-Pillar Matrix**. No wave may be marked complete unless all unit tests, integration tests, fault-injection tests, documentation updates, and `make ci` checks pass with zero warnings.
>
> **Audit Ledger**: [`docs/superpowers/audits/industrial-scientific-audit-ledger.md`](../audits/industrial-scientific-audit-ledger.md)

---

## 0. Execution Status and Completion Rules

This is a long-running, staged program. Existing code is not completion evidence.

### Maturity labels

- **Implemented and verified**: deterministic tests or measured evidence exercise the exact claim.
- **Implemented but unproven**: code exists, but production, cross-platform, or scientific evidence is absent.
- **Prototype**: a path works with known semantic, security, or operational gaps.
- **Simulation only**: model, fixture, seeded generator, or mock scheduler.
- **Planned / blocked**: not implemented or waiting on a higher-priority release blocker.

### Current baseline

| Workstream | Maturity | Blocking evidence gap |
| :--- | :--- | :--- |
| Workflow expansion, transform semantics, and declared-type admission | Implemented but unproven | Direct web TOML admission now shares the core checker; broad domain corpus, false-positive analysis, include resolution, and config-override parity remain |
| Web run lifecycle | Prototype / blocked | Atomic admission, immutable snapshots, truthful state transitions, and tenant isolation |
| PostgreSQL lifecycle | Implemented but unproven | Live parity, concurrency, crash recovery, and multi-instance behavior |
| Kubernetes and cloud batch | Prototype | Current adapters shell out to CLIs; no live lifecycle campaign |
| Range I/O and storage tiers | Implemented but unproven | Streaming correctness, pressure behavior, and measured footprint |
| NUMA affinity | Prototype | Named-hardware validation and measured performance |
| AI diagnosis and repair | Prototype | 7/10 proposals, 0/10 applied, 0/10 verified, 0/10 successful reruns in the current CLI simulation |
| Metro-DAG and editor UX | Prototype | Semantic round-trip, diff acceptance, accessibility, mobile, and scale evidence |
| Statistical, hardware, and cohort studies | Simulation or smoke test only | Matched measured campaigns with raw run-level provenance |
| Human-factors study | Simulation only | Ethics approval, recruitment, and real data |
| Manuscript package | Planned / blocked | No submission, acceptance, or independent replication |

### Completion rule

A checklist item may become `[x]` only when all of the following are attached:

1. The implementation commit or PR.
2. A regression test that fails on the prior defect.
3. The exact command and result used for verification.
4. Relevant negative and failure-path coverage.
5. Documentation and schema updates in the same change.
6. A maturity label justified by evidence.

Stage completion additionally requires every exit gate in the audit ledger.

---

## 1. Engineering, Hardware, Design & Scientific Governance Standards

Every code change, diagnostic rule, hardware interface, visual component, and benchmark experiment under this plan must satisfy four interlocked governance standards.

### 1.1 Industrial Engineering & Software Invariants

- **Memory Safety & Determinism**:
  - Preserve `#[forbid(unsafe_code)]` in `oxo-flow-core`.
  - All serialized checkpoint, manifest, and provenance structures (`CheckpointState`, `InputManifest`, `ReentryRecord`, `BioTypeContract`) must use deterministic key-ordered collections (`BTreeMap` / `BTreeSet`); `HashMap` iteration order must never reach persisted artifacts.
- **Backward Compatibility by Construction**:
  - All existing `.oxoflow` workflows (`examples/gallery/*.oxoflow` and test fixtures) must parse, validate, and execute with identical semantics when `[types]` and `[rules.types]` are omitted.
  - Variable and configuration expansion (`expand_to_fixed_point`, `expand_rule_patterns`) must always complete before deriving canonical rule fingerprints, DAG edges, or type bindings.
- **Collector Completeness**:
  - Validation, linting, deep-check (`D001`–`D005`), scientific preflight (`SCI-*`), and type-checking (`TYPE-*`) passes must accumulate all findings across a workflow without short-circuiting on the first error.
- **Documentation & Schema Lockstep**:
  - Every new TOML key or diagnostic code must ship in the same commit/PR with updates to `crates/oxo-flow-core/src/config/known_keys.rs`, `docs/schema/oxoflow-v1.schema.json`, regenerated VS Code completion data (`npm run generate` in `editors/vscode`), and user guide entries under `docs/guide/src/`.
- **Clinical & Regulatory Qualification (IQ/OQ/PQ)**:
  - Every claimed qualification requires a controlled traceability matrix
    linking each requirement and risk to Installation Qualification (IQ),
    Operational Qualification (OQ), and Performance Qualification (PQ) test
    IDs, implementation commit, evidence artifact, result, deviation, and
    approver. Change records follow
    [`docs/CHANGE_CONTROL.md`](../../CHANGE_CONTROL.md).
  - [`docs/VALIDATION_PROTOCOL.md`](../../VALIDATION_PROTOCOL.md) is a generic
    template. It does not currently provide project-specific mappings for
    the stage gates in this plan, and no IQ/OQ/PQ execution or sign-off is
    claimed. A project-specific matrix and approved protocol must exist
    before a qualification gate can pass.

### 1.2 Hardware Systems & Heterogeneous Compute Invariants

- **Topology-Aware Thread & Memory Binding**:
  - The core engine must discover host NUMA nodes and cache hierarchy (`crates/oxo-flow-core/src/hardware/numa.rs`). Compute-intensive alignment and assembly tasks must be pinned to local memory nodes; cross-socket memory access must be throttled or alerted when bandwidth drops below thresholds.
- **Hierarchical Scratch Safety**:
  - The 4-tier scratch ladder (Tier 0: tmpfs/RAM -> Tier 1: Local NVMe -> Tier 2: Shared Cluster FS -> Tier 3: Cloud Object Store) must be actively managed by `DiskPressureGuard`. The engine must never spill unbounded temporary files into the host root partition (`/tmp`).
- **Accelerator Isolation & Profiling**:
  - `GpuSpec` allocations must query accelerator capabilities (CUDA compute capability, VRAM capacity, TensorCore precision, Apple Silicon Unified Memory) before container launch, preventing fatal mid-run CUDA OOM or device-busy aborts.

### 1.3 Metropolis Design System & Human-Centered UX Invariants

- **Tokenized Single-Source Visual Grammar**:
  - Visual tokens (typography, spacing, borders, biological modality colors) are declared canonically and shared across the Web SPA (Tailwind / CSS vars), Desktop App (Wry/Tao), Terminal Progress TUI, and VS Code Extension.
- **Interaction Latency Budgets**:
  - Define p50/p95/p99 budgets for canvas pan/zoom, rule inspection, CLI dispatch, and IDE diagnostics. Measure on declared hardware; do not state target frame times as observed behavior.
- **Accessibility & Cognitive Clarity**:
  - All visual components must meet WCAG 2.1 AA contrast ratios. Color is never the sole carrier of status (always paired with iconic glyphs and text descriptors). Complex DAGs must support semantic multi-level collapse (`Rule` -> `Process` -> `Module`).
- **Semantics-Preserving Editing**:
  - Guided, grouped, or AI-assisted edits must preserve every supported workflow field. If a field cannot be represented safely, the edit must be blocked or shown as an explicit unsupported diff.
- **Explicit Change Acceptance**:
  - AI-generated content must never replace an editor buffer before acceptance. The displayed diff, applied patch, and verified content hash must refer to the same change.

### 1.4 Scientific Rigor & Falsifiability Protocol

- **Explicit Hypothesis Testing (Ablation Design)**:
  - Every scientific component (BioTypes verification, Curator grounding, compiler-in-the-loop feedback, causal self-healing, NUMA scheduling) must be evaluated via controlled ablation arms against held-out intents in `eval/frontier/`:
    - **Arm A (Baseline — Unconstrained)**: Direct LLM generation in Nextflow DSL2 / Snakemake / naive `.oxoflow` without compiler feedback.
    - **Arm B (Syntax/Schema Loop Only)**: `PipelineGenAgent` with TOML syntax and DAG topological validation, but `BioTypes` and `SCI-*` disabled.
    - **Arm C (Full Contract-Checked Compiler Loop)**: `PipelineGenAgent` + `BioTypes` (`TYPE-*`) + `scientific_preflight` (`SCI-*`) + `deep_check` (`D00*`).
- **Statistical Reporting Requirements**:
  - All benchmark claims must report per-item mean, standard deviation, `pass@k` ($k \in \{1, 3, 5\}$), 95% non-parametric bootstrap confidence intervals, and paired two-sided significance tests (McNemar's test for binary pass/fail outcomes; Wilcoxon signed-rank test for token cost, latency, and peak memory) across multiple fixed random seeds.
- **Strict Hold-Out & Anti-Leakage Hygiene**:
  - Benchmark test intents (`eval/gold/*.csv`, `eval/frontier/intents.json`) must never be embedded into `crates/oxo-flow-ai/src/knowledge/*.jsonl` or system prompts. `make eval-lint` (`eval/scripts/check_gold.py`) enforces provenance integrity and detects benchmark-to-knowledge leakage.
- **Zero Silent Scientific Mutation**:
  - Autonomous remediation (`--ai-recover` or self-healing bridge rules) must never silently alter biological filtering thresholds or drop samples. Every automated action must be recorded as a cryptographic `Modification` entry in `.oxo-flow/ai_sessions/` and `CheckpointState`, and verified by `oxo-flow provenance verify`.

### 1.5 Genomic Privacy and Data Governance

- **Data classification before ingestion**: Classify inputs as public, controlled-access,
  identifiable, or regulated before any staging or AI-assisted analysis. Controlled-access
  human genomic data must not be placed in a shared run directory without tenant isolation,
  access control, and a documented data-use agreement.
- **Minimization and de-identification limits**: Removing direct identifiers does not make
  genomic data anonymous. Do not claim de-identification or anonymization unless a documented
  threat model and relevant statistical disclosure controls support that claim.
- **AI egress boundary**: Treat prompts, retrieved context, tool results, logs, and generated
  patches as data-release events. A workflow containing private data must not reach an external
  provider, MCP server, fetch tool, or webhook without explicit policy and user approval.
- **Storage and retention**: Define encryption at rest and in transit, key ownership, backup
  scope, retention, deletion propagation, and audit-log retention before a team/HPC deployment.
- **Incident response**: Record the owner, escalation path, containment procedure, notification
  obligations, and forensic evidence plan for credential leakage, cross-tenant access, and
  unintended data egress.
- **Publication governance**: The manuscript artifact must state data access approvals,
  de-identification status, consent constraints, data-use terms, and whether any raw data can
  be redistributed.

---

## 2. Stage 1 Implementation Plan: Truthful Execution, Persistence & Safe Operations

### Task 1.0: Execution Truth and Isolation Release Blocker

**Maturity**: Planned / blocked

**Objective**: Close the Web/API release blockers before making any industrial, team, HPC, or clinical claim.

**Required implementation**:

- Atomic create, stage, persist, and spawn with a durable run-ledger entry.
- Unique, immutable per-run artifact snapshots.
- Environment isolation and an OS/container boundary for untrusted tenants.
- Minimal environment injection with secret-store routing.
- Monotonic compare-and-set lifecycle transitions.
- Atomic upload replacement and verified output invalidation.
- Safe remote archive extraction and verified artifact retrieval.
- Fail-closed session, audit, and revision persistence.

**Required tests**:

- Two-tenant adversarial filesystem, environment, network, CPU, and memory tests.
- Fault injection at every create/stage/persist/spawn boundary.
- Cancel versus completion, pause persistence failure, and resume without a process.
- Interrupted, oversize, and concurrent uploads.
- Remote pull failure, malicious archives, restart-safe cancellation, and missing-process reconciliation.

**Exit evidence**:

- The corresponding findings in the audit ledger move to **Implemented and verified**.
- Full `make ci` passes on the implementation commit.
- The verification artifact is linked from the PR.

---

### Task 1.1: Complete `StorageBackend` Run Lifecycle Parity (Issue #207)

**Maturity**: Implemented but unproven

**Objective**: Demonstrate that the existing storage abstraction supports the full run lifecycle on SQLite and PostgreSQL, then rename the historical SQLite-specific capability boundary.

**Files**:

- Modify: `crates/oxo-flow-web/src/infra/db/mod.rs`
- Modify: `crates/oxo-flow-web/src/infra/db/models.rs`
- Modify: `crates/oxo-flow-web/src/infra/db/sqlite.rs`
- Modify: `crates/oxo-flow-web/src/infra/db/postgres.rs`
- Modify: `crates/oxo-flow-web/src/domains/execution/handlers.rs`
- Modify: `crates/oxo-flow-web/src/executor.rs`
- Modify: `crates/oxo-flow-web/src/server.rs`
- Test: `tests/web_oauth_and_pg_contract.rs`, `tests/web_integration.rs`

**Interface Contract**:

```rust
#[async_trait]
pub trait StorageBackend: Send + Sync {
    // Existing user, pipeline, session, template, share, and audit methods...

    // Extended run lifecycle contract:
    async fn list_runs_cursor(
        &self,
        user_id: Option<&str>,
        cursor: Option<&str>,
        limit: usize,
        status_filter: Option<&str>,
    ) -> Result<CursorPage<models::RunRow>, String>;

    async fn upsert_run_node(&self, node: &models::RunNodeRow) -> Result<(), String>;
    async fn list_run_nodes(&self, run_id: &str) -> Result<Vec<models::RunNodeRow>, String>;
    async fn record_run_terminal(
        &self,
        run_id: &str,
        status: &str,
        exit_code: Option<i32>,
        finished_at: &str,
    ) -> Result<(), String>;
}
```

**Implementation Checklist**:

- [ ] **Step 1**: Add `RunNodeRow` and `CursorPage<T>` to `crates/oxo-flow-web/src/infra/db/models.rs` and extend `StorageBackend` in `mod.rs`.
- [ ] **Step 2**: Implement the extended trait methods in `sqlite.rs` and `postgres.rs` (using `INSERT ... ON CONFLICT DO UPDATE` for idempotent node state transitions and Postgres advisory locks for concurrent run-status updates).
- [ ] **Step 3**: Replace all direct `crate::infra::db::sqlite::try_pool()` queries in `domains/execution/handlers.rs` and `executor.rs` with calls through the global `StorageBackend` instance.
- [ ] **Step 4**: Rename `require_sqlite_for_runs`, `runs_gate_verdict`, and `RUNS_REQUIRE_SQLITE` only after tests prove the backend-agnostic contract; update AGENTS.md and user-guide references in lockstep.
- [ ] **Step 5**: Verify with `cargo test --test web_integration --test web_oauth_and_pg_contract`.
- [ ] **Step 6**: Add live PostgreSQL concurrency, crash-recovery, and multi-instance tests.

---

### Task 1.2: Native Kubernetes & Cloud Batch `ExecutorBackend` Drivers

**Maturity**: Prototype

**Objective**: Decide and document whether each cloud adapter is API-native or an explicit CLI wrapper, then qualify the claimed contract through live campaign testing. The current Kubernetes, AWS Batch, and GCP Batch paths invoke `kubectl`, `aws`, and `gcloud`; they must not be described as direct API drivers.

**Files**:

- Modify: `crates/oxo-flow-core/src/backend/mod.rs`
- Modify: `crates/oxo-flow-core/src/backend/k8s.rs`
- Modify: `crates/oxo-flow-core/src/backend/aws_batch.rs`
- Modify: `crates/oxo-flow-core/src/backend/gcp_batch.rs`
- Modify: `crates/oxo-flow-core/src/backend/driver.rs`
- Modify: `crates/oxo-flow-core/src/cluster.rs`
- Test: `tests/cluster_backend.rs`

**Implementation Checklist**:

- [x] **Step 1**: Extend `ClusterBackend` enum in `crates/oxo-flow-core/src/cluster.rs` with `Kubernetes`, `AwsBatch`, and `GcpBatch` variants while keeping existing `Slurm`, `Pbs`, `Sge`, and `Lsf` behavior byte-identical.
- [x] **Step 2** (superseded): dedicated `KubernetesExecutor` removed — `ClusterExecutor` (`backend/cluster.rs`) covers Kubernetes render + submit/poll/cancel/logs; the free `generate_k8s_*` generators in `backend/k8s.rs` remain the single manifest source (`src/cluster.rs:137`).
- [x] **Step 3** (superseded): dedicated `AwsBatchExecutor`/`GcpBatchExecutor` and `CloudCli` removed for the same reason — one production executor, free generator functions stay (`src/cluster.rs:140/143`).
- [ ] **Step 4**: Add mock transport tests for submission, array batching, cancellation errors, missing accounting, and per-element outcomes.
- [ ] **Step 5**: Execute live submit/queue/run/cancel/fail/retry/retrieve campaigns for every advertised backend.

---

### Task 1.3: Streaming & Range-Probed Object Storage I/O

**Maturity**: Implemented but unproven

**Objective**: Eliminate local scratch disk exhaustion on large remote inputs by adding byte-range reads (for header/index inspection) and streaming FIFO pipes to `StorageBackend`.

**Files**:

- Modify: `crates/oxo-flow-core/src/storage/mod.rs`
- Modify: `crates/oxo-flow-core/src/storage/local.rs`
- Modify: `crates/oxo-flow-core/src/storage/s3.rs`
- Modify: `crates/oxo-flow-core/src/storage/gcs.rs`
- Modify: `crates/oxo-flow-core/src/executor/staging.rs`
- Test: `tests/storage_invalidation.rs`

**Interface Contract**:

```rust
pub trait StorageBackend: Send + Sync {
    // Existing exists, download, upload, metadata, list_prefix methods...

    /// Read at most `len` bytes starting at `offset` without staging the full object.
    fn read_range(&self, path: &StoragePath, offset: u64, len: usize) -> Result<Vec<u8>>;
}
```

**Implementation Checklist**:

- [ ] **Step 1**: Add `read_range` to `StorageBackend` in `crates/oxo-flow-core/src/storage/mod.rs` and implement HTTP `Range: bytes=offset-(offset+len-1)` requests in `s3.rs` and `gcs.rs` (and `Seek` + `Read::take` in `local.rs`).
- [ ] **Step 2**: Add `StagingMode` (`Eager`, `StreamFifo`, `PassThrough`) to rule/workflow storage configuration and update `stage_remote_io` in `crates/oxo-flow-core/src/executor/staging.rs`.
- [ ] **Step 3**: Write unit and fault-injection tests verifying that a truncated stream or mid-stream network error fails the rule cleanly and invalidates partial outputs via `output_invalidation.rs`.

---

### Task 1.4: NUMA-Aware Affinity Scheduler & Multi-Tier Scratch Hierarchy

**Maturity**: Prototype

**Objective**: Interrogate physical hardware topology to pin memory-bound bioinformatics processes to local NUMA domains and cascade scratch allocations across a 4-tier storage hierarchy.

**Files**:

- Modify: `crates/oxo-flow-core/src/hardware/mod.rs`
- Modify: `crates/oxo-flow-core/src/hardware/numa.rs`
- Modify: `crates/oxo-flow-core/src/hardware/storage_tiers.rs`
- Modify: `crates/oxo-flow-core/src/scheduler.rs`
- Modify: `crates/oxo-flow-core/src/executor/process.rs`
- Test: `crates/oxo-flow-core/src/hardware/numa_tests.rs`

**Interface Contract**:

```rust
#[derive(Debug, Clone)]
pub struct HardwareTopology {
    pub numa_nodes: Vec<NumaNode>,
    pub total_memory_mb: u64,
    pub cache_l3_mb: u64,
}

#[derive(Debug, Clone)]
pub struct StorageTierLadder {
    pub tier0_ram_scratch: Option<PathBuf>,
    pub tier1_nvme_scratch: Option<PathBuf>,
    pub tier2_shared_scratch: Option<PathBuf>,
    pub tier3_cloud_remote: Option<String>,
}
```

**Implementation Checklist**:

- [ ] **Step 1**: Implement `HardwareTopology::discover()` using Linux sysfs / macOS sysctl to identify NUMA nodes, socket layouts, and memory locality.
- [ ] **Step 2**: Update `process.rs` to optionally set CPU affinity mask and memory policy (`numa_run_on_node` / `numa_set_membind`) for rules flagged `high_memory = true` or `numa_pin = true`.
- [ ] **Step 3**: Integrate `StorageTierLadder` with `disk_pressure_level`: if Tier 1 NVMe scratch reaches `Hold`, the executor dynamically redirects secondary scratch to Tier 2 shared scratch rather than aborting.
- [ ] **Step 4**: Pre-register the primary performance metric, then measure single-socket fallback and multi-socket behavior with repeated runs and confidence intervals. Do not describe a projected 25% gain as observed.

---

## 3. Stage 2 Implementation Plan: BioTypes Semantic Typing & Metro-DAG Visual Design System

### Task 2.1: `BioTypes` Data Model, Subtyping Lattice & DAG Type Checker

**Maturity**: Implemented but unproven

**Objective**: Harden and independently verify static biological-contract checking across DAG edges while preserving existing behavior when `[types]` and `[rules.types]` are omitted.

**Files**:

- Modify: `crates/oxo-flow-core/src/types/mod.rs`
- Modify: `crates/oxo-flow-core/src/types/checker.rs`
- Modify: `crates/oxo-flow-core/src/lib.rs`
- Modify: `crates/oxo-flow-core/src/rule.rs`
- Modify: `crates/oxo-flow-core/src/config/model.rs`
- Modify: `crates/oxo-flow-core/src/config/known_keys.rs`
- Modify: `docs/schema/oxoflow-v1.schema.json`
- Test: `crates/oxo-flow-core/src/types/tests.rs`

**Core Data Structures & Diagnostic Catalog**:

```rust
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BioTypeSpec {
    pub format: BioFormat, // Fastq, Sam, Bam, Cram, Vcf, Bcf, Bed, Gtf, Gff3, Fasta, H5ad, Rds, Parquet, Tsv
    #[serde(default)]
    pub sort_order: Option<SortOrder>, // Unsorted, Queryname, Coordinate
    #[serde(default)]
    pub index: Option<IndexKind>, // Bai, Csi, Crai, Tbi, Fai
    #[serde(default)]
    pub reference_build: Option<String>, // e.g., "GRCh38", "GRCh37", "T2T-CHM13"
    #[serde(default)]
    pub compression: Option<CompressionKind>, // None, Gzip, Bgzip, Zstd
    #[serde(default)]
    pub read_groups: Option<ReadGroupContract>,
    #[serde(default)]
    pub attributes: BTreeMap<String, String>,
}
```

| Diagnostic Code | Condition | Auto-Coercion Candidate |
| :--- | :--- | :--- |
| `TYPE-001` | Producer `format` incompatible with consumer `format` | `samtools view -b` (SAM -> BAM), `bcftools view` (BCF -> VCF) |
| `TYPE-002` | Producer `sort_order` does not satisfy consumer `sort_order` | `samtools sort -o {output} {input}` |
| `TYPE-003` | Consumer requires `index` (`BAI`/`TBI`/`CRAI`) not emitted by producer | `samtools index {input}` / `tabix -p vcf {input}` |
| `TYPE-004` | Producer `reference_build` conflicts with consumer `reference_build` | Hard compile error (requires explicit liftover/realignment) |
| `TYPE-005` | Consumer requires `@RG` fields (`ID`, `SM`, `PL`) absent from producer | `gatk AddOrReplaceReadGroups` / aligner `-R` flag hint |
| `TYPE-006` | Consumer requires `bgzip` block compression but producer emits plain `gzip` | `bgzip` recompression hint |

**Implementation Checklist**:

- [ ] **Step 1**: Define `BioTypeSpec`, `RuleTypesSpec` (`input: BTreeMap<String, String>`, `output: BTreeMap<String, String>`), and workflow-level `types: BTreeMap<String, BioTypeSpec>` in `crates/oxo-flow-core/src/types/mod.rs` and wire into `WorkflowConfig` and `Rule`.
- [ ] **Step 2**: Implement the partial order `BioTypeSpec::is_subtype_of(&self, expected: &BioTypeSpec) -> Result<(), Vec<TypeViolation>>` in `checker.rs`.
- [ ] **Step 3**: Integrate `check_workflow_types(&config, &dag)` into `validate`, `lint`, `dry-run`, and `run` preflight pipelines; emit structured `TYPE-001`..`TYPE-006` diagnostics with suggested coercion rules.
- [ ] **Step 4**: Update `known_keys.rs`, `docs/schema/oxoflow-v1.schema.json`, and run `npm run generate` in `editors/vscode` so `make ci` and `schema-drift` stay green.

---

### Task 2.2: Bounded Binary Header Introspectors

**Maturity**: Implemented but unproven

**Objective**: Verify on-disk and remote source files against declared `BioTypeSpec` invariants before dispatching rules.

**Files**:

- Modify: `crates/oxo-flow-core/src/types/inspect.rs`
- Modify: `crates/oxo-flow-core/src/deep_check.rs`
- Modify: `crates/oxo-flow-cli/src/commands/quality.rs`
- Benchmark: `crates/oxo-flow-core/benches/header_inspect_bench.rs`
- Test: `crates/oxo-flow-core/src/types/inspect_tests.rs`

**Implementation Checklist**:

- [ ] **Step 1**: Implement bounded header parsers using initial 64 KiB reads (`StorageBackend::read_range(path, 0, 65536)`):
  - **BGZF / GZIP**: Verify magic bytes (`1f 8b 08 04` and subfield `BC` for BGZF vs plain GZIP).
  - **BAM / SAM / CRAM**: Parse `@HD SO:<sort_order>`, `@SQ SN:<contig> LN:<len>` (detecting `chr`-prefix conventions and build fingerprints), and `@RG` tags (`ID`, `SM`, `PL`).
  - **VCF / BCF**: Parse `##fileformat=VCFv4.*`, `##reference=`, and `##contig=<ID=...,length=...>`.
  - **FASTQ**: Verify `@` record header framing, quality line length parity, and Phred+33 / Phred+64 ASCII ranges.
- [ ] **Step 2**: Wire `inspect_source_headers` into `oxo-flow validate --deep`, `oxo-flow test --deep`, and pre-execution source checks in `run`.
- [ ] **Step 3**: Benchmark header inspection latency, memory, malformed-input behavior, and read volume. Treat sub-millisecond latency as a hypothesis, not a guarantee.
- [ ] **Step 4**: Add remote range-read failure, stale index, truncated header, and adversarial parser tests.

---

### Task 2.3: Generalized Hybrid Re-Entry & 50+ `SCI-*` Preflight Rules

**Maturity**: Implemented but unproven

**Objective**: Generalize `reentry.rs` to arbitrary named `[[values]]` axes and expand `scientific_preflight.rs` to 50+ evidence-backed checks.

**Files**:

- Modify: `crates/oxo-flow-core/src/reentry.rs`
- Modify: `crates/oxo-flow-core/src/scientific_preflight.rs`
- Modify: `docs/guide/src/reference/diagnostics-engine.md`

**Implementation Checklist**:

- [ ] **Step 1**: Extend `ReentryRecord` and `Manifest` in `crates/oxo-flow-core/src/reentry.rs` with `values: BTreeMap<String, Vec<String>>` so a `checkpoint = true` rule can dynamically populate any declared `[[values]]` axis (e.g., `contig`, `cluster_id`, `chunk`) while preserving `MAX_REENTRY_ROUNDS` and deterministic resume replay.
- [ ] **Step 2**: Expand `crates/oxo-flow-core/src/scientific_preflight.rs` to cover 50+ curated checks across:
  - **Germline & Somatic Variant Calling**: `SCI-VQSR-COHORT`, `SCI-BQSR-NO-KNOWN-SITES`, `SCI-MUTECT2-TUMOR-ONLY`, `SCI-MUTECT2-NO-PON`, `SCI-GVCF-MISSING-ERC`, `SCI-STRELKA-UNINDEXED-BED`.
  - **Bulk & Single-Cell RNA-seq**: `SCI-FEATURECOUNTS-STRAND`, `SCI-STAR-SJDB-OVERHANG`, `SCI-SALMON-LIBTYPE`, `SCI-DESEQ2-MIN-REPLICATES`, `SCI-CELLRANGER-INTRONS`.
  - **Epigenomics & Metagenomics**: `SCI-MACS2-GENOME-SIZE`, `SCI-BISMARK-NON-DIRECTIONAL`, `SCI-KRAKEN2-CONFIDENCE`.
  - **DAG Concurrency & Dataflow**: `SCI-AGG-RACE`, `SCI-SORT-THREADS-MEM-PRODUCT`.
- [ ] **Step 3**: For every `SCI-*` rule, add positive and negative tests, document its assumptions, and attach a literature citation reviewed by a domain expert.
- [ ] **Step 4**: Measure false-positive and false-negative behavior on a curated corpus; document rules that remain heuristic.

---

### Task 2.4: Metropolis Cross-Platform Design System & Metro-DAG Studio Upgrade

**Maturity**: Prototype

**Objective**: Unify the visual language across CLI, Desktop, Web, and VS Code into the tokenized Metropolis design system and upgrade Metro-DAG rendering for $10^4$-node scalability.

**Files**:

- Modify: `crates/oxo-flow-web/src/theme/tokens.rs`
- Modify: `frontend/src/components/WorkflowCanvas.tsx`
- Modify: `frontend/src/components/MetroCanvas.tsx`
- Modify: `frontend/src/styles/theme.css`
- Modify: `crates/oxo-flow-cli/src/commands/output.rs`
- Modify: `editors/vscode/syntaxes/oxoflow.tmLanguage.json`

**Implementation Checklist**:

- [ ] **Step 1**: Define the Metropolis semantic color and typography tokens across biological modalities (Genomics, Transcriptomics, Epigenomics, Metagenomics) in `tokens.rs` and mirror them into `theme.css`.
- [ ] **Step 2**: Implement the Metro transit line layout algorithm in `MetroCanvas.tsx` using `@xyflow/react` and `d3-dag`, bundling parallel sample tracks into color-coded transit corridors with station interchange glyphs.
- [ ] **Step 3**: Implement dynamic semantic zoom: collapse rules by `Process` or `Module` at low zoom; expand to full rule instance parameters and live telemetry meters at high zoom.
- [ ] **Step 4**: Measure frame time, layout time, memory, interaction latency, and large-graph degradation on declared hardware.
- [ ] **Step 5**: Prove that grouped edits preserve all unsupported semantics or block grouping. Add keyboard-only navigation, WCAG checks, mobile viewport tests, and visual regression coverage.

---

## 4. Stage 3 Implementation Plan: Ecosystem Transpiler & Autonomous Self-Healing Continuum

### Task 3.1: Nextflow DSL2 & Snakemake Transpiler (`oxo-flow import`)

**Maturity**: Prototype

**Objective**: Provide a deterministic CLI command `oxo-flow import <source> --from nextflow|snakemake -o <out.oxoflow>` that converts community modules and workflows into typed `.oxoflow` pipelines.

**Files**:

- Modify: `crates/oxo-flow-core/src/transpile/mod.rs`
- Modify: `crates/oxo-flow-core/src/transpile/nextflow.rs`
- Modify: `crates/oxo-flow-core/src/transpile/snakemake.rs`
- Modify: `crates/oxo-flow-cli/src/commands/import.rs`
- Modify: `crates/oxo-flow-cli/src/main.rs`
- Test: `tests/transpile_integration.rs`

**Implementation Checklist**:

- [ ] **Step 1**: Implement lexical/structural parsers in `nextflow.rs` (extracting `process` blocks, `cpus`, `memory`, `time`, `container`, `conda`, `input:` tuple/path qualifiers, `output:` qualifiers, and `script:`/`shell:` templates) and `snakemake.rs` (extracting `rule`, `input`, `output`, `params`, `threads`, `resources`, `conda`, `container`, `shell`/`script`).
- [ ] **Step 2**: Translate wildcard/channel variables (`${task.cpus}` -> `{threads}`, `${meta.id}` -> `{sample}`, `{wildcards.sample}` -> `{sample}`, `{input.bam}` -> `{input[0]}`) and infer `[rules.types]` contracts by querying `nfcore_modules.jsonl` and `edam_terms.jsonl`.
- [ ] **Step 3**: Wire `oxo-flow import` into `crates/oxo-flow-cli/src/main.rs` (updating subcommand count and docs in `AGENTS.md`, `README.md`, and `docs/guide/src/`).
- [ ] **Step 4**: Validate transpiled outputs on 20+ canonical `nf-core` modules and Snakemake rules in `tests/transpile_integration.rs` using `oxo-flow validate` and `oxo-flow lint`.
- [ ] **Step 5**: Define supported versus unsupported constructs; unsupported constructs must fail explicitly rather than be silently dropped.
- [ ] **Step 6**: Add semantic-equivalence tests for parameters, channels, resources, containers, and failure behavior.

---

### Task 3.2: Four-Layer Causal Fault-Isolation & Bounded Self-Healing Runtime

**Maturity**: Prototype

**Objective**: Unify runtime diagnostics across CLI and Web and evaluate deterministic,
provenance-tracked bounded repair. No repair is described as verified until the exact patch
applies, the complete workflow validates, and a rerun produces checked outputs.

Current CLI simulation baseline: 8/10 detections, 7/10 attributed diagnoses, 7/10 repair proposals, 0/10 applied repairs, 0/10 verified repairs, and 0/10 successful reruns. Proposal quality and repair efficacy must be reported separately.

**Files**:

- Modify: `crates/oxo-flow-core/src/diagnostics.rs`
- Modify: `crates/oxo-flow-web/src/domains/execution/diagnostics.rs`
- Modify: `crates/oxo-flow-ai/src/agent/orchestrator.rs`
- Modify: `crates/oxo-flow-cli/src/commands/run/mod.rs`
- Test: `tests/self_healing_integration.rs`

**Implementation Checklist**:

- [ ] **Step 1**: Move the core pattern table and causal classifier into `crates/oxo-flow-core/src/diagnostics.rs` so both `oxo-flow-cli` and `oxo-flow-web` call the exact same diagnostic engine (honoring the *Reuse, never retype* invariant).
- [ ] **Step 2**: Enrich diagnostic evaluation inputs with `BenchmarkRecord` telemetry (`max_rss_mb`, `cpu_seconds`, `duration_ms`), exit signals (`137`/`SIGKILL`, `139`/`SIGSEGV`, `140`/`SIGXCPU`), and `BioTypes` header inspection results.
- [ ] **Step 3**: Define bounded, deterministic `RepairAction` variants:
  - `ScaleResources { memory_multiplier: f64, time_multiplier: f64 }` (for confirmed OOM / walltime kills, capped by system/cluster ceiling).
  - `InsertBridgeRule { adapter_rule: Rule }` (for `TYPE-002` unsorted BAM or `TYPE-003` missing index).
  - `RerouteScratch { tmp_dir: PathBuf }` (for `ENOSPC` on system temp volume).
- [ ] **Step 4**: Record every applied `RepairAction` inside `CheckpointState` and `.oxo-flow/ai_sessions/`, re-verify the modified DAG with `check_workflow_types`, and resume execution from the failed node.
- [ ] **Step 5**: Prove that the exact displayed patch is the exact applied patch, including a before/after content hash.
- [ ] **Step 6**: Refuse automatic changes to biological thresholds, cohort inclusion, or sample selection without explicit human approval.
- [ ] **Step 7**: Measure successful rerun and output verification separately from detection and proposal.

---

### Task 3.3: Cross-Platform Continuum & Bounded Remediation Actions

**Maturity**: Prototype

**Objective**: Target an unbroken interaction flow between VS Code, CLI Terminal, Desktop Wry shell, and Web SPA, with reviewed remediation actions. The current cross-surface and retry paths remain prototype-level until the audit-ledger UX blockers are closed.

**Files**:

- Modify: `crates/oxo-flow-cli/src/commands/run/mod.rs`
- Modify: `frontend/src/pages/MonitorReport.tsx`
- Modify: `editors/vscode/src/extension.ts`
- Modify: `crates/oxo-flow-web/src/domains/chat/handlers.rs`

**Implementation Checklist**:

- [ ] **Step 1**: When a rule fails under `run` or `run --cluster`, format terminal output with the exact root cause and emit a single-key prompt: `[R] Auto-scale memory & resume  [D] Debug command with AI  [A] Abort`.
- [ ] **Step 2**: In the Web and Desktop UI, render failed stations with a luminous red diagnostic halo and display a 1-click remediation card containing the exact planned repair diff.
- [ ] **Step 3**: Require explicit diff acceptance before AI text replaces editor content; validation failure must block stale success state; drafts and chat must be tenant-scoped and cleared on logout.
- [ ] **Step 4**: VS Code must validate before writing AI output and diagnose the displayed unsaved buffer, not an unrelated disk version.
- [ ] **Step 5**: Design and implement bi-directional deep linking: clicking an error in the Web dashboard opens the exact rule in the VS Code editor (`vscode://...`); add a future `oxo-flow run --web-preview` surface, which does not exist yet, to open the live Metro-DAG canvas in the desktop or browser shell.

---

## 5. Stage 4 Implementation Plan: Benchmark Suite, Human Factors & Reproducible Manuscript Package

### Task 4.0: Pre-Registration and Evidence Governance

**Maturity**: Planned / blocked

**Objective**: Freeze the confirmatory analysis before examining confirmatory outcomes.

**Required artifacts**:

- Primary and secondary outcomes, direction, and statistical models.
- Inclusion, exclusion, stopping, outlier, and missing-data rules.
- Fixed model, prompt, tool, seed, hardware, dataset, and budget manifests.
- Leakage audit and held-out-intent custody.
- Independent reviewer sign-off.
- A publication decision rule that permits a negative result.

### Task 4.1: Multi-Omics Ablation & Fault-Injection Benchmark (`eval/frontier/`)

**Maturity**: Simulation only

**Files**:

- Modify: `eval/frontier/intents.json`
- Modify: `eval/frontier/run_eval.py`
- Modify: `eval/frontier/rejudge.py`
- Modify: `eval/frontier/fault_injection.py`
- Modify: `eval/frontier/statistical_report.py`

**Implementation Checklist**:

- [ ] **Step 1**: Expand `eval/frontier/intents.json` across major omics modalities (WGS/WES germline & somatic, bulk RNA-seq, scRNA-seq/scATAC-seq, ChIP-seq/ATAC-seq, spatial transcriptomics, metagenomics, long-read ONT/PacBio).
- [ ] **Step 2**: Implement `fault_injection.py` to seed controlled biological and infrastructure faults (queryname-sorted BAMs, mismatched `chr` prefixes, unindexed VCFs, truncated BGZF blocks, under-provisioned memory limits, small cohort VQSR) and measure pre-execution detection vs. runtime self-healing rates.
- [ ] **Step 3**: Implement `statistical_report.py` to compute `pass@k`, bootstrap 95% confidence intervals, McNemar's test $p$-values, and token/compute cost breakdowns across Arms A, B, and C.
- [ ] **Step 4**: Run the pre-registered campaign and retain raw per-item outputs for independent reanalysis.

---

### Task 4.2: Direct Nextflow/Snakemake Execution Equivalence Benchmark

**Maturity**: Planned / blocked

**Objective**: Establish whether the same supported workflow produces
equivalent declared artifacts with comparable operational behavior on the same
host. This is the minimum evidence for any replacement claim; AI-authoring
results alone are insufficient.

**Required design**:

- One pre-registered workflow family with available, independently reviewed
  Nextflow, Snakemake, and `.oxoflow` implementations.
- Same inputs, reference data, container digests, CPU/memory limits, scheduler
  state, filesystem, and host.
- Randomized run order with repeated measurements and clean caches between arms.
- Declared-output equivalence, completion, wall time, peak RSS, CPU time, disk
  I/O, restart behavior, and failure-mode comparison.
- Explicit unsupported-construct coverage and exclusions, without deleting
  difficult cells.

**Implementation Checklist**:

- [ ] **Step 1**: Freeze workflow implementations, acceptance criteria, and
  equivalence checks before collecting comparison data.
- [ ] **Step 2**: Capture engine/runtime versions, container digests, commands,
  resource limits, host telemetry, and raw run-level outputs.
- [ ] **Step 3**: Run the randomized campaign and retain all failures and
  exclusions.
- [ ] **Step 4**: Publish only after independent reanalysis and a negative-result
  audit.

---

### Task 4.3: Large-Scale Cohort & Hardware Scaling Benchmark

**Maturity**: Simulation or smoke test only

**Objective**: Measure end-to-end performance gains across multi-socket NUMA systems, NVMe scratch tiering, and GPU acceleration on 10,000 public genomes.

**Files**:

- Modify: `eval/benchmarks/hardware_scaling.py`
- Modify: `eval/benchmarks/cohort_driver.py`

**Implementation Checklist**:

- [ ] **Step 1**: Run benchmark cohorts across three hardware configurations: (1) Default OS scheduler on dual-socket AMD EPYC; (2) `oxo-flow` NUMA-pinned execution; (3) NVMe tiered scratch vs. standard NFS.
- [ ] **Step 2**: Quantify memory interconnect bus traffic, L3 cache misses, peak memory footprint, and wall-clock execution time.
- [ ] **Step 3**: Generate candidate figures only after raw-data validation and independent analysis review. Model projections must remain visually and textually separate from measurements.

---

### Task 4.4: Quantitative Human-Factors & User Experience Study

**Maturity**: Simulation only

**Objective**: Empirically evaluate the System Usability Scale (SUS), cognitive debugging load, and time-to-first-working-pipeline across independent bioinformaticians.

**Files**:

- Modify: `eval/benchmarks/ux_eval_protocol.md`
- Modify: `eval/benchmarks/sus_survey_analysis.py`

**Implementation Checklist**:

- [ ] **Step 1**: Obtain ethics approval; pre-register tasks, randomization, counterbalancing, recruitment, exclusions, and analysis.
- [ ] **Step 2**: Measure time-to-first-pass, number of manual configuration edits, and error comprehension latency across user cohorts.
- [ ] **Step 3**: Compute SUS and NASA-TLX with confidence intervals and mixed-effects analysis; report all participants and null findings.

---

## 6. Verification Checklist Before Concluding Any Wave

Every PR implementing a task from this plan must verify:

1. `make ci` exits `0` for the exact implementation tree. This includes formatting,
   Clippy with warnings denied, build, tests, schema drift, version drift,
   dependency/security policy, secret scanning, Rust documentation, frontend lint,
   evaluation tests and lint, and the VS Code extension fast gate.
2. `make extension-integration` exits `0` when an editor, frontend, schema, or
   diagnostics contract changes.
3. Every new behavior has a regression test that fails against the prior
   implementation and passes against the change.
4. Every changed diagnostic, TOML key, CLI flag, environment variable, API shape,
   and schema entry is updated in the same PR across code, schema, generated editor
   data, `AGENTS.md`, and the relevant `docs/guide/src/` page.
5. Every scientific or performance claim has a raw artifact identifying the source
   commit, host, software versions, seeds, inputs, exclusions, commands, and
   analysis code.
6. All modified or added Markdown files satisfy repository formatting rules and
   contain no calendar dates in new roadmap, plan, protocol, or audit prose.
7. The final evidence is recorded with the exact command, exit status, commit, and
   artifact location; a previous run is not evidence for a later tree.
8. No stage gate is marked complete from a mock, synthetic, simulated, projected, or
   component-only result when the gate requires live or human evidence.
