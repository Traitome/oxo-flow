# Execution Backends

oxo-flow computes a fully determined **static plan** — topological order,
parallel groups, per-rule resource declarations — before any execution
begins. Execution backends do **not** re-understand the DAG: they map that
single plan onto a scheduler API. Because plan computation, invalidation,
and the checkpoint format stay single-implementation, the dry-run preview is
valid for every executor, and local and remote execution share the same
checkpoint semantics.

```text
.oxoflow file ──► wildcard expansion ──► static plan ──┬─► LocalExecutor (run)
                (single implementation)               └─► ExecutorBackend (scheduler)
```

## The static plan

`oxo_flow_core::backend::ScheduledPlan` is the executor-agnostic plan:

- `order` — topological execution order (`WorkflowDag::execution_order`),
- `groups` — parallel groups (`WorkflowDag::parallel_groups`),
- `rules` — one `ScheduledRule` per schedulable instance: the expanded rule
  (single source for script rendering and resource directives), the
  environment-wrapped shell command with `cd <workdir>` folded in, the
  workdir, instance-level dependencies, and `{config.x}` bindings.

## `ExecutorBackend` trait

```rust
pub trait ExecutorBackend: Send + Sync {
    fn name(&self) -> &'static str;
    fn render_script(&self, rule: &ScheduledRule) -> Result<String>;
    async fn submit(&self, script_path: &Path) -> Result<String>;
    async fn poll(&self, job_ids: &[String]) -> Result<HashMap<String, BackendJobStatus>>;
    async fn cancel(&self, job_id: &str) -> Result<()>;
    async fn logs(&self, job_id: &str) -> Result<String>;
}
```

The first implementation, `ClusterExecutor` (SLURM/PBS/SGE/LSF), renders
scripts with the existing directive generator (`core/cluster.rs`, unchanged)
and performs real submission and tracking:

- `submit` — `sbatch --parsable <script>` for SLURM (bare job id on stdout,
  fixing the `--dependency=afterok:Submitted batch job N` trap the old
  wrapper script had); PBS prints a bare id; SGE/LSF ids are parsed out of
  their submission sentences. All id capture shares one helper
  (`parse_job_id`), as does status-line parsing (`parse_status_line`),
  including array elements like `12345_12`.
- `poll` — `squeue -j <ids> --noheader -o "%i|%t"` and per-backend
  equivalents.
- `cancel` / `logs` — `scancel` / `sacct` and equivalents.

## `BackendDriver`

`oxo_flow_core::backend::driver::BackendDriver` executes a `ScheduledPlan`
through a backend. It never decides *what* to run — the caller computes the
will-run set from the shared invalidation predicates (the same ones the
dry-run preview uses). The driver:

- submits waves of at most `max_submitted` in-flight jobs (pending + running),
- polls at `poll_interval` and settles terminal jobs into `JobRecord`s — the
  same record type the local executor produces, so checkpoint recording is
  identical in shape,
- propagates failures: dependents of a failed rule are skipped
  (`blocked by failed upstream dependency`),
- cancels everything in flight on submit errors and poll timeouts,
- writes a greppable run directory: `events.jsonl` (append-only, source of
  truth) and `jobs/<rule>/` with `job.sh`, `job.id`, and `status.json`.
  Content-addressing stays inside the checkpoint — never in paths,
- supports checkpoint re-entry via two hooks (`on_checkpoint`, `merge`) so
  dynamically discovered instances execute in the same run (see
  [Checkpoint re-entry](#checkpoint-re-entry) below).

## Testing without a cluster

The workspace ships a **mock SLURM scheduler** at
`tests/fixtures/mock-scheduler/` — shell shims (`sbatch`, `squeue`, `scancel`,
`sacct`) that execute submitted scripts for real in the background and record
job state under `$MOCK_SCHEDULER_DIR`. Integration tests construct a
`ClusterExecutor` with `.with_scheduler_dir(...)` and
`.with_env("MOCK_SCHEDULER_DIR", ...)` to run whole workflows through the
driver in CI without any cluster. The shims reproduce two real-scheduler
behaviours worth knowing about: `sbatch` returns immediately (the job's
output pipe is fully detached) and failing jobs still record a terminal
state.

The P1 acceptance tests (`tests/cluster_backend.rs`) assert the core claim:

- local execution and backend execution of the same workflow produce the
  same checkpoint semantics (completed sets, benchmarks, input manifests)
  and identical outputs,
- the dry-run preview's will-run set equals the set the driver submits,
- poll timeouts cancel in-flight jobs.

`run --profile <NAME>` now drives the backend when the profile carries a
`[cluster]` block (see [Cluster submission](../commands/run.md#cluster-submission)).
Its submission set is the dry-run preview's will-run set unioned with the
run's `force_rules` — the same bypass set the local executor receives, since
`run` has already applied and persisted its invalidation analysis by the time
the cluster path runs. `tests/cluster_run_profile.rs` pins that combination to
what the local executor actually executes from identical state.

Real-cluster validation (SLURM scheduler matrix, version quirks) is tracked
in the cluster testing checklist; job arrays and accounting-backed resource
feedback have landed, re-attach to in-flight jobs is a follow-up.

Accounting is read once per job as it settles, not once per poll, and feeds
both the run directory and the checkpoint benchmarks — see
[what a finished job records](../commands/run.md#what-a-finished-job-records).
Backends report only what their store proves: SLURM reads exit code, elapsed
time, peak RSS and total CPU from `sacct` (peak memory comes from the step
rows, which is where SLURM puts it — the allocation row leaves it blank), PBS
and SGE report the equivalent `resources_used` / `ru_*` fields, and LSF
reports state alone.

## Assumptions

- Outputs, workdir, and logs live on **shared storage** — every compute node
  sees the same files, so jobs can run anywhere and the driver's checkpoint
  bookkeeping stays meaningful.
- Jobs are POSIX shell scripts; the scheduler runs them with `/bin/bash`.

## Checkpoint re-entry

See [Workflow Format](workflow-format.md#checkpoint-re-entry) for the config
surface. A `checkpoint = true` rule writes a re-entry manifest at runtime;
when it completes, the engine merges the new values, re-expands the rule
templates, and executes the new instances in the same run — on the local
path (`run`) and on the driver path alike. Every round is still a static
plan, so previews stay deterministic and resumes reconstruct the same plan.
