# Static Plan + Pluggable Executors: ExecutorBackend, Unified Storage Invalidation, Checkpoint Re-entry

Design for [issue #78](https://github.com/Traitome/oxo-flow/issues/78).
Status: **approved** (2026-08-14, scope P1+P2+P3, plan A — core-first, additive).

## 1. Context and coordination

- **#77** (parity contract test matrix for run/dry-run) has SHIPPED in a peer session. This work
  must not modify `run.rs`'s local execution loop or `run_preview.rs`'s predicates for P1/P2;
  P3 needs a bounded, additive hook in the event loop (see §4.6).
- **#74** (cluster execution as a `run` backend) is owned by @andrewbudge, who has real SLURM
  access and implements `run --profile slurm`, the run directory, job arrays, and real-cluster
  validation. Per the maintainer's #74 comment: directive generation in `core/cluster.rs` **stays
  as-is** — the `ExecutorBackend` impl calls it, it is not moved. The mock-scheduler CI harness
  requested in #74 comment 3 and the shared job-ID parsing helper requested in #74 comment 5 are
  delivered here as part of P1. A reply on #74 will map the landed foundation to its phasing.
- Everything lands on `main` in phase commits, each with `make ci` green (fmt + clippy -D warnings
  + build + test + audit).

## 2. Core idea

The engine already computes a fully determined static plan (topological order, parallel groups,
per-rule resources, invalidation set). Executors must not re-understand the DAG: they map the same
plan onto a scheduler API. Static parts (plan computation, invalidation, checkpoint, dry-run
preview) keep a single implementation, which keeps the dry-run preview valid for any executor.

## 3. P1 — `ExecutorBackend` trait and cluster executor

New module `crates/oxo-flow-core/src/backend/` (feature-free, in core so CLI and future consumers
share it):

### 3.1 Types

```rust
/// One executable unit of the static plan: a fully resolved rule instance.
pub struct ScheduledRule {
    pub name: String,                 // expanded instance name (e.g. "align_auto-discovered_S1")
    pub workdir: PathBuf,
    pub shell_cmd: String,            // rendered shell command (build_execution_command + expansion)
    pub script: String,               // rendered submit script (shebang + directives + wrapped cmd)
    pub threads: u32,                 // effective_threads() resolved at plan time
    pub memory_mb: Option<u64>,       // effective_memory() resolved at plan time
    pub dependencies: Vec<String>,    // instance-level deps (DAG edges + depends_on resolution)
    pub wildcard_values: HashMap<String, String>, // {config.x} / {sample} / {group} bindings
}

/// The executor-agnostic plan. Built once from the expanded WorkflowConfig + WorkflowDag.
pub struct ScheduledPlan {
    pub order: Vec<String>,                    // dag.execution_order()
    pub groups: Vec<Vec<String>>,              // dag.parallel_groups()
    pub rules: HashMap<String, ScheduledRule>, // instance name → unit
}
```

`ScheduledPlan::build(config: &WorkflowConfig, dag: &WorkflowDag, workdir: &Path,
env_resolver: &EnvironmentResolver, wildcard_values: &HashMap<String,String>) -> Result<Self>`.
The rendered shell command reuses `build_execution_command` / `render_shell_command` — the same
single implementation the local executor uses. Script rendering goes through the backend's
`render_script` (below), which calls the existing `core::cluster::generate_submit_script*`
functions unchanged and prepends `cd <workdir>` to the script body so the job runs from the
declared workdir regardless of the submitting process's cwd (required for mock-scheduler and
real-scheduler behavioural parity).

### 3.2 Trait

```rust
#[async_trait]
pub trait ExecutorBackend: Send + Sync {
    fn name(&self) -> &'static str;
    /// Render a scheduler submit script for one scheduled rule.
    fn render_script(&self, rule: &ScheduledRule, config: &ClusterJobConfig) -> Result<String>;
    /// Submit a fragment (wave); returns backend job ids in fragment order.
    async fn submit(&self, fragment: &[ScheduledRule], config: &ClusterJobConfig) -> Result<Vec<String>>;
    /// Poll statuses; returns job id → status.
    async fn poll(&self, job_ids: &[String]) -> Result<HashMap<String, BackendJobStatus>>;
    async fn cancel(&self, job_id: &str) -> Result<()>;
    /// Fetch job logs (slurm: sacct output; others best-effort).
    async fn logs(&self, job_id: &str) -> Result<String>;
}

pub enum BackendJobStatus { Pending, Running, Completed, Failed, Cancelled, Unknown }
```

### 3.3 `ClusterExecutor`

One implementation parameterized by `ClusterBackend` (slurm | pbs | sge | lsf):

- `render_script` → `generate_submit_script` (unchanged functions from `core/cluster.rs`).
- `submit` → `sbatch --parsable <script>` (slurm, bare job id — fixes #74 item 1),
  `qsub <script>` (pbs, bare id; sge, sentence-parsed), `bsub <script>` (lsf, sentence-parsed).
  All id capture goes through one shared helper `parse_job_id(backend, stdout, stderr)`.
- `poll` → `squeue -j <ids> --noheader -o "%i|%t"` (slurm) and per-backend equivalents; state
  parsing in one shared helper (includes array elements like `12345_12`).
- `cancel` → `scancel` / `qdel` / `bkill`; `logs` → `sacct -j <id> --format=JobID,State,ExitCode,Elapsed,MaxRSS` (slurm), best-effort elsewhere.

Submission is async (tokio::process::Command, spawn_blocking where blocking); timeouts on all
scheduler calls.

### 3.4 `BackendDriver`

```rust
pub struct DriverConfig { pub max_submitted: usize, pub poll_interval: Duration, pub poll_timeout: Option<Duration> }
pub struct BackendDriver { backend: Arc<dyn ExecutorBackend>, config: DriverConfig, run_dir: PathBuf }

impl BackendDriver {
    /// Execute exactly `to_run` on `plan`. Caller computes `to_run` from the shared
    /// invalidation predicates (run_preview) — the driver never decides *what* to run.
    pub async fn run(&self, plan: &ScheduledPlan, to_run: &HashSet<String>,
                     cluster_config: &ClusterJobConfig) -> Result<Vec<JobRecord>>;
}
```

Semantics:

- Wave loop: at most `max_submitted` jobs in flight (pending+running); submit a wave when
  all its dependencies are done (native `--dependency=afterok` chaining per backend render, or
  driver-side ordering when the backend cannot chain). Submit failures mid-DAG leave a consistent
  state: everything already submitted is cancelled, remaining rules reported as skipped.
- Poll loop with `poll_interval`; on `Completed`/`Failed` map to `JobRecord` (same type the
  local executor uses, so checkpoint recording is identical in shape).
- Failure propagation: dependents of a failed rule are marked skipped (`blocked by failed
  upstream dependency`) — same vocabulary as `run`.
- Cancel-on-drop: dropping the driver (Ctrl-C, error) calls `cancel` on every in-flight job.
- Run directory (greppable, per #74's proposal, minimal phase-2 subset):
  `run_dir/events.jsonl` (append-only, source of truth), `jobs/<rule>/<key>=<value>/`
  containing `job.sh`, `job.id`, `stdout.log`, `stderr.log`, `status.json`. Content-addressing
  for resume stays inside `status.json`/checkpoint — never in paths.

The driver is core-only: no CLI dependency, no checkpoint mutation (the caller records
`JobRecord`s into `CheckpointState`), no plan/invalidation computation.

### 3.5 CLI changes

`cli/commands/cluster.rs` script generation goes through `ClusterExecutor::render_script` instead
of calling `generate_submit_script_with_env` directly. Output is byte-identical (parity test
asserts it). `mkdir -p logs` before scripts are written (#74 phase-1 note 2). Everything else in
the command stays.

### 3.6 Mock scheduler CI harness (per #74 comment 3)

`crates/oxo-flow-cli/tests/fixtures/mock-scheduler/`: shell shims `sbatch`, `squeue`, `scancel`,
`sacct` that

- execute submitted scripts for real in the background (state dir via `MOCK_SCHEDULER_DIR`),
- record `Submitted batch job N` on stderr and bare id on stdout under `--parsable`,
- answer `squeue -o "%i|%t"` and `sacct` queries from the state dir,
- honour `scancel` (kills the process group),
- support array syntax `12345_12` in queries.

Integration tests prepend the fixture dir to `PATH`. This is the same ScriptedBackend idea the
maintainer asked for in #74 and is reusable for @andrewbudge's CI.

### 3.7 P1 acceptance tests (real executions, no cluster)

1. **Parity (the issue's acceptance):** a 3-sample workflow with a dependency chain, run twice in
   fresh workdirs — once via the local path (`LocalExecutor` through the normal run machinery),
   once via `BackendDriver` + `ClusterExecutor` + mock scheduler. Assert: identical completed
   rule sets, identical output file contents, checkpoint files with the same completed rules and
   input-manifest semantics (same recorded manifests modulo timestamps).
2. **Preview parity:** the dry-run will-run set (computed with the same shared predicates) equals
   the driver's submitted set.
3. **Failure propagation:** one rule fails → dependents skipped, driver returns non-zero, in-flight
   jobs cancelled, `events.jsonl` shows the FAILED/SKIPPED sequence.
4. **Render parity:** `cluster` command output byte-identical before/after the trait refactor.
5. Unit: `parse_job_id` per backend, `squeue`/`sacct` output parsing (incl. arrays), plan build
   (order/groups/deps match `WorkflowDag`), resource resolution.

Out of scope here: `run --profile slurm` CLI wiring, re-attach, job arrays, web-triggered runs
(→ #74 phases 2–4).

## 4. P2 — unified content-addressed invalidation across storage

### 4.1 `StorageBackend::head`

```rust
pub struct RemoteStat { pub size: u64, pub etag: Option<String> }
// StorageBackend gains:
async fn head(&self, path: &StoragePath) -> Result<Option<RemoteStat>>;
```

- S3 (`s3-storage` feature): `HeadObject` → `ContentLength` + `ETag` (raw string; composite
  `"hash-N"` ETags from multipart uploads are recorded as-is — equality comparison only, never
  recomputed locally).
- GCS (`gcs-storage` feature): objects:get → `size` + `md5Hash` (base64) as the etag. GCS has no
  native ETag; documented divergence (md5Hash is the stronger, pure content hash).
- Local: returns `None` (local invalidation keeps using size+mtime+sha256).

### 4.2 Manifest extension (backward compatible)

`InputManifestEntry` gains `remote: Option<RemoteManifestEntry>` where
`RemoteManifestEntry { scheme: String, key: String, size: u64, etag: Option<String> }`.
Serde default `None` → legacy checkpoints load unchanged (existing adoption path applies).

### 4.3 Snapshot + match

- `snapshot_input_manifest(rule, workdir, wildcard_values, resolver: &StorageResolver)`: rendered
  input patterns are parsed with `StoragePath::parse`. Remote URIs (exact object references —
  no remote glob/dir semantics in this iteration, documented) get a `head()` call; on error the
  entry is skipped with a warning (consistent with today's degrade-to-no-manifest behaviour).
  Local patterns keep the existing size/mtime/sha256 logic untouched.
- `manifests_match`: local entries → existing logic; remote entries → `scheme == scheme &&
  key == key && size == size && etag == etag`; when etag is unavailable on either side, compare
  size only (documented conservative-for-availability fallback); scheme mismatch → mismatch.
- Call sites (run.rs preview + run loop) thread a resolver; default `StorageResolver::with_local()`
  keeps today's behaviour when no cloud backend is configured. These are mechanical one-line
  edits (pass `&resolver`) — the only `run.rs` touches in P2; the execution logic is unchanged.

### 4.4 P2 tests

- In-memory fake `StorageBackend` with a mutable etag: complete a rule whose input is
  `s3://bucket/key` → same-size rewrite with a new etag invalidates the rule (the exact scenario
  in the issue); unchanged etag → skipped.
- Backward compatibility: legacy manifest JSON (no `remote` field) loads and matches via the
  existing path.
- Unit: S3 `HeadObject` XML fixture parsing, GCS JSON fixture parsing, `manifests_match` remote
  matrix (etag equal/diff/None, size diff, scheme mismatch).
- Out of scope: staging remote files into the workdir, remote globs, writeback/upload — the
  `warn_if_remote_paths` stub stays; invalidation is the deliverable.

## 5. P3 — static + dynamic hybrid DAG: checkpoint re-entry

### 5.1 Config surface

```toml
[[rules]]
name = "discover"
shell = "python discover.py > discover.toml"
checkpoint = true
checkpoint_manifest = "discover.toml"   # new field: Option<String>, serde default
```

Manifest written by the rule at runtime (TOML):

```toml
[reentry]
group = "auto-discovered"   # optional; default "auto-discovered"
sample = ["s4", "s5"]       # new wildcard values appended (dedup) to that group
```

Validation: `checkpoint = true` requires `checkpoint_manifest`; checkpoint rules must not be
parameterized by `{sample}`/`{group}` (bounded re-entry — see 5.5).

### 5.2 Protocol

1. Round 0: existing static expansion (`expand_wildcards`) unchanged; plan + execution as today.
2. On successful completion of a checkpoint rule: read + parse its manifest (missing or
   unparsable → **fail the rule** with a clear error — checkpoint rules are contract-bearing).
3. Empty manifest → valid no-op (no re-entry).
4. Non-empty: append values to the named group; re-expand **from the rule templates** (see 5.3);
   diff instance names; append only NEW instances to `config.rules`; rebuild the DAG (deterministic
   `WorkflowDag::from_rules`); the event loop continues — already-completed rules stay completed,
   new instances run.
5. Checkpoint records the re-entry: `CheckpointState.reentries: Vec<ReentryRecord { round: u32,
   rule: String, group: Option<String>, samples: Vec<String> }>` (serde default; round is a global
   counter). Resume deterministically replays only the records whose checkpoint rule is still
   up-to-date (see 5.4).

### 5.3 Template preservation

`expand_wildcards` currently overwrites `self.rules` with expanded instances (config.rs:2497).
Add `WorkflowConfig.rule_templates: Vec<Rule>` populated before the first expansion (or at parse
time). Re-entry re-expands from templates with the merged groups — names are deterministic
(`{name}_{group}_{sample}`), so existing instances regenerate identically and only new values
produce new instances.

### 5.4 Invalidation and deterministic resume

- A reentry record stands only while its checkpoint rule is up-to-date (completed in checkpoint,
  outputs exist, not invalidated by config/input changes — the same shared predicates).
- If the checkpoint rule is invalidated, its contribution is **revoked**: its samples are excluded
  from the group merge, so plan reconstruction no longer contains those instances; when the rule
  re-runs, its manifest re-records (and supersedes) the record.
- Resume therefore reconstructs exactly the plan a fresh run would, for any mix of completed and
  invalidated checkpoint rules. `--rerun` behaves as with any other rule (its manifest re-reads).

### 5.5 Boundedness and limits

- Checkpoint rules never re-expand (their own wildcards frozen at round 0) → no self-referential
  growth.
- Global cap `MAX_REENTRY_ROUNDS = 32`; exceeding it aborts with a clear error (a runaway
  discovery loop is a workflow bug, not an engine feature).
- [[pairs]]-driven re-entry is out of scope (documented limitation); only `sample` (via group)
  is supported this iteration.

### 5.6 Execution-loop hooks (the only `run.rs` edits in this design)

- In the event loop, after a successful completion: if the rule is a checkpoint rule → run the
  5.2 protocol. The `dag` binding becomes mutable across rounds (rebuild after a merge);
  `compute_ready` takes the dag as a parameter instead of capturing it; `SchedulerState` gains an
  additive `add_rule(name)` so new instances are schedulable (unknown names count as pending).
- `BackendDriver` gets the same hook (checkpoint completion → caller-provided merge callback →
  driver continues with the augmented plan) so P3 E2E tests can run on either executor.

### 5.7 Dry-run preview

- Replays valid reentries (same reconstruction as run) → the previewed static plan covers all
  rounds the checkpoint already guarantees; checkpoint rules are annotated as possible re-entry
  points ("may add instances at runtime"); `--json` gains a `reentry` section listing valid and
  potential records. Preview/run parity (the #77 principle) is preserved.

### 5.8 P3 tests

- Unit: manifest parsing, group merge + dedup, instance diff, revoke-on-invalidate, template
  preservation idempotence.
- Integration (local path): discover rule emits 2 new samples → round-2 instances execute →
  outputs exist; resume (no changes) re-runs nothing and reconstructs the same plan; invalidating
  the discover rule revokes its samples (old instances gone from the plan) and re-executes;
  empty manifest → no re-entry; missing manifest → clear failure; `MAX_REENTRY_ROUNDS` abort.
- Integration (driver path, mock scheduler): same discovery workflow through `BackendDriver`.
- Dry-run: preview shows round-0 + recorded reentry reconstruction + checkpoint annotation.

## 6. Testing and CI

- TDD per the repo rules: tests first (RED) → implementation (GREEN) → refactor; AAA structure;
  integration tests for every phase; existing ~1500 tests must stay green throughout.
- `make ci` (fmt + clippy -D warnings + build + test + audit) must pass after each phase commit.
- Live smoke checks in addition to automated tests (e.g. a real `run` on a discovery workflow
  for P3, `cluster submit` byte-parity, dry-run JSON).

## 7. Documentation sync

- User guide: new "Execution backends" page (static plan → executor mapping, `cluster` command as
  render layer, mock-scheduler harness for CI); extend `workflow-format.md` (checkpoint_manifest,
  reentry manifest format, remote inputs + etag invalidation semantics, documented limitations);
  `run.md`/`dry-run.md` re-entry preview semantics; glossary entries (executor backend, re-entry,
  remote manifest); mkdocs strict build must pass.
- CHANGELOG: conventional commits per phase (git-cliff CI generates).

## 8. Issue management on completion

- Close #78 with a comment summarizing per-phase deliverables, test evidence, and links.
- Reply on #74: the trait/driver/mock-scheduler foundation is in tree; map to its phasing
  (Phase 2 wiring can sit on `BackendDriver`; the mock scheduler + shared job-id parsing fulfil
  comment items 3 and 5); note P2/P3 do not overlap its scope.
- No new follow-up issues expected beyond real-cluster validation (#67 checklist) and the
  documented limitations (remote globs/staging, pairs re-entry, `run --profile` wiring).

## 9. Risks

- **`run.rs` touch points**: P2 = two mechanical one-line resolver passes at the
  `snapshot_input_manifest` call sites (§4.3); P3 = bounded loop edits (§5.6). All are additive
  (dag rebinding, param instead of capture, `SchedulerState::add_rule`); #77 has shipped so its
  file space is free.
  (dag rebinding, param instead of capture, `SchedulerState::add_rule`); #77 has shipped so its
  file space is free.
- **`core/cluster.rs` churn**: none — the trait calls it unchanged (per #74).
- **Remote invalidation without a real cloud**: semantics are proven against a fake backend with
  controllable etags; S3/GCS adapters are feature-gated and parsing-tested from fixtures;
  real-cloud verification remains #67/#74 territory.
- **Re-expansion determinism**: names are generated from templates + groups; templates are
  preserved verbatim; diff-based merge never renames existing instances.
