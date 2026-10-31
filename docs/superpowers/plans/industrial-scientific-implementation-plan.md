# Rigorous Implementation Plan & Scientific Execution Protocol

> **For Engineers & Autonomous Agents**: This document defines the concrete implementation tasks, module boundaries, verification commands, and scientific evaluation standards required to execute the specification in [`docs/superpowers/specs/industrial-scientific-roadmap.md`](../specs/industrial-scientific-roadmap.md).
>
> **Execution Governance**: Work proceeds sequentially by Stage and Wave. No wave may be marked complete unless all unit tests, integration tests, fault-injection tests, documentation updates, and `make ci` checks pass with zero warnings.

---

## 1. Engineering & Scientific Governance Standards

Every code change, diagnostic rule, and benchmark experiment under this plan must satisfy both **Industrial Reliability Standards** and **Scientific Falsifiability Standards**.

### 1.1 Industrial Engineering Invariants

- **Memory Safety & Determinism**:
  - Preserve `#[forbid(unsafe_code)]` in `oxo-flow-core`.
  - All serialized checkpoint, manifest, and provenance structures (`CheckpointState`, `InputManifest`, `ReentryRecord`, `BioTypeContract`) must use deterministic key-ordered collections (`BTreeMap` / `BTreeSet`); `HashMap` iteration order must never reach persisted artifacts.
- **Backward Compatibility by Construction**:
  - All existing `.oxoflow` workflows (`examples/gallery/*.oxoflow` and test fixtures) must parse, validate, and execute with identical semantics when `[types]` and `[rules.types]` are omitted.
  - Variable and configuration expansion (`expand_to_fixed_point`, `expand_rule_patterns`) must always complete before deriving canonical rule fingerprints, DAG edges, or type bindings.
- **Collector Completeness**:
  - Validation, linting, deep-check (`D001`–`D004`), scientific preflight (`SCI-*`), and type-checking (`TYPE-*`) passes must accumulate all findings across a workflow without short-circuiting on the first error.
- **Documentation & Schema Lockstep**:
  - Every new TOML key or diagnostic code must ship in the same commit/PR with updates to `crates/oxo-flow-core/src/config/known_keys.rs`, `docs/schema/oxoflow-v1.schema.json`, regenerated VS Code completion data (`npm run generate` in `editors/vscode`), and user guide entries under `docs/guide/src/`.
- **Clinical & Regulatory Qualification (IQ/OQ/PQ)**:
  - Each stage exit gate maps directly to Installation Qualification (IQ), Operational Qualification (OQ), and Performance Qualification (PQ) test IDs in [`docs/VALIDATION_PROTOCOL.md`](../../VALIDATION_PROTOCOL.md) and change records in [`docs/CHANGE_CONTROL.md`](../../CHANGE_CONTROL.md).

### 1.2 Scientific Rigor & Falsifiability Protocol

- **Explicit Hypothesis Testing (Ablation Design)**:
  - Every scientific component (BioTypes verification, Curator grounding, compiler-in-the-loop feedback, causal self-healing) must be evaluated via controlled ablation arms against held-out intents in `eval/frontier/`:
    - **Arm A (Baseline — Unconstrained)**: Direct LLM generation in Nextflow DSL2 / Snakemake / naive `.oxoflow` without compiler feedback.
    - **Arm B (Syntax/Schema Loop Only)**: `PipelineGenAgent` with TOML syntax and DAG topological validation, but `BioTypes` and `SCI-*` disabled.
    - **Arm C (Full Verified Compiler Loop)**: `PipelineGenAgent` + `BioTypes` (`TYPE-*`) + `scientific_preflight` (`SCI-*`) + `deep_check` (`D00*`).
- **Statistical Reporting Requirements**:
  - All benchmark claims must report per-item mean, standard deviation, `pass@k` ($k \in \{1, 3, 5\}$), 95% non-parametric bootstrap confidence intervals, and paired two-sided significance tests (McNemar's test for binary pass/fail outcomes; Wilcoxon signed-rank test for token cost and latency) across multiple fixed random seeds.
- **Strict Hold-Out & Anti-Leakage Hygiene**:
  - Benchmark test intents (`eval/gold/*.csv`, `eval/frontier/intents.json`) must never be embedded into `crates/oxo-flow-ai/src/knowledge/*.jsonl` or system prompts. `make eval-lint` (`eval/scripts/check_gold.py`) enforces provenance integrity and detects benchmark-to-knowledge leakage.
- **Zero Silent Scientific Mutation**:
  - Autonomous remediation (`--ai-recover` or self-healing bridge rules) must never silently alter biological filtering thresholds or drop samples. Every automated action must be recorded as a cryptographic `Modification` entry in `.oxo-flow/ai_sessions/` and `CheckpointState`, and verified by `oxo-flow provenance verify`.

---

## 2. Stage 1 Implementation Plan: Cloud-Native Core & Decoupled Persistence

### Task 1.1: Unify `StorageBackend` for Run Lifecycle & Remove SQLite Gate (Issue #207)

**Objective**: Eliminate direct `sqlite::try_pool()` calls from the `/api/runs*` execution domain so PostgreSQL-backed team deployments support full run lifecycle execution and monitoring.

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
- [ ] **Step 4**: Remove `require_sqlite_for_runs` from `crates/oxo-flow-web/src/server.rs` and update `tests/web_oauth_and_pg_contract.rs` to assert `200 OK` / `201 Created` on `/api/runs*` under Postgres mode.
- [ ] **Step 5**: Verify with `cargo test --test web_integration --test web_oauth_and_pg_contract`.

---

### Task 1.2: Native Kubernetes & Cloud Batch `ExecutorBackend` Drivers

**Objective**: Extend `crates/oxo-flow-core/src/backend/` with direct API drivers for Kubernetes Jobs and Cloud Batch schedulers without relying on SSH or shell-parsed CLI wrappers.

**Files**:

- Modify: `crates/oxo-flow-core/src/backend/mod.rs`
- Create: `crates/oxo-flow-core/src/backend/k8s.rs`
- Create: `crates/oxo-flow-core/src/backend/aws_batch.rs`
- Create: `crates/oxo-flow-core/src/backend/gcp_batch.rs`
- Modify: `crates/oxo-flow-core/src/backend/driver.rs`
- Modify: `crates/oxo-flow-core/src/cluster.rs`
- Test: `tests/cluster_backend.rs`

**Implementation Checklist**:

- [ ] **Step 1**: Extend `ClusterBackend` enum in `crates/oxo-flow-core/src/cluster.rs` with `Kubernetes`, `AwsBatch`, and `GcpBatch` variants while keeping existing `Slurm`, `Pbs`, `Sge`, and `Lsf` behavior byte-identical.
- [ ] **Step 2**: Implement `ExecutorBackend` for `KubernetesExecutor` (`k8s.rs`), mapping `ScheduledRule` resources (`threads`, `memory_mb`, `gpu_spec`, `time_limit`, `tmp_dir`) into Kubernetes `Job` manifests with deterministic label selectors (`oxoflow.io/run-id`, `oxoflow.io/rule`).
- [ ] **Step 3**: Implement `ExecutorBackend` for `AwsBatchExecutor` (`aws_batch.rs`) and `GcpBatchExecutor` (`gcp_batch.rs`), mapping `BackendJobStatus` and `TerminalRecord` (exit code, wall-clock runtime, peak memory from container metrics) into `BackendDriver`.
- [ ] **Step 4**: Add mock-API transport tests in `tests/cluster_backend.rs` verifying submission, array batching, `cancel_inflight`, and accounting fallback when jobs leave the active queue.

---

### Task 1.3: Streaming & Range-Probed Object Storage I/O

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

## 3. Stage 2 Implementation Plan: BioTypes Semantic Type System & Static Verification

### Task 2.1: `BioTypes` Data Model, Subtyping Lattice & DAG Type Checker

**Objective**: Introduce compile-time biological type checking across DAG edges while preserving 100% compatibility with existing `.oxoflow` workflows.

**Files**:

- Create: `crates/oxo-flow-core/src/types/mod.rs`
- Create: `crates/oxo-flow-core/src/types/checker.rs`
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
| `TYPE-001` | Producer `format` incompatible with consumer `format` | `samtools view -b` (SAM $\to$ BAM), `bcftools view` (BCF $\to$ VCF) |
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

### Task 2.2: Sub-Millisecond Binary Header Introspectors

**Objective**: Verify on-disk and remote source files against declared `BioTypeSpec` invariants before dispatching rules.

**Files**:

- Create: `crates/oxo-flow-core/src/types/inspect.rs`
- Modify: `crates/oxo-flow-core/src/readiness.rs`
- Test: `crates/oxo-flow-core/src/types/inspect_tests.rs`

**Implementation Checklist**:

- [ ] **Step 1**: Implement bounded header parsers using initial 64 KiB reads (`StorageBackend::read_range(path, 0, 65536)`):
  - **BGZF / GZIP**: Verify magic bytes (`1f 8b 08 04` and subfield `BC` for BGZF vs plain GZIP).
  - **BAM / SAM / CRAM**: Parse `@HD SO:<sort_order>`, `@SQ SN:<contig> LN:<len>` (detecting `chr`-prefix conventions and build fingerprints), and `@RG` tags (`ID`, `SM`, `PL`).
  - **VCF / BCF**: Parse `##fileformat=VCFv4.*`, `##reference=`, and `##contig=<ID=...,length=...>`.
  - **FASTQ**: Verify `@` record header framing, quality line length parity, and Phred+33 / Phred+64 ASCII ranges.
- [ ] **Step 2**: Wire `inspect_source_headers` into `oxo-flow validate --deep`, `oxo-flow test --deep`, and pre-execution source checks in `run`.
- [ ] **Step 3**: Benchmark header inspection latency in `benches/` to guarantee $<1\text{ ms}$ per local file and zero full-file reads.

---

### Task 2.3: Generalized Hybrid Re-Entry & 50+ `SCI-*` Preflight Rules

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
- [ ] **Step 3**: For every `SCI-*` rule, add positive and negative unit tests in `scientific_preflight.rs` and document its biological rationale and literature citation in `docs/guide/src/`.

---

## 4. Stage 3 Implementation Plan: Ecosystem Transpiler & Causal Self-Healing

### Task 3.1: Nextflow DSL2 & Snakemake Transpiler (`oxo-flow import`)

**Objective**: Provide a deterministic CLI command `oxo-flow import <source> --from nextflow|snakemake -o <out.oxoflow>` that converts community modules and workflows into typed `.oxoflow` pipelines.

**Files**:

- Create: `crates/oxo-flow-core/src/transpile/mod.rs`
- Create: `crates/oxo-flow-core/src/transpile/nextflow.rs`
- Create: `crates/oxo-flow-core/src/transpile/snakemake.rs`
- Create: `crates/oxo-flow-cli/src/commands/import.rs`
- Modify: `crates/oxo-flow-cli/src/main.rs`
- Test: `tests/transpile_integration.rs`

**Implementation Checklist**:

- [ ] **Step 1**: Implement lexical/structural parsers in `nextflow.rs` (extracting `process` blocks, `cpus`, `memory`, `time`, `container`, `conda`, `input:` tuple/path qualifiers, `output:` qualifiers, and `script:`/`shell:` templates) and `snakemake.rs` (extracting `rule`, `input`, `output`, `params`, `threads`, `resources`, `conda`, `container`, `shell`/`script`).
- [ ] **Step 2**: Translate wildcard/channel variables (`${task.cpus}` $\to$ `{threads}`, `${meta.id}` $\to$ `{sample}`, `{wildcards.sample}` $\to$ `{sample}`, `{input.bam}` $\to$ `{input[0]}`) and infer `[rules.types]` contracts by querying `nfcore_modules.jsonl` and `edam_terms.jsonl`.
- [ ] **Step 3**: Wire `oxo-flow import` into `crates/oxo-flow-cli/src/main.rs` (updating subcommand count and docs in `AGENTS.md`, `README.md`, and `docs/guide/src/`).
- [ ] **Step 4**: Validate transpiled outputs on 20+ canonical `nf-core` modules and Snakemake rules in `tests/transpile_integration.rs` using `oxo-flow validate` and `oxo-flow lint`.

---

### Task 3.2: Four-Layer Causal Fault-Isolation & Bounded Self-Healing Runtime

**Objective**: Unify runtime diagnostics across CLI and Web and enable deterministic, provenance-tracked autonomous repair.

**Files**:

- Create: `crates/oxo-flow-core/src/diagnostics.rs`
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

---

## 5. Stage 4 Implementation Plan: Benchmark Suite & Reproducible Manuscript Package

### Task 4.1: Multi-Omics Ablation & Fault-Injection Benchmark (`eval/frontier/`)

**Files**:

- Modify: `eval/frontier/intents.json`
- Modify: `eval/frontier/run_eval.py`
- Modify: `eval/frontier/rejudge.py`
- Create: `eval/frontier/fault_injection.py`
- Create: `eval/frontier/statistical_report.py`

**Implementation Checklist**:

- [ ] **Step 1**: Expand `eval/frontier/intents.json` across major omics modalities (WGS/WES germline & somatic, bulk RNA-seq, scRNA-seq/scATAC-seq, ChIP-seq/ATAC-seq, spatial transcriptomics, metagenomics, long-read ONT/PacBio).
- [ ] **Step 2**: Implement `fault_injection.py` to seed controlled biological and infrastructure faults (queryname-sorted BAMs, mismatched `chr` prefixes, unindexed VCFs, truncated BGZF blocks, under-provisioned memory limits, small cohort VQSR) and measure pre-execution detection vs. runtime self-healing rates.
- [ ] **Step 3**: Implement `statistical_report.py` to compute `pass@k`, bootstrap 95% confidence intervals, McNemar's test $p$-values, and token/compute cost breakdowns across Arms A, B, and C.

---

## 6. Verification Checklist Before Concluding Any Wave

Every PR implementing a task from this plan must verify:

1. `cargo fmt -- --check` exits `0`.
2. `cargo clippy --workspace --all-targets -- -D warnings` exits `0`.
3. `cargo test --workspace -- --test-threads=1` exits `0`.
4. `make extension-test` exits `0` (and `npm run generate` in `editors/vscode` produces zero git diff).
5. `python3 eval/scripts/check_gold.py` (`make eval-lint`) exits `0`.
6. All modified or added `.md` files pass `npx markdownlint-cli2` with zero `MD022`, `MD031`, `MD032`, `MD040`, or `MD058` errors.
