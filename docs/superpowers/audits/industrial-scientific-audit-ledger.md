# Industrial and Scientific Audit Ledger

> **Status**: living audit record for the industrial-grade and publication-tier program tracked by PR #997.
>
> **Scope**: architecture, execution truth, security boundaries, scientific evidence, cross-platform UX, documentation, and release readiness.
>
> **Rule**: no item may be described as production-ready, clinically qualified, or publication-grade until the evidence column is populated with a reproducible artifact and the relevant exit gate passes.

No phase, stage, benchmark, usability result, or manuscript claim is complete merely because code or prose exists.

---

## 1. Maturity Vocabulary

Every material capability must use one of these labels.

| Label | Meaning |
| :--- | :--- |
| **Implemented and verified** | Code exists, the stated contract is exercised by deterministic tests or measured evidence, and the artifact is available for review. |
| **Implemented but unproven** | Code exists and has local tests, but the production-scale, cross-platform, or scientific claim has not been independently demonstrated. |
| **Prototype** | A usable path exists with known semantic, security, or operational gaps; it is not a release-grade contract. |
| **Simulation only** | The result comes from a model, fixture, seeded generator, mock scheduler, or synthetic dataset. It is not experimental evidence. |
| **Planned / blocked** | Work is defined but not implemented, or a prerequisite defect must be remediated first. |

The following terms are reserved for completed evidence:

- **verified** means the exact claimed property passed an identified test or measurement;
- **measured** means run-level data with provenance was collected on named hardware and software;
- **qualified** means the applicable IQ/OQ/PQ protocol has been executed and signed;
- **publishable** means the study has a pre-registered analysis, complete negative results, independent replication where applicable, and an auditable artifact.

---

## 2. Evidence Boundary

This ledger combines static review, local tests, and command-level simulations. It does not establish full CI, production readiness, clinical fitness, or publication acceptance.

### Verified local evidence available at this audit point

- No complete `make ci` exit status is yet claimed for the final candidate tree.
- Focused core tests and `cargo fmt --all -- --check` passed on the working tree before the final evidence pass.
- The workflow API contract suite passed 39/39 with AI providers disabled before the latest documentation and webhook edits.
- The SUS harness unit tests passed 4/4 from `eval/benchmarks`.
- Fault-injection CLI simulation: 10 cases produced 8 static detections, 7 attributed diagnoses, 7 repair proposals, 0 applied repairs, 0 verified repairs, and 0 successful reruns.
- Cohort CLI pilot: `N=10` produced 30 measured plan nodes from the real `dry-run --json` output.
- The statistical fixture is explicitly synthetic and non-inferential.
- The hardware projection is explicitly labeled as a model projection and not a measurement.

### Not established

- No completed GitHub or designated-Linux-host CI result is claimed for this tree.
- No live PostgreSQL, horizontal multi-process, crash-recovery, or production-load campaign is complete.
- No live SLURM, PBS, SGE, LSF, Kubernetes, AWS Batch, or Google Cloud Batch campaign is complete.
- No NUMA, NVMe-tiering, accelerator, or 10,000-sample performance study is complete.
- No human-subjects study is complete.
- No manuscript has been submitted, accepted, or independently replicated.
- The latest commits have not been remotely validated on the designated Linux host.

---

## 3. Covered Surfaces

| Surface | Review depth | Result |
| :--- | :--- | :--- |
| Workflow parsing, expansion, transforms, BioTypes, and CLI preflight | Static review plus focused tests | Material fixes applied; checker false-acceptance and resolved-admission gaps remain |
| Cluster and remote execution | Static review plus mock/fault paths | Prototype; cancellation, artifact retrieval, and accounting gaps |
| Web/API/auth/storage/execution | Read-only static review | Multiple release blockers remain |
| Frontend, editor, desktop, and VS Code UX | Static review | Semantic-loss, stale-validation, accessibility, and lifecycle gaps |
| Evaluation, statistics, hardware, and cohort tooling | Static review plus local simulations | Claims separated from evidence; scientific campaign not executed |
| Roadmap, specification, implementation checklist, and reports | Full review | Completion claims are being corrected in this ledger wave |
| CLI/AI, provider, MCP, and recovery | Independent static review; no provider or MCP server was executed | Multiple security and scientific-integrity blockers |
| Broad tests/benches/examples and scripts/data | Not yet covered by a dedicated wave | Planned |

---

## 4. Release-Blocking Findings

### RB-01: No OS-level multi-tenant execution boundary

**Severity**: P0

**Maturity**: Prototype; unsafe for untrusted team or HPC tenants

Normal users can submit arbitrary workflow shell commands executed under the server account. The executor inherits the server environment, so documented secrets such as `DATABASE_URL`, `OXO_FLOW_MASTER_KEY`, and provider credentials may be exposed. Directory confinement is not a process, filesystem, network, CPU, memory, or credential boundary.

Evidence:

- `crates/oxo-flow-web/src/domains/execution/handlers.rs:72`
- `crates/oxo-flow-web/src/domains/execution/handlers.rs:377`
- `crates/oxo-flow-web/src/executor.rs:552`
- `crates/oxo-flow-web/src/domains/execution/sandbox.rs:35`

Required exit evidence:

- A two-tenant adversarial test proves workflows cannot read another tenant's workspace, the server database, host credentials, or arbitrary host files.
- The runner clears or allowlists the environment.
- CPU, memory, process, filesystem, and network limits are enforced by an OS or container boundary.

### RB-02: Saved-pipeline runs do not have immutable artifact snapshots

**Severity**: P0

**Maturity**: Prototype; blocks reproducible run-history claims

Runs created from a saved pipeline can share the persistent pipeline directory as their `workdir`. A later run can overwrite an earlier run's workflow, checkpoint, logs, and outputs. Concurrent initial runs are not protected by the guarded insertion path used for retry and resume.

Evidence:

- `crates/oxo-flow-web/src/domains/execution/handlers.rs:493`
- `crates/oxo-flow-web/src/domains/execution/handlers.rs:557`
- `crates/oxo-flow-web/src/domains/execution/handlers.rs:566`
- `crates/oxo-flow-web/src/domains/execution/handlers.rs:1374`
- `crates/oxo-flow-web/src/workspace.rs:236`
- `crates/oxo-flow-web/src/domains/execution/files.rs:600`

Required exit evidence:

- Every run has a unique, immutable workdir or content-addressed snapshot.
- Two sequential runs leave all hashes and provenance records for the first run unchanged.
- Concurrent creation from one saved pipeline yields one admitted run and one conflict under the declared policy.

### RB-03: Create, stage, persist, and spawn are not atomic

**Severity**: P1

**Maturity**: Prototype

Workflow-write failure, run-row insertion failure, and uploaded-input staging failure can be logged while execution continues. The client can receive a successful response without a durable, cancellable run ledger entry or with incomplete inputs.

Evidence:

- `crates/oxo-flow-web/src/domains/execution/handlers.rs:503`
- `crates/oxo-flow-web/src/domains/execution/handlers.rs:566`
- `crates/oxo-flow-web/src/domains/execution/handlers.rs:593`
- `crates/oxo-flow-web/src/domains/execution/handlers.rs:649`

Required exit evidence:

- Fault injection at each stage produces a non-2xx response and no process spawn.
- The workflow manifest, input manifest, run row, and workdir lease are durably committed before spawn.
- A crash before or after commit has a deterministic recovery outcome.

### RB-04: Web admission does not yet have resolved-graph parity

**Severity**: P1

**Maturity**: Implemented but unproven

Web run creation now invokes the same core validator as CLI preflight before
persistence for the submitted TOML. Typed-source routing parses TOML
structurally, recognizes table and inline declaration forms, and follows only
locally staged sources reachable through `[[include]]`. Any typed or
uninspectable include graph, and a directly typed workflow with any include
graph or configuration overrides, fails closed because the web layer cannot
yet evaluate the CLI's fully resolved graph. Unrelated staged assets are
ignored.

This closes part of the routing gap, but it does not make admission sound:
the shared core checker itself has false-acceptance paths (RB-04A). It also
does not establish parser-complete type-declaration discovery, remote-include
policy, or CLI/web identity for the resolved graph.

Evidence:

- `crates/oxo-flow-cli/src/commands/run/preflight.rs:36`
- `crates/oxo-flow-core/src/format.rs:1108`
- `crates/oxo-flow-web/src/domains/execution/service.rs:21`
- `crates/oxo-flow-web/src/domains/execution/handlers.rs:461`
- `crates/oxo-flow-web/src/domains/execution/service.rs` (`validate_workflow_types` call and focused tests)
- `crates/oxo-flow-web/src/domains/execution/handlers.rs` (`TYPED_ADMISSION_UNSUPPORTED` fail-closed gate)
- `crates/oxo-flow-web/tests/api_contract_hardening.rs` (`create_run_rejects_invalid_biotypes_before_persistence`, `create_run_accepts_a_compatible_typed_workflow`, `create_run_fails_closed_for_typed_workflows_with_overrides`, `create_run_fails_closed_for_inline_types_with_overrides`, `create_run_fails_closed_when_an_include_declares_types`, `create_run_fails_closed_when_an_include_uses_inline_types`, `create_run_ignores_unrelated_staged_toml_that_declares_types`, `create_run_fails_closed_when_a_reachable_include_is_unstaged`, `create_run_fails_closed_for_an_unstaged_include_without_optional_fields`, `create_run_fails_closed_for_a_git_include_without_optional_fields`, `create_run_accepts_direct_types_with_an_unrelated_staged_asset`, `create_run_fails_closed_for_direct_types_with_a_local_include_graph`, `create_run_rejects_a_dot_alias_of_the_main_workflow_key`, `create_run_rejects_duplicate_canonical_include_keys`, `create_run_rejects_case_insensitive_duplicate_include_keys`, `create_run_detects_types_through_a_dot_alias_include_key`)

Required exit evidence:

- Direct invalid declared types receive a structured `400` before run-row persistence; unit and API tests cover this boundary. Static control flow returns before directory lease or process spawn, but dedicated filesystem/process assertions remain open.
- Typed include graphs, uninspectable includes, and direct typed workflows with any include graph or overrides fail before persistence with `TYPED_ADMISSION_UNSUPPORTED`; positive compatible-type coverage proves the direct gate and unrelated staged assets do not cause false rejection.
- Include-file resolution and configuration-override semantics must run
  through the same fully resolved admission path before the finding is closed.
- Undefined config references must receive the same pre-persistence rejection.
- CLI and web admission must evaluate the same resolved workflow and invoke
  the same canonical checker.
- Declaration detection must be parser-derived and table-driven across every
  supported TOML spelling; an uninspectable source must fail closed.
- The remote-include policy must be explicit and fail closed, with tests for
  network access, pinned repository includes, cycles, and path aliases.

### RB-04A: BioType checker can scientifically false-accept

**Severity**: P1

**Maturity**: Partial (2026-11-18) - checker false-accept fixed (issue #1107); the resolved include/override admission seam remains open (RB-04)

A typed consumer can be connected to an undeclared producer without a
diagnostic. A required reference build is compared only when both sides
declare one, so an absent producer build passes. Declared `attributes` are
never compared. File-header introspection has a second compatibility
implementation that has already diverged from DAG subtyping.

Evidence:

- `crates/oxo-flow-core/src/types/checker.rs:299-303`: a typed consumer skips
  an untyped producer.
- `crates/oxo-flow-core/src/types/checker.rs:140`: reference builds are
  compared only when both sides declare metadata.
- `crates/oxo-flow-core/src/types/checker.rs`: `is_subtype_of` now enforces
  the TYPE-007 declared-attribute contract (checker.rs:229-239, added since
  this entry was written); the remaining compatibility divergence is the
  duplicate header-introspection implementation at
  `crates/oxo-flow-core/src/types/inspect.rs:65` (`satisfies_spec` vs the
  DAG checker).
- `crates/oxo-flow-web/src/domains/execution/handlers.rs`: the web gate can
  be fail-closed at routing level while ultimately invoking this permissive
  checker for admitted workflows.

Required exit evidence:

- A typed consumer fed by an undeclared or partially declared producer fails
  closed with a stable `TYPE-*` diagnostic.
- Every consumer-required reference build must be present and match; absence
  cannot satisfy the contract.
- Attribute compatibility has documented subset/equality semantics and tests
  for missing, conflicting, and additional producer attributes.
- One canonical compatibility function serves DAG subtyping and file-header
  introspection, with table-driven tests proving identical outcomes.
- Adversarial fixtures cover all supported declaration syntax, include
  graphs, wildcard channels, missing builds, attributes, unknown types, and
  false-positive/negative behavior. No scientific gate may pass before this
  evidence exists.

### RB-05: Remote execution can report completion without complete artifacts

**Severity**: P1

**Maturity**: Partial (2026-11-18) — result-pull failure and archive extraction safety are fixed; post-pull declared-output verification and restart-safe cancellation remain open

A failed result pull used to be logged while the remote exit code was still used to finalize the run. That is fixed (issue #1112): when the pull fails the run is failed through the shared non-success path (`mark_failed`) with the pull error as its reason, and the run row records the reason in `artifacts_incomplete` so downstream consumers can distinguish "ran and succeeded" from "raced a broken transfer". Archive extraction no longer trusts the stream: the remote pull and bundle verification both go through `oxo-flow-core::archive::extract_tar_requiring`, which rejects absolute/`..` member paths and link targets that are absolute or contain any `..` component (a `..` can escape through a symlinked segment, so lexical cancellation is not accepted), re-verifies every member on disk, and requires `execution.log` + `.exit-code` plus a run-id completion record that the wrapper tars **after** the result tree — a truncated stream cannot carry it (#1189). The pull does not re-derive the run's expanded output list locally, so declared-output existence after the pull still rests on the remote engine's exit-0 contract; that remains the open half of CL-04. Cancellation still relies on an in-memory registry and wrapper-only process targeting.

Evidence:

- `crates/oxo-flow-core/src/archive.rs` (`extract_tar_requiring`, `ensure_link_stays_inside`, `ignore_zeros` read path)
- `crates/oxo-flow-web/src/domains/clusters/remote.rs` (`pull_directory`, `pull_command`, `REQUIRED_RESULT_MEMBERS`, `drain_stderr_capped`, `decide_remote_finalize`)
- `crates/oxo-flow-cli/src/commands/bundle.rs` (`extract_and_verify_bundle`)

Required exit evidence:

- Pull and extraction integrity checks must pass before a run can become `completed`. [PARTIAL — a failed pull cannot finalize as `completed` (#1112); extraction rejects traversal and a truncated stream (run-id completion record tarred after the tree, `execution.log`, `.exit-code`) before the finalize decision (#1189). Declared output paths are not re-derived locally: if the remote engine itself exits 0 without producing a declared output, the pull cannot detect it, so CL-04 stays Partial.]
- Malicious path traversal in an archive is rejected. [DONE (#1189) — `../escape.txt`, absolute members, and link targets that are absolute or contain any `..` component (including the `x -> .` + `y -> x/..` symlink-chain escape and chained `a3 -> a2/..` variants) fail extraction with the member named; tests `archive::tests::parent_dir_member_is_rejected_and_nothing_is_written_outside`, `absolute_member_is_rejected_instead_of_rewritten`, `escaping_symlink_is_rejected_and_never_created`, `escaping_hard_link_is_rejected`, `symlink_chain_through_dotdot_is_rejected`, `chained_symlink_through_dotdot_is_rejected`, `link_target_with_dotdot_is_rejected_even_when_lexically_inside`, `result_extraction_rejects_parent_dir_members`, `bundle_with_parent_dir_member_is_rejected`, `bundle_with_absolute_member_is_rejected`, `bundle_with_escaping_symlink_is_rejected`.]
- Restart-safe cancellation terminates the remote process tree or reports the exact unresolved jobs. [OPEN — not addressed by #1112 or #1189.]

### RB-06: Scheduler and cluster outcomes are not scientifically reliable

**Severity**: P1

**Maturity**: Prototype; mock and CLI-wrapper evidence only

Cancellation errors can be discarded, missing accounting data can be converted to exit code `0`, and a non-element backend can expand one array-base verdict across all elements. Cloud executors shell out through `kubectl`, `aws`, and `gcloud`; they are not direct API clients.

Evidence:

- `crates/oxo-flow-core/src/backend/driver.rs:194`
- `crates/oxo-flow-core/src/backend/driver.rs:203`
- `crates/oxo-flow-core/src/backend/driver.rs:760`
- `crates/oxo-flow-core/src/backend/driver.rs:769`
- `crates/oxo-flow-core/src/backend/driver.rs:991`
- `crates/oxo-flow-core/src/backend/k8s.rs:242`
- `crates/oxo-flow-core/src/backend/aws_batch.rs:201`
- `crates/oxo-flow-core/src/backend/gcp_batch.rs:218`

Required exit evidence:

- Cancellation failure is surfaced and reconciled.
- Missing accounting is unknown, not success.
- Per-element outcomes are represented independently where the scheduler exposes them.
- At least one live campaign covers submit, queue, run, cancel, fail, retry, and artifact retrieval for each claimed backend.

### RB-07: Run lifecycle state can contradict the process and database

**Severity**: P1

**Maturity**: Partial (2026-11-18) - resume no longer fabricates `running` without a live process group and pause/resume roll back the freeze when persistence fails (issue #1111); monotonic compare-and-set transitions and cancel-versus-completion race testing remain open

Cancel used to write a terminal row before signaling, risking an overwrite of a racing completion; pause sends `SIGSTOP` before a fallible database write; resume could write `running` when no process group existed.

Evidence:

- `crates/oxo-flow-web/src/domains/execution/handlers.rs:1463`
- `crates/oxo-flow-web/src/domains/execution/handlers.rs:1540`
- `crates/oxo-flow-web/src/domains/execution/handlers.rs:1644`
- `crates/oxo-flow-web/src/domains/execution/handlers.rs:1731`
- `crates/oxo-flow-web/src/infra/db/sqlite.rs:949`
- `crates/oxo-flow-web/src/infra/db/postgres.rs:700`

Required exit evidence:

- Monotonic compare-and-set state transitions.
- Cancel versus completion race testing.
- Pause rollback when persistence fails.
- Resume rejects missing processes without fabricating state.

### RB-08: Upload replacement is destructive on failure

**Severity**: P1

**Maturity**: Fixed (2026-11-18) - uploads stage to a sibling temp file and rename over the destination (issue #1109); an interrupted, oversize, or write-failure upload leaves the prior file intact

The destination used to be truncated before multipart consumption succeeded; oversize or read errors removed the destination, and write errors could leave a partial replacement.

Evidence:

- `crates/oxo-flow-web/src/domains/execution/files.rs:795`
- `crates/oxo-flow-web/src/domains/execution/files.rs:817`
- `crates/oxo-flow-web/src/domains/execution/files.rs:835`

Required exit evidence:

- Interrupted, oversize, and write-failure uploads preserve the prior file hash.
- Successful replacement is atomic for concurrent readers.

### RB-09: Quotas are accounting hints, not enforced or durable limits

**Severity**: P1

**Maturity**: Partial (2026-11-18) — atomic admission and durable/shared state are Fixed (issue #1133); declared-memory enforcement is proven for local Linux runs, while non-Linux platforms and remote (cluster) runs remain advisory (follow-up #1221)

Pre-fix behavior (verified on `main` before #1133): `QuotaTracker` kept all
usage in a process-local `Mutex<HashMap>` (`infra/quota.rs:66` before the
rewrite), `RUN_RESERVATIONS` was a process-local map (`:197`), and
`check()` (`:86`) / `record_start()` (`:133`) were separate operations with
a wide window between them; the execution handlers performed the check at
`domains/execution/handlers.rs:420` and the record at `:605`, so two
concurrent creates could both pass and both be counted, and every restart
or second instance started from zero. Declared resources were never
enforced (advisory counts only).

Mechanism (issue #1133):

- Quota state now routes through the unified `StorageBackend` — three
  idempotent tables created by both the SQLite and PostgreSQL `init`
  bootstraps: `quota_usage` (per-user counters + rolling 24 h window
  anchor), `quota_reservations` (per-run resource claims; the release
  source) and `quota_config` (singleton admin limits row).
- Admission is ONE storage operation: a compare-and-swap
  `INSERT ... ON CONFLICT(user_id) DO UPDATE ... WHERE <every limit>`
  whose rows-affected is the decision, plus the reservation insert in the
  same transaction. A rejected request rolls back its reservation insert,
  so nothing is recorded; re-admitting a reserved run id is an idempotent
  replay. Release is a `DELETE ... RETURNING` (lookup and release gate in
  one statement) plus counter decrement in one transaction (idempotent
  under cancel-vs-failure races).
- Handlers call `admit_spawn_budget` immediately before the atomic workdir
  claim (create/retry/resume) and give the reservation back on any later
  error path; every terminal path calls the now-async
  `quota::release_and_complete`. Startup reconciliation
  (`quota::reconcile_stale`, wired into `init_database`) reclaims
  reservations whose run row is terminal or missing.
- Enforcement: `executor::apply_memory_rlimit` installs `RLIMIT_DATA` on
  the spawned CLI process tree at the run's declared total memory
  (`[rules.resources] memory` sum, 512 MB floor for engine overhead) for
  local runs on Linux; exceeding it fails the allocation, the rule dies,
  and the run fails. The budget is derived from the RESOLVED workflow
  (include expansion before derivation — AGENTS invariant) for create, and
  from the workdir's workflow file for retry/resume, so included modules'
  own declarations count. The in-memory `QuotaTracker` remains only as the
  no-backend fallback (single-lock `try_admit`) and the limits cache.
- The crate attribute moved from `forbid(unsafe_code)` to
  `deny(unsafe_code)` with ONE localized `#[allow(unsafe_code)]` on the
  `pre_exec` rlimit helper (no safe API exists for a post-fork syscall).

Exit evidence:

- Atomic admission under concurrent requests — MET. Eight simultaneous
  admission calls against `max_concurrent_runs = 2` (two backend instances,
  one database) yield exactly two approvals; at the HTTP layer, two
  concurrent `POST /api/runs` against a limit of one yield exactly one 200
  and one `429 QUOTA_EXCEEDED`, and the loser leaves no run row. Fail-first
  probe: the pre-fix check-then-record shape admitted 8/8.
- Durable and shared quota state — MET on SQLite. A second backend instance
  (and a simulated restart with a fresh pool) reads the counters the first
  wrote; a release through one instance is visible through the other; the
  limits row round-trips. The PostgreSQL implementation (same idempotent
  schema bootstrap, same single-statement CAS) is compile-checked
  (`--features postgres --all-targets`) and the live storage contract test
  gained a quota section, but that contract requires
  `OXO_FLOW_TEST_POSTGRES_URL` and was NOT executed in this environment —
  live PostgreSQL parity is not claimed here.
- Enforced resource ceilings with observable rejection — PARTIAL. Local
  Linux runs: proven end-to-end — a stub CLI allocating 520 MiB under a
  512 MiB declaration fails the run, while the identical stub with no
  declaration completes. Non-Linux (Darwin rejects `RLIMIT_DATA`/`RLIMIT_AS`
  with EINVAL) and remote cluster runs remain advisory; follow-up #1221.

Evidence tests:

- `infra::db::sqlite::quota_tests::quota_admission_is_atomic_durable_and_releasable`
- `domains::execution::handlers::quota_tests::resolved_quota_counts_include_declared_resources`
- `quota_durable::quota_state_is_atomic_durable_and_shared_across_instances`
- `quota_durable::concurrent_create_requests_admit_exactly_one_under_the_limit`
- `quota_memory_enforcement::declared_memory_is_enforced_with_rlimit_and_rejection_is_observable` (Linux-gated)
- `postgres_storage_contract::storage_backend_contract_round_trips_on_postgres` (quota section; requires `OXO_FLOW_TEST_POSTGRES_URL`)

Docs: `docs/guide/src/reference/web-api.md` → "Quota" (durable/atomic
semantics + "Declared-resource enforcement"); 
`docs/guide/src/reference/web-system-architecture.md` → "Durable runtime
state". Commit binding: `c7e347839fa3e1f4adbf11827bd32a2479fec083`.

### RB-10: Cross-platform editing can silently destroy scientific semantics

**Severity**: P1

**Maturity**: Prototype

The guided editor reconstructs TOML and silently drops advanced semantics. AI chat can replace editor content before explicit acceptance. Metro grouped editing is semantically unsafe, stations lack complete keyboard access, and monitor process grouping collapses environments to `system`.

Evidence to be expanded by the dedicated frontend audit follow-up. The reviewed surfaces include `frontend/src/pages/PipelineEditor.tsx`, `frontend/src/components/MetroCanvas.tsx`, and the run monitor and chat surfaces.

Required exit evidence:

- Round-trip editing preserves every supported field or rejects unsupported edits.
- Generated changes require explicit diff acceptance.
- Keyboard-only graph navigation and editing are tested.
- Group operations are proven semantics-preserving or disabled.

---

### RB-11: AI repair can overwrite a complete workflow with a model-approved partial block

**Severity**: P0

**Maturity**: Fixed (2026-11-18) - a patch that shrinks the original's rules/includes/references is refused, and accepted fixes are written atomically (issue #1110)

The recovery prompt permits a `[workflow]`-only or `[[rules]]`-only replacement block, and the
model's own safety verdict authorizes replacement of the entire file. A `[workflow]`-only block
parses as a complete workflow with an empty default rule list. There is no independent semantic
diff review, post-write full-file validation, or successful rerun before the failure summary is
shown.

Evidence:

- `crates/oxo-flow-cli/src/commands/ai_recover.rs:99`, `:145`, `:214`, `:235`
- `crates/oxo-flow-cli/src/commands/run/failure.rs:303`, `:309`, `:331`
- `crates/oxo-flow-core/src/config/parse.rs:28`
- `crates/oxo-flow-core/src/config/model.rs:1907`

Required exit evidence:

- A scripted-provider test proves that partial replacement blocks cannot overwrite the file.
- The displayed diff, applied patch, and post-write content hash refer to the same bounded change.
- The complete workflow validates, preserves protected semantics, and reruns successfully before
  the UI or CLI may report a fix.

### RB-12: Sensitive config values are materialized in scheduler and container artifacts

**Severity**: P0

**Maturity**: Partial (2026-11-18) — local container argv and cloud job bodies route values through
references; HPC script bodies and live artifact-inspection proof remain.

Cluster command files contain the fully expanded shell command. Kubernetes, AWS Batch, and GCP
Batch manifests embed that command, and Docker/Singularity execution keeps sensitive values in
child arguments. Masking covers records and status text, not the artifacts consumed by schedulers,
container runtimes, backups, or workload inspectors.

Evidence:

- `crates/oxo-flow-core/src/backend/driver.rs:479`, `:620`, `:1231`
- `crates/oxo-flow-core/src/backend/k8s.rs:79`
- `crates/oxo-flow-core/src/backend/aws_batch.rs:71`
- `crates/oxo-flow-core/src/backend/gcp_batch.rs:41`
- `crates/oxo-flow-core/src/executor/process.rs:683`
- `crates/oxo-flow-cli/src/commands/run_cluster.rs:37`

Required exit evidence:

- A sentinel declared `sensitive = true` never appears in generated local, HPC, Kubernetes, AWS, or
  GCP artifacts or child argv. [DONE for local containers (docker/singularity) and for the cloud
  job bodies of Kubernetes / AWS Batch / GCP Batch — both the `run --profile` driver and
  `cluster submit` route each value to an `OXO_FLOW_SENSITIVE_<n>` reference before any manifest is
  built (issue #1120), with one shared binding table per run. HPC script bodies
  (SLURM/PBS/SGE/LSF) still materialize the literal — open. Residual found by PR
  #1176's review and fixed in `dea4ee73` (issue #1177): a path-valued sensitive
  value keeps its docker auto-bind mount — mounts are discovered from the
  PRE-routing command and a sensitive path is expanded host-side from its routed
  variable (`-v "${OXO_FLOW_SENSITIVE_n}":…`), so the wrapper string and the
  rule shell's argv carry only the reference. The docker client itself
  necessarily receives the expanded path as the mount source (a bind mount
  cannot be expressed without the runtime seeing the path); a value embedded in
  a LONGER path is skipped with a masked warning instead of being
  re-materialized literally. Tests:
  `environment::tests::{docker_wrap_mounts_sensitive_path_through_routed_variable,
  docker_wrap_explicit_mounts_keep_non_sensitive_paths_literal,
  docker_wrap_skips_mount_for_path_embedding_a_sensitive_value,
  singularity_wrap_unchanged_by_external_mount_threading}`,
  `executor::process::tests::resolve_command_mounts_sensitive_path_from_pre_routing_command`
  (the latter fails on the pre-fix rescan-the-routed-text path).]
- Each backend obtains the value through a restricted secret-injection path. [Kubernetes carries a
  `secretKeyRef` to the documented `oxoflow-secrets` Secret; docker/singularity forward the routed
  name through the child environment. AWS Batch and GCP Batch have no engine-visible injection
  field (AWS: job-definition `containerProperties.secrets`; GCP: submission-time
  `secretVariables` needs the operator's project), so the operator-side mapping is documented and
  the reference fails closed rather than leaking. Not yet proven by a live campaign.]
- Artifact inspection, restart, and cancellation tests prove no secret is recoverable. [OPEN.]

### RB-13: AI tool and MCP trust boundaries fail open

**Severity**: P1

**Maturity**: Fixed (2026-11-18) - [AI-04] generic-token release is gated on an explicit
operator origin allowlist (issue #1123 token half, commit `ced0f3ad`, merged in `e8048647`);
the [AI-05] read-to-egress chain now requires an explicit data-release decision (this PR).

A generic MCP bearer token was sent to any activated public MCP endpoint unless a per-server token
existed, with no operator trust decision. [FIXED (#1123, commit `ced0f3ad`, merged `e8048647`):
the generic `OXO_FLOW_MCP_TOKEN` is now released only to exact origins listed in
`OXO_FLOW_MCP_TRUSTED_ORIGINS`; per-server `OXO_FLOW_MCP_TOKEN_<SERVER>` tokens remain available
because their variable name is derived from the endpoint identity — destination-scoped by
construction — so setting one is itself the operator's per-endpoint decision.] Automatically
approved project reads and outbound fetches formed a data-exfiltration path for project `.env`
files and unpublished results. [FIXED (#1123 second half): built-in tools now classify each
invocation for outbound egress (`Tool::egress_target`; `fetch_url` reports the destination
origin, MCP bridges are Opaque); the approval policy (`tool_call_approved`) no longer lets an
egress call inherit the read-only shortcut — it runs only with an explicit human approval or
when its exact origin is listed in `OXO_FLOW_AI_FETCH_TRUSTED_ORIGINS`, and a non-interactive
session (no approver) refuses. Refusals name the destination origin only, never the URL path /
query or file contents. The auto-approved scoped read is unchanged, so local workflows that
make no outbound calls are unaffected.]

Evidence:

- `crates/oxo-flow-ai/src/mcp.rs:163` (MCP tools never read-only), `:174` (MCP calls classified
  Opaque for the data-release gate), `:270` (trust-entry canonicalization), `:290` (trust-list
  parsing), `:310` (exact-origin match), `:332` (token selection), `:352` (per-endpoint
  resolution), `:385` (`token_env_var`), `:748` (header attach)
- `crates/oxo-flow-ai/src/tools/builtin.rs:51` (`resolve_scoped_path` — no content denylist),
  `:140` (scoped read is auto-approved), `:273` (`url_origin` — one origin transformation,
  shared with `mcp::origin_of`), `:294` (trust-list canonicalization, fail-closed on invalid
  entries), `:317` (`fetch_trusted_origins`), `:526` (`FetchUrlTool`), `:587`
  (`FetchUrlTool::egress_target`)
- `crates/oxo-flow-ai/src/tools/mod.rs:31` (trait default `is_read_only() == true`), `:43`
  (`Tool::egress_target` default `None`), `:219` (`ToolRegistry::egress_target`,
  deny-by-default `Opaque`)
- `crates/oxo-flow-ai/src/agent/orchestrator.rs:638` (`tool_call_approved`), `:658`
  (`tool_call_approved_with_trusted_origins` — egress gate + exact-origin trust), `:688`
  (`tool_refusal_message` — origin only, no payload), `:2023` (fail-closed malformed-arguments
  regression test)
- `crates/oxo-flow-cli/src/commands/ai_runtime.rs:72` (scoped `read_file`), `:82` (knowledge
  registry incl. `fetch_url`), `:118` (manifest-declared `mcp://` origin connected + registered)
- Tests: `egress_data_release_policy`, `egress_refusal_names_only_the_origin_and_never_the_payload`,
  `read_then_fetch_cannot_reach_the_network_without_approval` (orchestrator),
  `fetch_url_egress_classification`, `trusted_origins_parse_exactly_and_drop_invalid_entries`,
  `url_origin_canonicalizes_http_origins_only` (builtin),
  `registry_egress_classification_and_defaults` (tools), plus the token-half tests
  `generic_token_is_not_sent_to_an_unapproved_origin`,
  `generic_token_is_sent_to_an_approved_origin`, and
  `bearer_token_env_is_propagated_with_per_server_precedence`.

Required exit evidence:

- Generic credentials are sent only to an explicit operator-approved origin. [DONE for the generic
  MCP token (#1123): `OXO_FLOW_MCP_TRUSTED_ORIGINS` gates release on an exact origin and withholds
  the generic token otherwise; adversarial tests cover unapproved, lookalike, and approved
  origins (wire-level for the first and the approved case) plus per-server precedence.
  Per-server tokens are destination-scoped by construction.]
- Read followed by network egress requires an explicit data-release decision and visible summary.
  [DONE (#1123 second half): the policy gate now requires an approver for every classified egress
  call unless the exact origin is operator-trust-listed; the decision is surfaced as the approval
  prompt (interactive) or a recorded tool result naming the origin (non-interactive refusal), and
  the refusal never echoes the URL path/query or file contents. Pinned by
  `egress_data_release_policy`, `egress_refusal_names_only_the_origin_and_never_the_payload`, and
  the end-to-end `read_then_fetch_cannot_reach_the_network_without_approval`.]

- Invalid tool arguments are rejected or retried; no executable `{}` call reaches approval. [DONE —
  the orchestrator's JSON-validity gate fails closed before approval/execution (AI-07,
  `malformed_tool_arguments_fail_closed_with_a_reemit_tool_result`). The earlier `{}`-repair claim
  was refuted against shipped code and removed from this entry.]
- Adversarial project-skill, MCP-output, and prompt-injection tests cover each path. [DONE — the
  generic-token release path has project-skill-selected-endpoint wire tests
  (`generic_token_is_not_sent_to_an_unapproved_origin`,
  `bearer_token_env_is_propagated_with_per_server_precedence`), and the read-to-egress path is
  covered by a prompt-injection-shaped scripted-model test
  (`read_then_fetch_cannot_reach_the_network_without_approval`: auto-approved scoped read of a
  secret file followed by `fetch_url` to an unapproved origin — the fetch never executes, the
  refusal carries the origin only, and neither the refusal nor CLI output echoes the secret.
  (The scripted backend's `observed_calls` stores the model's own tool-call arguments, so the
  model-side transcript does contain the value the model chose to send; that is a record of the
  attempted call, not an output path opened by the gate.)]

### RB-14: Workflow-scoped AI policy is parsed but not enforced

**Severity**: P1

**Maturity**: Fixed (2026-11-18) — the CLI now resolves the documented config chain (defaults →
global config file → environment → project `.oxo-flow/ai.toml` → workflow `[ai]` → CLI overrides)
BEFORE building the provider, and applies the resolved provider/model end to end
(`create_provider_from_config_chain`); enabled AI that cannot resolve a usable provider emits a
structured degraded-mode reason (`AI_DISABLED_BY_ENV` / `AI_PROVIDER_INVALID` /
`AI_CREDENTIAL_MISSING` / `AI_NOT_CONFIGURED`); `template --ai` refuses an existing destination
without `--force` or interactive confirmation and writes through the shared
`oxo_flow_core::atomic_file` staged-write helper (issue #1122).

**Review delta (independent review of PR #1207).** All review findings are fixed:
(HIGH) a saved config's provider-specific **endpoint, model, and key** are now dropped when a
higher tier switches the provider — `AiConfig::merge` clears them on a provider switch, so the
environment-selected provider's credential can never be sent to a saved origin and the saved
model can never be requested from another provider; (MEDIUM) `ai explain` now resolves through
`resolve_ai_scope` and prints the structured degraded reason; (LOW) the overwrite decision is
re-checked immediately before the atomic replace, so a destination created while the model ran
is refused rather than clobbered; (LOW) a typo'd `OXO_FLOW_AI_PROVIDER` is warned about on the
explicit-chain path too, via one shared predicate. The E2E stubs are deadline-bounded so a
regression that stops calling the provider fails instead of hanging.

> **Correction**: the previous sentence "`auto_fix` has no runtime consumer outside parsing and
> tests" was stale and is removed. `crates/oxo-flow-cli/src/commands/run/failure.rs` consumes
> `auto_fix_decision` at run time, so `never` / `ask` / `always` already produced distinct write
> behavior before this fix.

Evidence:

- `crates/oxo-flow-ai/src/config.rs` — `AiConfig::provider_explicit` (distinguishes an absent
  `provider` key from an explicit default), `AiConfig::resolve_chain_with_env`
- `crates/oxo-flow-ai/src/provider.rs` — `create_provider_from_config_chain`,
  `AiDegradedReason`, `degraded_reason`
- `crates/oxo-flow-cli/src/commands/ai_runtime.rs` — `resolve_ai_scope`, `AiRuntime::new`
- `crates/oxo-flow-cli/src/commands/ai_template.rs` — `overwrite_decision`,
  `ensure_destination_writable`, `ensure_destination_did_not_appear` (TOCTOU re-check),
  `write_output`, `try_resolve_ai`
- `crates/oxo-flow-cli/src/commands/ai_explain.rs` — explain resolves `resolve_ai_scope`
- `crates/oxo-flow-cli/src/commands/run/failure.rs:325-358` — `auto_fix` IS a runtime consumer
- Docs: `docs/guide/src/reference/ai-cli.md` (provider/model precedence table, degraded mode,
  overwrite semantics), `docs/guide/src/commands/template.md`,
  `docs/guide/src/commands/ai.md`, `docs/guide/src/reference/workflow-format.md`

Tests:

- `chain_provider_and_model_override_the_environment`,
  `chain_explicit_provider_without_credential_degrades_to_noop`,
  `chain_saved_key_is_reused_only_for_the_same_provider`,
  `chain_model_override_applies_without_an_explicit_provider`,
  `chain_disabled_pin_outranks_an_explicit_provider`,
  `degraded_reason_names_the_failure_class` (`oxo-flow-ai`, provider.rs)
- `provider_switch_never_reuses_the_saved_providers_origin_model_or_key`,
  `invalid_provider_env_does_not_override_an_explicit_chain_selection`,
  `provider_env_is_invalid_recognizes_typos_only` (`oxo-flow-ai`, provider.rs)
- `provider_explicit_distinguishes_absent_from_an_explicit_default`,
  `merge_provider_switch_drops_inherited_provider_specific_fields`,
  `resolve_chain_with_env_orders_global_env_project_workflow_cli` (`oxo-flow-ai`, config.rs)
- CLI end to end (`crates/oxo-flow-cli/tests/ai_policy.rs`; all nine fail on the pre-fix tree,
  the first five against the original code and the review-delta four against the first-fix
  commit):
  `workflow_provider_and_model_override_project_env_and_global`,
  `project_ai_config_applies_when_the_workflow_selects_nothing`,
  `enabled_ai_with_unusable_credentials_reports_a_structured_reason`,
  `template_ai_refuses_to_overwrite_an_existing_destination`,
  `template_ai_force_passes_the_overwrite_guard`,
  `provider_switch_never_sends_the_env_credential_to_the_saved_origin` (two stubs),
  `ai_explain_uses_the_workflow_provider_and_model`,
  `ai_explain_reports_a_structured_degraded_reason`,
  `destination_created_while_generating_is_not_clobbered` (TOCTOU)
- Atomic replacement / overwrite (`ai_template.rs`):
  `overwrite_decision_requires_consent_for_existing_destinations`,
  `destination_that_appeared_during_generation_is_refused`,
  `force_consents_to_a_destination_that_appeared_during_generation`,
  `destination_present_before_generation_is_not_treated_as_a_race`,
  `write_output_replaces_the_destination_atomically_without_following_symlinks`,
  `write_output_leaves_no_staged_temp_files`

Commit binding: `00631450` (initial fix) + `a2bb041f` (independent-review delta) on branch
`fix/1122-rb14-ai-policy` (issue #1122, PR #1207); pre-fix failure verified by temporarily
stashing the fix and re-running the E2E suite (5/5 failed), then stashing only the review-delta
fixes and re-running it (the four review-delta tests failed: saved-origin leak, missing
`ai explain` chain, no structured explain reason, and the clobbered raced destination).

Required exit evidence:

- Documented global/project/workflow provider and model precedence is tested end to end. —
  **Met**: the E2E tests spawn the real CLI and assert the workflow's provider/model beats the
  project file, the environment, and the saved global file, with a local stub proving which
  endpoint actually received the request.
- `never`, `ask`, and `always` produce distinct write behavior. — **Met before this change**
  (`auto_fix_decision` + `run/failure.rs`); the stale prose is corrected above.
- Enabled AI with unusable credentials emits a structured degraded-mode reason. — **Met**:
  the reason carries a stable code, the provider/credential detail, and the actionable
  suggestion; degrader surfaces print it, hard-fail surfaces (`template --ai`,
  `env create --ai`) return it as the command error.
- Generation refuses an existing destination without confirmation and uses atomic replacement. —
  **Met**: the guard runs before any paid generation; an accepted write uses the shared
  `atomic_file` helper (temp file + fsync + rename + parent fsync).

Still unproven (not claimed by this fix): real-provider behaviour (live endpoints, auth and
streaming failures) was not exercised — the E2E tests use local stubs; the interactive y/N
prompt path is covered only by its pure decision function (the non-interactive refusal and
`--force` consent are covered end to end); the web/desktop provider registry keeps its own
precedence and is unchanged by this CLI-scoped fix.

### RB-15: Rule shells inherit the runner's ambient credentials

**Severity**: P1

**Maturity**: Prototype

The rule shell adds declared environment variables but does not clear or allowlist the inherited
environment. A shared or imported workflow can read provider keys, cloud credentials, database
URLs, MCP tokens, and proxy credentials without declaring them.

Evidence:

- `crates/oxo-flow-core/src/executor/process.rs:3960` (base `spawn_rule_shell`; the local fix routes it through `apply_rule_env_allowlist`)
- `crates/oxo-flow-core/src/executor/process.rs:1323` (environment setup spawn via `env_tooling_shell_command` — the accepted-risk channel described below)
- `crates/oxo-flow-core/src/cluster.rs` `generate_{slurm,pbs,sge,lsf}_script` (HPC job scripts; isolated behind the shared allowlist in the same fix)

Required exit evidence:

- A rule with no declared environment cannot observe sentinel ambient secrets.
- Explicit declarations expose only the named variables.
- Local and HPC execution paths share the same tested allowlist policy. Local docker/singularity dispatch runs inside the same cleared rule shell; its runtime plumbing is re-admitted through the narrow list below. Cloud dispatch (k8s/aws-batch/gcp-batch) has no ambient environment to clear: the engine writes only the rendered command into the manifest/spec, so a job's environment is the container image's `ENV` (or the GCP VM image environment) plus what the engine writes into the command — the rule's declared `envvars` (as `export K='V'`) and the `OXO_FLOW_SENSITIVE_<n>` secret references. The submit host's environment is never forwarded and no generated body carries an ambient `env`/`environment` channel; `kubernetes_job_body_carries_no_submit_host_environment`, `aws_batch_job_body_carries_no_submit_host_environment`, and `gcp_batch_job_body_carries_no_submit_host_environment` pin the boundary by re-executing the test binary with sentinel credentials in its ambient environment (#1171).
- Local docker/singularity rules re-admit an exact, exported list of container-daemon variables — `CONTAINER_DAEMON_ENV_VARS` in `crates/oxo-flow-core/src/executor/process.rs` (issue #1173, RB-15 residual): `DOCKER_HOST`, `DOCKER_CONFIG`, `DOCKER_CERT_PATH`, `DOCKER_TLS_VERIFY`, `DOCKER_BUILDKIT`, `APPTAINER_CACHEDIR`/`SINGULARITY_CACHEDIR`, `APPTAINER_BIND`/`SINGULARITY_BIND`, `APPTAINER_TMPDIR`/`SINGULARITY_TMPDIR`, and `APPTAINER_LOCALCACHEDIR`/`SINGULARITY_LOCALCACHEDIR`. This is a deliberately narrow admission of runtime plumbing the engine itself invokes (a remote daemon is unusable otherwise), never a `DOCKER_*`/`APPTAINER_*` prefix. Declared `envvars` are applied last and override every ambient copy (including these); `OXO_FLOW_ENV_PASSTHROUGH` re-admits the same ambient value and cannot change it. The bare-local path's allowlist is unchanged. Cloud dispatch is not covered by this list because it forwards no submit-host environment at all (see the exit-evidence bullet above).

**Accepted risk — environment SETUP channel (issue #1174, decision (b)).**
Environment setup spawns (`conda/mamba env create -f <workflow spec>`,
`pixi install`, `python3 -m venv` + `pip install -r <workflow
requirements>`, plus their teardown/retry/verify forms) deliberately keep
the full ambient environment; rule shells, `pre_exec`, and rule/workflow
hooks do not. This is an accepted residual, not a resolved finding:
RB-15 stays open. Rationale: the setup inputs are workflow-authored
executable content — `pip install` builds pinned sdists (whose build
backend runs `setup.py`) and conda runs package post-link scripts — and
that code runs as the operator with ordinary filesystem access, so it can
read `~/.aws/credentials`, `~/.docker/config.json`, or
`$HOME/.oxo-flow/ai_config.json` whether or not the environment was
cleared; an environment clear is cosmetic, not a boundary. Clearing would
instead break legitimate provisioning that depends on proxy/mirror/CA
variables (`HTTP_PROXY`, `HTTPS_PROXY`, `NO_PROXY`, `PIP_INDEX_URL`,
`PIP_TRUSTED_HOST`, `SSL_CERT_FILE`, `CONDA_*`, corporate mirrors) the
engine cannot enumerate per tool. The actual boundary is the
workflow-trust decision: do not run workflows you do not trust. Pinned by
`env_setup_spawn_inherits_ambient_environment` and
`rule_shell_does_not_inherit_the_setup_sentinel`
(`crates/oxo-flow-core/src/executor/process.rs`); documented in
`docs/guide/src/reference/environment-system.md` and
`docs/guide/src/reference/workflow-format.md`.

---

## 5. Detailed Findings

### 5.1 Workflow, Validation, and Engine

| ID | Severity | Finding | Current maturity | Evidence and remediation |
| :--- | :---: | :--- | :--- | :--- |
| WF-01 | P1 | Load-path parsing bypassed `extends`, so inherited rules were absent. | Fixed (#1178) | Behavior landed in af4929c53e67e7bc5ab7ab65c8b96edc0b47bea6 (#997): both load paths (`WorkflowConfig::parse`, used by the web handlers, and `WorkflowConfig::from_file`, used by `validate`/`run`/`dry-run`/`graph`) call `resolve_rule_templates` before validation/DAG derivation, and multi-level chains resolve from the resolved base rather than the pre-resolution snapshot. This PR additionally rewrites `extends` with the module namespace inside namespaced `[[include]]`s (previously such a module failed to load with "extends 'base' which does not exist"). Evidence tests: `cli_load_path_resolves_multilevel_rule_extends`, `cli_load_path_merges_included_base_rules_before_extends_resolution`, `cli_load_path_resolves_extends_inside_namespaced_include` (`tests/extends_load_path.rs`); `web_parse_handler_resolves_multilevel_rule_extends`, `web_validate_handler_reports_inherited_fields_for_multilevel_chain` (`crates/oxo-flow-web/tests/extends_load_path.rs`); `config::tests::resolve_includes_with_namespace_rewrites_extends`. |
| WF-02 | P1 | Empty scatter domains and zero/invalid `transform.split.n` could produce a misleading successful plan. | Fixed (#1184) | Commit binding: the pre-existing guards landed in af4929c53e67e7bc5ab7ab65c8b96edc0b47bea6 (#997) — empty scatter value domain ("has an empty value domain for"), and `split.n` zero/non-integer ("must be greater than zero", "is neither a positive integer nor a numeric config value"); `f265db315` (unified transform operator) added the split-without-source ("has no split values") and `split.glob`-zero-match guards. Empirical CLI probes at that base showed the guards did not cover the contract: `validate` exited 0 for every degenerate domain, and `dry-run` exited 0 (misleading successful plan) for an empty scatter `variable`, an empty `[[sample_groups]]` `samples` list, a zero-match `sample_pattern`, and an empty `split.by`; `split.n = 100001` ground through 100k map instances for 5+ minutes instead of failing. PR #1184 adds documented E023 (empty scatter domain/variable, empty/missing `split.by`, missing split source, empty `split.values_from`, zero/negative/non-integer/oversized `split.n`), shares the `split.n` classification between validate and expansion, caps `n` at the engine-wide `MAX_COMBINATIONS` (100,000) ceiling, and fails plan-time when a DECLARED sample domain binds zero instances (undeclared wildcards keep the W041 + execution-gate contract). Every probe now exits non-zero with a rule-named diagnostic (`split.n = 100001` fails in 0.03 s). Evidence tests (one non-zero-exit CLI test per degenerate input, asserting diagnostic text): `crates/oxo-flow-cli/tests/degenerate_domains.rs::{empty_scatter_values_fails_closed, empty_scatter_values_from_fails_closed, unresolvable_scatter_values_from_fails_closed_on_plan, empty_scatter_variable_fails_closed, empty_sample_group_samples_fails_closed, empty_values_table_fails_closed, empty_values_from_fails_closed, zero_match_sample_pattern_fails_closed_on_plan, zero_match_pairs_pattern_fails_closed_on_plan, split_n_zero_fails_closed, split_n_negative_fails_closed, split_n_non_integer_fails_closed, split_n_above_expansion_cap_fails_closed, split_n_integer_overflow_fails_closed, empty_split_by_fails_closed, missing_split_by_fails_closed_naming_the_rule, transform_without_split_source_fails_closed, split_glob_zero_matches_fails_closed_on_plan}`; core registry: `config::tests::{expand_scatter_rejects_empty_variable, expand_rejects_empty_sample_group_domain, expand_rejects_zero_match_sample_pattern_domain, expand_transform_rejects_empty_split_by, expand_transform_rejects_degenerate_split_n_matrix, expand_transform_rejects_no_split_source, classify_split_n_distinguishes_config_refs_from_degenerate_literals, validate_reports_e023_for_statically_degenerate_domains, validate_e023_respects_split_source_priority}`. Docs: glossary E023, `commands/validate.md`, `workflow-format.md` degenerate-domain contract. Review fixes (independent review of #1184): an empty `[[sample_groups]]` list is a W042 WARNING, not E023 — the cohort may be supplied at run time with `--samples @sheet.tsv` and validate takes no samples, so hard-failing validate would reject a documented workflow pattern; dry-run/run still fail closed without samples. A zero-match `pairs_pattern` now fails closed for pair placeholders (`{pair_id}`, `{experiment}`, `{control}`, …), the same empty-domain class as `sample_pattern`, and the unbound-input warning no longer misattributes an unrelated expansion failure to an empty sample domain. |
| WF-03 | P1 | Transform-generated rules lost environment, hooks, retries, staging, tags, benchmarks, and related semantics. | Fixed (#1182) | The defect was live at af4929c53^: map/combine rules were rebuilt from `Rule::default` with only six copied fields (probe: generated `t_chr1` had `tags=[]` vs authored `["variant"]`). Construction fixed in af4929c53e67e7bc5ab7ab65c8b96edc0b47bea6 (#997) — generated rules clone the authored rule and override only `name`/`input`/`output`/`shell`/`cleanup_chunks`, clearing `transform`/`scatter`/`output_pattern`. Exhaustive field-by-field evidence over all 56 `Rule` fields (compile-guarded by a rest-free destructure; the source rule carries a non-default value in every field that can coexist with `transform` — `scatter`, `output_pattern`, and `input_groups` cannot, and are asserted to keep their defaults): `config::transform_semantics_tests::transform_generated_rules_preserve_every_rule_field`, `config::transform_semantics_tests::transform_without_combine_preserves_every_rule_field`, `config::transform_semantics_tests::transform_aggregate_combine_preserves_every_rule_field`. Docs: `workflow-format.md` `transform` → "Generated-Rule Semantics". |
| WF-04 | P1 | BioType validation can still false-accept undeclared producers, missing required builds, and declared attributes. | Checker fixed (#1118); admission advisory | TYPE-008/009 now report untyped-producer and build-incomplete edges (advisory at the admission gate, not blocking); resolved web include/override admission remains RB-04. |
| WF-05 | P2 | Conda-family environment handling had duplicated and divergent paths. | Fixed (#1183) | One `conda_family_*` builder generates create/verify/wrap/teardown for conda and mamba (binary token + documented conda-only `--no-capture-output` are the only differences), and `EnvironmentResolver` is the single dispatch point the executor uses. Cross-backend lifecycle evidence (no network, no real toolchain; fake executables on a temp PATH that record argv and materialize the prefixes the resolver checks): `conda_and_mamba_share_one_lifecycle_command_builder` (core unit: builder equality + resolver dispatch identity), `conda_lifecycle_create_use_teardown_and_reuse_share_one_path`, `mamba_lifecycle_create_use_teardown_and_reuse_share_one_path`, `conda_and_mamba_execute_one_shared_command_shape`, `pixi_lifecycle_create_use_store_rehome_and_env_gc` (create → use → store re-home → `env gc` checkpoint/age gates → broken-link rebuild), `batch_conda_environment_wraps_with_the_engine_created_env` (`crates/oxo-flow-cli/tests/conda_family_lifecycle.rs`). This PR also removed the divergent BATCH copy (commit `6d82dca2`): `batch -e "conda: …"` now routes through the shared wrap with the spec resolved against the batch workdir (it used to target `<file stem>`, an env `run` never creates). Deliberate exceptions remain: the standalone `env create` bootstrap (user-named env, its name is the documented contract) and the generated container Dockerfile lines. Remaining gap: real solver/network behavior, real-toolchain compatibility beyond the emulated contract, and cluster execution paths are not covered; `env gc` does not apply to conda/mamba by design. |
| WF-06 | P1 | `job.sh` can still contain expanded secrets. | Planned / blocked | Secret-store or minimal-environment routing is required before team/HPC use. |

### 5.2 Cluster, Remote, and Scheduler

| ID | Severity | Finding | Current maturity | Evidence and remediation |
| :--- | :---: | :--- | :--- | :--- |
| CL-01 | P1 | Post-submit failures could orphan accepted jobs before local job registration. | Fixed (#1188) | Code commit a0f0885: every fallible step after the scheduler accepts a job now cancels the accepted job(s) by scheduler id and fails the run naming them — non-array `jobs/<rule>/job.id` and `status.json` writes, array `index.json` plus per-element `status.json` writes. `cancel_inflight` attempts every target (arrays by base id, deduped) and returns the unresolved ids with the scheduler's own error instead of swallowing it; `fail_with_cancel` appends that outcome to the original failure, so an unconfirmed cancel is never reported as cancelled and a poll-timeout/interrupt/required-failure path surfaces it too. Tests (mock scheduler, fail-first verified against a0f0885^): `backend::driver::tests::post_submit_artifact_write_failures_cancel_accepted_jobs`, `backend::driver::tests::driver_errors_when_index_json_is_not_writable`, `backend::driver::tests::array_element_status_write_failure_cancels_accepted_array`, `backend::driver::tests::cancel_failure_after_post_submit_write_error_reports_unresolved_job`, `backend::driver::tests::cancel_failure_after_array_index_write_reports_unresolved_base`, `backend::driver::tests::cancel_inflight_reports_unresolved_jobs_only`. Docs: `docs/guide/src/how-to/run-on-cluster.md` → "Submission failure semantics". Remaining boundary (documented): a job the scheduler accepts but prints no parseable id for has no cancel handle; the run fails with the scheduler output quoted for manual cleanup. |
| CL-02 | P1 | Cluster artifacts and commands could expose sensitive values. | Implemented but unproven | Artifact permissions tightened and command logging masked; scheduler/container secret artifacts remain RB-12. |
| CL-03 | P1 | Cancellation and per-element accounting remain unreliable. | Planned / blocked | See RB-06. |
| CL-04 | P1 | Remote pull and extraction can claim success with missing artifacts. | Partial (#1189) | A failed result pull fails the run and records `artifacts_incomplete` (#1112). Extraction runs through `oxo-flow-core::archive::extract_tar_requiring`: absolute/`..` members and link targets that are absolute or contain any `..` component (including the `x -> .` + `y -> x/..` chain) are rejected by name, every extracted member is re-verified on disk, and the pull requires `execution.log` + `.exit-code` + a run-id completion record tarred after the tree, so a truncated stream fails the run. Still open: declared output paths are not re-verified locally after the pull (output existence remains the remote engine's exit-0 contract). Tests: `archive::tests::*`, `result_extraction_*`, `bundle_with_*_is_rejected`, `bundle_missing_a_declared_member_fails_naming_it`. See RB-05. |
| CL-05 | P2 | Cloud backend tests are mostly mock and CLI-wrapper tests. | Prototype | Live cloud campaigns are required before cloud-native claims. |

### 5.3 Web, API, Auth, and Storage

| ID | Severity | Finding | Current maturity | Evidence and remediation |
| :--- | :---: | :--- | :--- | :--- |
| WEB-01 | P0 | No OS-level tenant execution boundary. | Planned / blocked | RB-01. |
| WEB-02 | P0 | No immutable per-run artifact snapshot. | Planned / blocked | RB-02. |
| WEB-03 | P1 | Non-atomic create/stage/persist/spawn. | Planned / blocked | RB-03. |
| WEB-04 | P1 | Lifecycle transitions can contradict process state. | Fixed (#1116) | Resume refuses a run with no live process group (`409 RUN_NOT_RESUMABLE`) and no longer fabricates `running`; pause/resume roll back the process freeze when the status write fails. See RB-07. |
| WEB-05 | P1 | Failed re-upload can destroy the previous valid file. | Fixed (#1114) | Uploads stream to a sibling temp file and `rename` over the destination only after validation; every failure path removes only the temp, so the prior file survives. See RB-08. |
| WEB-06 | P1 | Remote pull failure does not prevent `completed`. | Fixed (#1112) | A failed result pull now fails the run and records `artifacts_incomplete`. See RB-05. |
| WEB-07 | P1 | Scheduler cancellation and accounting collapse outcomes. | Planned / blocked | RB-06. |
| WEB-08 | P1 | Web admission still depends on a permissive BioType checker and lacks resolved include/override parity. | Checker parity fixed (#1118); resolved admission open | Direct TOML routes now report TYPE-008/009 for untyped-producer and build-incomplete edges (advisory at the admission gate); the resolved include/override admission seam remains RB-04. |
| WEB-09 | P1 | Quotas are racy and process-local. | Partially fixed (#1133) | RB-09. Admission is one atomic storage CAS + reservation insert and quota state is durable/shared through `StorageBackend` (SQLite+PostgreSQL); declared memory is OS-enforced for local Linux runs (`RLIMIT_DATA`). Non-Linux and remote cluster runs stay advisory (follow-up #1221). |
| WEB-10 | P1 | OAuth can return a token when session insertion fails; logout can report success when deletion fails. | Planned / blocked | Fail closed or use a durable revoke/retry protocol. Evidence: `crates/oxo-flow-web/src/domains/auth/handlers.rs:760`, `:454`. |
| WEB-11 | P1 | Pipeline edits can silently lose updates; revision insertion is best-effort. | Fixed (#1136) | Baseline re-confirmed on main @ ccf1d4ffa: the update path wrote the row unconditionally (`crates/oxo-flow-web/src/domains/workflow/handlers.rs:802` at that base — the issue's `:775` had drifted) and `record_revision` (`:31`) logged-and-continued on DB failure, so the request reported success without its snapshot. Mechanism: a new monotonic `pipelines.revision` column (default 1) is the concurrency token (distinct from the TOML-derived `version`); it travels as a strong HTTP validator — `ETag: "<revision>"` on GET/create/PUT/rollback responses and back in `If-Match` on PUT/rollback — so the mirrored JSON response contract (`frontend/src/api/types.ts`) is unchanged. The storage layer performs a single-statement compare-and-swap (`UPDATE ... SET revision = revision + 1 WHERE id = ? AND (? IS NULL OR revision = ?)`, rows-affected checked) implemented in both the SQLite and PostgreSQL `StorageBackend` impls (`create_pipeline_with_revision`, `update_pipeline_cas`), so the guard is the atomic write, not a read-then-write. A stale validator writes nothing and answers `409 {code:"PIPELINE_VERSION_CONFLICT", detail:"expected revision N but the stored revision is M", suggestion:"Re-read GET /api/pipelines/{id} for its current ETag and retry with If-Match ..."}`; a malformed validator answers `400 INVALID_IF_MATCH`; omitting the header keeps the historical unconditional update (documented backward compatibility, token still advances). Atomicity statement: create (row + initial snapshot) and update/rollback (CAS row write + pre-write snapshot + 50-row prune) each run in ONE storage transaction per backend, so either both commit or the request fails; failure injection (dropping `pipeline_revisions` on a dedicated in-memory DB, no production test hooks) proves the update/create rolls back and cannot report success. Remaining boundary (documented): the fork/import `save_pipeline` upsert path writes no revision rows (copies start at revision 1 with no history), and the permission/default read before the CAS is outside the transaction — the write itself is guarded by the token, not by that read. Evidence tests: `interleaved_writers_one_conflicts_and_the_winner_survives`, `update_without_if_match_stays_backward_compatible`, `malformed_if_match_is_rejected_not_ignored`, `quoted_and_unquoted_if_match_both_work`, `cas_applies_once_and_records_the_pre_write_snapshot`, `stale_token_is_a_conflict_and_writes_nothing`, `failed_revision_insert_rolls_back_the_pipeline_update`, `failed_initial_revision_insert_rolls_back_the_create` (`crates/oxo-flow-web/tests/pipeline_concurrency.rs`). Docs: `docs/guide/src/reference/web-api.md` → "Optimistic concurrency (`ETag` / `If-Match`)" and Version History transaction semantics. Commit binding: 755259911e93dc68feb6c64daf0761c5e7aa90ba. |
| WEB-12 | P1/P2 | DAG edit history is unowned, process-global, unbounded, and non-durable. | Fixed (#1191) | Commit binding: `7636a2ab67b683fa6f68a8e27795729fe2cca5e1` (PR #1191, issue #1137). Stacks are now keyed by `(CurrentUser.id, pipeline id)` — the same `resolve(Option<Extension<CurrentUser>>)` identity the other pipeline handlers use, with the `default` pseudo-user in personal mode — so two identities editing the same pipeline id own independent stacks. Bounds are documented constants in `crates/oxo-flow-web/src/domains/dag/service.rs`: `MAX_EDIT_HISTORY_DEPTH` = 50 transitions per stack (oldest dropped) and `MAX_EDIT_HISTORY_PIPELINES` = 256 stacks process-wide (LRU eviction on a monotonic use clock). Restart semantics are fail-closed and documented, not persisted: history remains in RAM, undo/redo return a typed `HistoryOutcome`, and the HTTP layer answers `409 EDIT_HISTORY_UNAVAILABLE` (no stack: never edited, restart, or LRU loss) or `409 EDIT_HISTORY_STALE` (supplied TOML is not the history tip) with remediation suggestions; `404 NO_UNDO`/`NO_REDO` remain only for a genuinely exhausted direction. Tests: `domains::dag::service::tests::{edit_history_is_scoped_by_owner, edit_history_depth_cap_drops_the_oldest_transition, edit_history_lru_evicts_the_oldest_pipeline, undo_and_redo_without_history_are_unavailable, exhausted_undo_is_distinguished_from_unavailable_history}`; `domains::dag::handlers::tests::{undo_is_scoped_to_the_acting_user, undo_after_restart_fails_closed_with_structured_code, redo_after_restart_fails_closed_with_structured_code, stale_undo_fails_closed_without_consuming_history, undo_with_no_remaining_transitions_keeps_no_undo}`. Docs: `docs/guide/src/reference/web-api.md` (DAG editing scope/caps/lifetime/codes), `docs/guide/src/reference/dag-edit-api.md` ("History scope, caps, and lifetime" + failure-code table), `docs/guide/src/reference/web-system-architecture.md` (error-code table). |
| WEB-13 | P2 | Template visibility is ambiguous across tenants. | Fixed (#1193) | Commit binding: `ce0d3026f229c26524d7b31921ee5c47d45010dc` (PR #1193, issue #1138). One exported predicate — `can_read_pipeline` plus its write sibling `can_write_pipeline` in `crates/oxo-flow-web/src/domains/workflow/visibility.rs`, re-exported from the workflow domain root — now encodes the policy every path evaluates: an owner sees their own pipeline; `workspace`-visibility (shared-library/template) pipelines are readable by every signed-in user; another tenant's `private`/`link` pipeline is invisible (`link` is a share-token capability, not a read grant); the pre-existing admin cross-tenant override is documented; unknown visibility values fail closed; deletion is a hard delete in both backends, so a deleted pipeline has no row, is invisible to everyone (admins included), and every read answers 404. List/read convention: `GET /api/pipelines` filters its candidate window and `POST /api/pipelines/search` filters returned rows with the same predicate, while `GET`/`PUT`/`DELETE /api/pipelines/{id}`, both revision routes, rollback, fork, share, saved-id `diff`/`export`, `POST /api/runs` with `pipeline_id`, and `POST /api/ai/optimize` with `pipeline_id` answer `404 NOT_FOUND` (never 403) when it returns false. Storage `list_pipelines_visible`/`search_pipelines` are documented candidate windows (SQL scoping before `LIMIT`, audit #664), not the policy source. Documented exemptions: share landing/import (the token is the capability), the global `/api/templates` library (no per-tenant visibility column; writes owner/admin-gated), and the TOML-only DAG edit endpoints. Evidence tests (real team-mode router with Bearer identities): `own_pipeline_is_visible_in_list_and_read`, `other_tenants_private_pipeline_is_absent_from_list_and_404_on_read`, `workspace_shared_pipeline_is_visible_to_other_tenants_read_only`, `deleted_pipeline_is_invisible_in_list_and_read`, `foreign_pipeline_update_rollback_and_delete_are_refused_404` (also fork/share/run-create; asserts the row is intact), `admin_override_sees_foreign_private_pipeline`, `share_token_is_the_authorization_exemption` (`crates/oxo-flow-web/tests/pipeline_visibility.rs`); predicate unit matrix `domains::workflow::visibility::tests::{owner_sees_own_pipeline_in_every_visibility, other_tenants_private_pipeline_is_invisible, other_tenants_link_pipeline_is_invisible, workspace_shared_pipeline_is_visible_to_any_signed_in_user, unknown_visibility_fails_closed, admin_sees_every_tenant, only_owner_or_admin_may_write}`. Docs: `docs/guide/src/reference/web-api.md` → "Visibility policy (WEB-13 / issue #1138)"; `docs/guide/src/how-to/collaboration.md` permissions table aligned with the enforced policy. |
| WEB-14 | P2 | Browser downloads cannot attach the Bearer session header. | Fixed (#1194) | Re-confirmed on `main` @ `ce29e8ffd`: every artifact download takes the authenticated fetch path — `download()` (`frontend/src/api/client.ts:131`) attaches `authHeaders()` and routes a 401 through `redirectToLoginOn401()` (clears the token, lands on `/login?reason=expired\|invalid`); `downloadRunFile` (`:295`) builds on it; `MonitorReport.downloadRunArtifact` (`frontend/src/pages/MonitorReport.tsx:536`) reads the response as a Blob, derives the filename from `Content-Disposition`, and clicks a synthetic object-URL anchor. Whole-frontend scan: no `window.open`, and the only raw `<a href>` to an API URL is the public `openapi.json` doc link (`frontend/src/pages/ApiDocs.tsx:68`), so no long-lived token rides a query string. Frontend 401-surfacing evidence: `frontend/e2e/download-401.spec.ts` (Playwright) executes a real run (the core-flows Stage 4 & 5 fixture), opens its Report tab, and clicks the workdir-zip download. Because the shipped e2e server runs personal mode (no auth middleware), the spec installs a route-level stand-in for the team/hpc `require_auth` gate that answers with the middleware's exact structured 401 bodies: an absent session redirects to `/login?reason=invalid` with the "Sign-in is no longer valid" notice, a stale token to `/login?reason=expired` with the "Session expired" notice and the token removed, and both assert the click issued the request (with the expected absent vs stale `Authorization` header), zero browser download events, and no saved file; the authenticated case downloads the real zip through the fetch/Blob path with the session header attached (non-empty saved file). Negative control: removing the 401 branch from `download()` makes both negative cases fail on the login-redirect assertion. Remaining boundary (documented): the negative cases simulate the auth-gate response instead of driving a live team/hpc server — the guard itself is `require_auth` (`crates/oxo-flow-web/src/server.rs:1140`) and is not re-proven by this frontend test. Commit binding: `88780cd3ce5dd7818e9aca66731a3c9260c558a4`; PR #1194. |
| WEB-15 | P2 | AI provider metadata, chat persistence, and audit writes overstate privacy or durability. | Fixed (#1196) | Re-confirmed on `main` @ `0ae243850`. **Provider metadata is role-gated**: one predicate `may_read_provider_metadata` (`crates/oxo-flow-web/src/domains/ai/handlers.rs`) allows full metadata only in personal mode (loopback single-user) or for `admin`; `GET /api/ai/config/effective` keeps provider NAMES + `is_configured` for `user`/`viewer` but nulls `effective.model`/`effective.api_url` and `tiers.{env_model,env_url,server_model,user_model,user_api_url}`; the `GET /api/ai/config/user` shared-runtime fallback (no per-user row) nulls `api_url`/`model` for non-admins (a row the caller saved themselves stays theirs); `GET /api/ai/config` keeps its existing strictest public mask (`provider: ""`, no model/url); `GET /api/ai/config/server` stays admin-only. **Audit durability is explicit per call site**: one writer `audit.rs::write_audit` behind named `audit_required` / `audit_best_effort` (both delegate to the existing `StorageBackend::record_audit_log`; no parallel insert), and the middleware classifies every mutation with `audit_durability_for`. REQUIRED (fails closed with structured `500 AUDIT_WRITE_FAILED`): auth attempts/logout/OAuth, `POST /api/users`, `DELETE /api/users/{id}`, `POST/DELETE /api/auth/keys`, `POST /api/runs` + `{cancel,pause,resume,retry,resume-checkpoint,clean}`, `DELETE /api/pipelines/{id}`, `POST /api/pipelines/{id}/share`, `POST /api/ai/config`, `PUT /api/ai/config/{server,user}`, `POST /api/clusters`, `DELETE /api/clusters/{id}`, `PUT /api/webhook`, `POST /api/license/upload`; an already-5xx response is preserved so the runs-domain `503 RUNS_REQUIRE_STORAGE_BACKEND` boundary survives a no-backend deployment. BEST-EFFORT (warn, response unchanged): AI generation/translate/test, parse/validate/format/lint, DAG edits, chat, pipeline create/update/fork/rollback, template writes, quota/event-ticket, run report AI calls; the `collaboration::fork_pipeline` direct write migrated to the named best-effort helper. **Last handler-owned rows migrated (issue #1199, PR #1202):** the successful-login row in `domains/auth/handlers.rs` now uses `audit_required_with_hint` — REQUIRED, so a lost row answers the shared structured `500 AUDIT_WRITE_FAILED`, and the suggestion carries one sign-in-specific sentence (the session is persisted before the audit write, so a session token may already have been minted; the error does not claim the sign-in was refused). The license-upload metadata row moved to `audit_best_effort` (the middleware's REQUIRED `POST /api/license/upload` row already covers the action), so its loss leaves the handler's 200 unchanged. No handler calls `StorageBackend::record_audit_log` directly; `audit.rs` and `infra/db/mod.rs` list no remaining call sites. **Chat persistence** was already best-effort in code (`domains/chat/handlers.rs:29`) and is now documented as such (`web-api.md` Chat section: a failed row write warns and the reply still succeeds; `GET /api/chat/sessions` returns `[]` when storage is unavailable and that is not proof no conversation happened). Evidence tests: `ai_metadata_role_gate::provider_metadata_is_role_gated` (admin full view vs user/viewer redacted vs anonymous 401; per-user fallback redaction; the non-admin own-row branch stays fully visible including its saved endpoint/model; server-config 403 matrix); `audit_durability::required_audit_failures_fail_closed_and_best_effort_do_not` (drops `audit_logs` to inject the storage error: required pipeline deletion answers structured `500 AUDIT_WRITE_FAILED`, best-effort parse still answers `400 PARSE_ERROR`; control asserts the required row is normally recorded; the same test pins the already-5xx preservation boundary with a synthetic REQUIRED route answering 503); unit `audit::tests::audit_durability_matrix_covers_security_relevant_routes` and `audit::tests::required_audit_failure_is_a_structured_500` (pins the honest message "may already have been applied", forbids "refused", and requires verify-state/duplicate-effect guidance); `login_audit_required::successful_login_row_is_required_and_license_metadata_is_best_effort` (drops `audit_logs`: control login on healthy storage answers 200 and records exactly one row for the canonical user id; a successful login on broken storage answers structured `500 AUDIT_WRITE_FAILED` with the may-have-applied wording and the session-token note, and the persisted session row proves the ordering claim; a lost license-upload metadata row leaves the handler's 200 unchanged; verified fail-first against the old best-effort behavior). **Remaining work tracked here (D1 root cause, no separate issue; the #1199 call-site migration is DONE):** the audit row is not written in the same transaction as the mutation; a required-audit failure can leave the action applied (e.g. a deleted pipeline, an already-spawned retry, an applied key revocation), so the `500` states that the action may already have been applied and warns that retrying can duplicate effects — a transactional write is the follow-up. Docs: `docs/guide/src/reference/web-api.md` ("Provider metadata is role-gated (WEB-15 / issue #1140)", "Audit durability (WEB-15 / issue #1140)", Chat best-effort note), `docs/guide/src/how-to/deploy-modes.md` (audit paragraph corrected), `docs/guide/src/reference/web-system-architecture.md` (`AUDIT_WRITE_FAILED` error row). Commit binding: `2e6a493e` (implementation), `12061e51` (review fixes: post-commit audit wording, 5xx preservation, role-matrix tests); rebased onto `0ad50ade` — pre-rebase equivalents `90258f03` and `1c7ef24b`; PR #1196. #1199 migration: `7b05293a`, PR #1202. |
| WEB-16 | P2 | OAuth redirect and state are not fully bound server-side. | Fixed (#1141, #1197) | State is bound to `(provider, redirect_uri, issued_at)` server-side: `POST /api/auth/oauth/authorize` persists the binding in `oauth_states` (new `provider` / `redirect_uri` columns, idempotent SQLite + PostgreSQL migrations; historical rows default to `''` and can never verify), and the callback verifies and consumes it in ONE atomic `DELETE … RETURNING` before any token exchange. Unknown/replayed/malformed states answer `400 OAUTH_INVALID_STATE`; states past the 10-minute TTL answer the same code with an "expired" message; provider or redirect mismatches answer `400 OAUTH_STATE_MISMATCH`, and a mismatch still consumes the state so it can never be retried. The callback accepts an optional `redirect_uri` resolved with the same precedence as authorize, so the presented URI must equal the issued one. Redirect targets must have an origin on a server-side allowlist derived from the origin config the server already trusts — no parallel setting was added: `OXO_FLOW_ALLOWED_ORIGINS` (or the identical localhost defaults the CORS layer uses, via the new shared `server::configured_origins()`), the origin of `OXO_FLOW_OAUTH_REDIRECT_URI` (or the documented fallback), and the `OXO_FLOW_PUBLIC_URL` origin. Matching normalizes scheme/host/port (default ports dropped, path ignored), rejects userinfo and non-http(s) schemes; an off-list target answers `400 OAUTH_REDIRECT_NOT_ALLOWED` at authorize and again at callback (defense in depth). Evidence tests: `state_issued_for_one_provider_is_rejected_on_another_providers_callback`, `state_issued_with_one_redirect_is_rejected_when_callback_presents_another`, `replayed_state_is_rejected_on_the_callback`, `expired_state_is_rejected_even_when_bindings_match`, `off_allowlist_redirect_target_is_rejected_with_400`, `allowlisted_origins_from_env_and_public_url_are_accepted` (`crates/oxo-flow-web/tests/oauth_state_binding.rs`); `well_formed_oauth_flow_mints_a_session` (`crates/oxo-flow-web/src/domains/auth/handlers.rs`; cfg(test)-only stub provider, asserts a persisted session); `test_oauth_state_binding_rejects_provider_and_redirect_mismatch`, `test_url_origin_normalization`, `test_redirect_allowlist_matches_origins_not_paths` (`service.rs`). Docs: `docs/guide/src/reference/web-api.md` (state binding + redirect allowlist), `web-system-architecture.md` (error-code table), `AGENTS.md` env rows. Commit binding: `bdafd9abc95f166b2d7018c52d463b22d6c99192`. **Browser-binding residual (issue #1197) FIXED:** `POST /api/auth/oauth/authorize` now also issues a per-flow random binding value as the `oxoflow_oauth_binding` cookie — `HttpOnly`, `SameSite=Lax`, `Path` scoped to the OAuth route prefix (the parent of the request path, base-path mounts included), `Max-Age=600` (= the 10-minute state TTL), and `Secure` only when the browser-facing callback origin is https (a plain-http localhost flow never carries it, or the browser would drop the cookie). Only the SHA-256 digest is persisted, in a new `oauth_states.binding_hash` column added by idempotent SQLite and PostgreSQL migrations; historical rows default to `''` and can never verify (fail closed, like the pre-#1141 `provider=''` rows). The callback requires the presented cookie to hash to the stored binding; the binding check runs FIRST among the binding checks, so a missing/mismatched cookie — or a legacy row — answers the same structured `400 OAUTH_STATE_MISMATCH` as a provider/redirect mismatch, with a message that discloses no component, and the existing consume-first `DELETE … RETURNING` ordering still spends the state (a mismatch consumes it; single use preserved). Every handler-generated callback response — success and failure — clears the cookie with the same `Path` (a request rejected by body extraction answers before the handler, so its short-lived cookie simply expires). Evidence tests: `state_issued_in_one_browser_is_rejected_from_another` (no cookie and another browser's cookie both answer `OAUTH_STATE_MISMATCH`; replay with the original cookie then answers `OAUTH_INVALID_STATE`, proving the mismatch consumed the state; the response clears the cookie), `legacy_state_row_without_a_binding_is_rejected`, `authorize_issues_a_scoped_httponly_binding_cookie` (`crates/oxo-flow-web/tests/oauth_state_binding.rs`); `test_oauth_state_is_bound_to_the_initiating_browser`, `test_legacy_state_rows_without_a_binding_never_verify`, `test_oauth_binding_cookie_contract` (`service.rs`); `well_formed_oauth_flow_mints_a_session` now threads the issued cookie through authorize → callback (session still persisted) and asserts the callback clears it (`handlers.rs`); the pre-existing replay/provider/redirect/expiry/allowlist tests still pass with the cookie threaded through. Fail-first verified locally (not committed): with the binding check disabled, the cross-browser HTTP callback reached the token exchange (`OAUTH_CALLBACK_ERROR` instead of `OAUTH_STATE_MISMATCH`) and the service unit test failed on "a callback without the binding cookie must be rejected". Scope stated honestly in the docs: this rejects cross-browser completion and cookie-less replay; it is not a claim that login CSRF is impossible in every threat model. Docs: `docs/guide/src/reference/web-api.md` → "Browser binding (issue #1197)", `web-system-architecture.md` error-code row. Commit binding: `6684545628cc6a0e895e356372dbad6cbc3d3fed`. |
| WEB-17 | P2 | PostgreSQL run lifecycle code exists, but live production parity is unproven and the middleware/error naming still says SQLite. | Implemented but unproven | Rename the capability boundary after evidence; run contract and crash tests on both backends. Evidence: `crates/oxo-flow-web/src/server.rs:498`, `crates/oxo-flow-web/src/infra/db/mod.rs:75`. |
| WEB-18 | P2 | Integration tests disable the run limiter through a debug-build seam rather than testing the production budget composition. | Fixed (#1192) | The limiter is now an instance (`RunRateLimiter`, `crates/oxo-flow-web/src/domains/execution/run_limit.rs`) with an injectable `Clock`: `server::build_router` installs the process-global production instance (`run_limit::global()`) as a request extension, and every spawn handler (`create_run`, `retry_run`, `resume_checkpoint`) resolves it from extensions and fails closed with a structured `500 INTERNAL_ERROR` when the wiring is missing. The debug-only `RunRateLimiter::for_tests(max, clock)` + `server::build_router_with_run_limiter(mode, limiter)` entry points feed the router-level test `enabled_run_limiter_is_composed_through_the_router` (`crates/oxo-flow-web/tests/run_limiter_composition.rs`), which drives the real team-mode router with real auth identities and an enabled limiter (injected budget 2/min, env budget set to 100 to prove the instance wins): two allowed attempts, then `429 RUN_RATE_LIMITED` with the countdown in `message` and no `Retry-After` header, a second identity keying its own budget, and `TestClock` advance past 60 s refilling the window. Negative controls verified locally: removing the extension layer fails the test (`500 Server misconfiguration: run rate limiter unavailable`); removing the `check_spawn_budget` call fails it (no 429). Unit tests: `run_limit::tests::{fixed_budget_allows_up_to_limit_then_blocks_with_wait, budgets_are_keyed_per_identity, advancing_the_clock_refills_the_window, zero_budget_disables_the_limiter}`. The `#[cfg(debug_assertions)]` `set_run_rate_limit_for_tests` seam stays because the existing multi-run integration suites still use it to zero the process-wide window; its docs point at the router-level test for the real composition. Docs: `docs/guide/src/reference/web-api.md` (Create Run rate-limit contract). Commit binding: `97a8dd8ef4b00a999b58e8bf6a540b32a2a15ce5`; PR #1192. |

### 5.4 Frontend, Editor, Desktop, and UX

| ID | Severity | Finding | Current maturity | Evidence and remediation |
| :--- | :---: | :--- | :--- | :--- |
| UX-01 | P1 | Guided editor round-trip silently discards advanced TOML semantics. | Fixed (#1198) | Re-confirmed on `main` @ `0ad50ade3`: the audit's `:47`/`:73` had drifted — write-back now splices (`cardsToToml` in `frontend/src/components/GuidedRuleBuilder.tsx`) and carries most unmanaged TOML through, but managed keys are still regenerated from the card model, so the loss path persisted. Modeled: `[workflow]` name/version (other workflow keys such as `description` are preserved) and per-rule `name`/`shell`/`input`/`output`/`threads`/`memory`/`environment` (plus the `resources.threads`/`resources.memory` spellings). Unmodeled and now refused: every other top-level section (`[engine]`, `[[values]]`, `[config]`, `[ai]`, `[defaults]`, …), every `[rules.*]` sub-table (except a threads/memory-only `[rules.resources]`), unmodeled rule keys (`when`, `depends_on`, `script`, hooks, retries, staging, tags, `benchmark`/`log`, `priority`, `target`, `params`, …), table forms of `input`/`output`/`environment` (map/dir outputs previously lost their keys), root-level keys, and header-shaped lines the TOML grammar cannot parse. Header recognition is total over TOML-legal spellings — inner whitespace (`[[ rules ]]`, `[ engine ]`), quoted key parts (`[rules."resources"]`), dotted keys, whitespace around statement dots, and CRLF — so a nested header can never be swallowed into the previous section span, and header-looking lines inside multi-line strings stay string content (fixed from the 2026-11-18 review finding). Mechanism: `detectGuidedUnsupported` (`frontend/src/utils/guidedSafety.ts`, reusing `scanTomlTopLevelSections`/`splitSectionStatements`/`statementKey`) drives BOTH surfaces — `GuidedRuleBuilder` replaces the form with a `role="alert"` refusal naming each construct plus a Switch-to-TOML button, and `PipelineEditor.handleSave` refuses in guided mode with a result-bar message before any API call; guided edits also refuse to emit before the async import resolves or on parse failure. The Canvas + TOML save path is unchanged. Tests (`frontend/e2e/guided-roundtrip.spec.ts`): `Guided mode refuses lossy saves (#1143) › advanced semantics: refusal names the loss, save makes no API call, TOML unchanged` (fixture with `[[values]]` + `[config]` + `[engine]` + `when` + `depends_on`; asserts the specific labels, no editable form, zero `POST /api/pipelines`, and the intact TOML after switching) and `Guided mode refuses lossy saves (#1143) › basic workflow still saves through guided mode` (created payload carries the full rule set); adjacent editor specs 18/18 pass. Detector unit coverage: `frontend/e2e/guided-safety.spec.ts` `Guided safety detector (UX-01)` table-driven cases `basic workflow (modeled keys only) is allowed`, `spaced [[ rules ]] header still exposes its rule fields`, `spaced [ engine ] header is a section, not swallowed workflow body`, `quoted [rules."resources"] sub-table is recognized`, `quoted resources sub-table with only threads/memory is modeled`, `CRLF documents with spaced headers are read identically`, `header-looking lines inside multi-line strings stay string content`, `dotted [rules.resources] sub-table is recognized`, `mixed indexed and spaced headers attribute fields to the right rule`, `unparseable header-shaped line fails closed`, `whitespace around a dotted statement key is normalized`, `escaped quoted statement key normalizes to its semantic name`, `empty document has nothing to lose`, `root-level keys are unmodeled`; e2e case `Guided mode refuses lossy saves (#1143) › spaced [[ rules ]] / [ engine ] headers cannot bypass the refusal`. Docs: `docs/guide/src/how-to/web-editor.md` → "Guided mode refuses documents it cannot represent"; the VS Code extension has no guided builder, so its reference is unchanged. Commit binding: `698ebf5a864c877bcee8e8fd772b6fb5a3a278b4` (PR #1198). |
| UX-02 | P1 | Browser-wide drafts and chat state survive logout and can leak between accounts. | Fixed (#1203) | Re-confirmed on `main` @ `a3375319e`: `PipelineSession.tsx` persisted one browser-wide `oxo_session` blob (draft TOML + chat transcripts), `Layout.tsx` logout removed only `oxo_token`, and `WorkflowCanvas.tsx` carried a second browser-wide key (`oxo-canvas-pos-<scope>`) whose payload holds the draft's rule names. **Mechanism**: one module, `frontend/src/utils/identityStorage.ts`, now owns every account-scoped key — `oxo_session::user:<encodeURIComponent(username)>` for draft + chat, `oxo_session::guest` for anonymous use, `oxo-canvas-pos::<identity>::<draftScope>` for node positions — plus `oxo_identity`, the server-stated username cached only as a label; it is written and cleared in the same operations as `oxo_token` (`POST /api/auth/login` response in Login.tsx, `clearIdentityScopedStorage()` on logout, `forgetSessionIdentity()` on a rejected session). `PipelineSessionProvider` **confirms before it paints and fails closed**: when the browser holds a token or claims an identity, no bucket is read and nothing renders until `GET /api/auth/me` names the token's owner, and ONLY a confirmed match restores a bucket (a mismatch is corrected without ever rendering the wrong account's draft). If confirmation does not arrive — network error, or no answer within 5 s — the app renders an explicit "Session not confirmed" notice with NO drafts and never falls back to the cached identity's bucket; a late answer is still adopted (promoting the notice to the confirmed account, or to the guest session for "nobody") and the notice offers Retry plus Sign in again, which drops the token + cached identity but keeps every bucket so the draft returns with the same account. A sign-out or account switch in ANY tab is authoritative: the identity-change path suppresses all writes before its reload branch, re-points the session refs synchronously, and reloads to re-initialize from the new bucket, so a background tab's reload/unload flush cannot rewrite what the sign-out removed; independently, `saveState` refuses any non-guest bucket that no longer matches the browser's current identity, and persistence is disabled entirely while the identity is unconfirmed. Logout calls `clearIdentityScopedStorage()`, which suppresses further writes FIRST (the `beforeunload` flush can no longer recreate the keys), then removes the token, the hint, EVERY `oxo_session` / `oxo_session::*` bucket (all identities, not only the current one) and every `oxo-canvas-pos*` key; an emptied session is stored as absent (`removeItem`) instead of an empty blob. A rejected session (401) drops token + hint but keeps the bucket, so the same user's unsaved draft returns on re-login; the unattributable pre-#1144 browser-wide blob is discarded on first load. Not scoped by design (device preferences, not account state): theme, language, sidebar collapse, editor mode, onboarding. **Evidence tests** (`frontend/e2e/identity-scoped-drafts.spec.ts`, real personal-mode server; per-user keys asserted as LITERALS so a collapsed identity mapping fails the test): `logout wipes identity-scoped state and a second login cannot read it` — accounts minted through `POST /api/users`, the login form, a draft typed into the shipped CodeMirror editor, chat written through the real SSE handler (network-level mock exactly like `chat-tool-cards.spec.ts`); A's draft and chat land in `oxo_session::user:<A>`, logout leaves ZERO identity-scoped keys after the unload window, and B (second login, same browser) sees neither A's draft in the editor nor A's transcript in chat while A's bucket is gone; `switching accounts without signing out re-keys the live session` — B signs in from the login page while A is still signed in, the live session moves to B's bucket, A's draft is never rendered, and A's bucket survives intact on disk; `a stale identity hint never paints a foreign bucket's draft` — forged hint plus planted foreign bucket, with a paint probe (MutationObserver attached after DOM readiness + a 25 ms sampler, plus an OWNER-marker anti-vacuity control that fails the test if the probe is dead) recording that the foreign TOML never reaches the DOM while the owner draft does; `an unconfirmed identity fails closed: no foreign paint even past the cap` — the confirmation request is held past the 5 s cap, the notice renders with no editor and no foreign/owner paint, then the released answer promotes it to the confirmed owner's draft; `an unreachable identity check fails closed and retry adopts the confirmed owner` — `/api/auth/me` aborted: notice, no drafts, the forged hint is NOT trusted, and Retry (server reachable again) renders only the confirmed owner's draft; `signing out in one tab is not undone by a background tab` — two real tabs share the profile and after the foreground signs out the background tab's reload AND close leave A's bucket absent and no user bucket behind. Negative controls verified: running the two fail-closed specs against the previous head's provider (the 5 s cached-identity fallback) fails at the paint probe with `foreign TOML painted before confirmation: {"foreign":true,"owner":false}` — the old behavior really paints the foreign bucket and the probe is live — and restoring the pre-fix provider for the two-tab spec fails with A's bucket rewritten after sign-out. `frontend/e2e/core-flows.spec.ts` seeded the old unscoped key and now seeds the guest bucket through the shipped helper (assertions unchanged). Docs: `docs/guide/src/how-to/web-editor.md` → "Browser storage and account switching"; `AGENTS.md` frontend key-components. Remaining boundary (documented trade-off): a signed-in page load spends one `/api/auth/me` roundtrip behind a blank first paint, and when the server cannot confirm the account the app shows the notice rather than any offline draft. Commit binding: `35c6b02465804556855e09432c9049a43ba574e3`; PR #1203. |
| UX-03 | P1 | Validation-request failure can leave a stale successful validation authorizing execution. | Fixed (#1200) | Re-confirmed on `main` @ `a3375319e`: the save path's catch (`frontend/src/pages/PipelineEditor.tsx:602` — the audit's `:716` had drifted) was `} catch { /* keep the last known validation */ }`, so a network error or 5xx on `POST /api/pipelines/validate` retained the previous `{valid: true}` state — the editor kept a green "Valid" badge and an enabled Run button on content that was never judged — and the save still reported success. Mechanism (frontend only): the editor's validation state is now explicitly three-way (`valid` / `invalid` / `unavailable`); one helper, `markValidationUnavailable`, forces `{valid: false, unavailable: true}` with a `VALIDATION_UNAVAILABLE` finding and is called from BOTH failure paths — the debounced `buildDag`+validate (`updateDag`, whose catch now also honors its monotonic `editSeq` guard so a superseded failure cannot clobber a newer verdict) and the save path's exact-content validation. The save path no longer retains the prior verdict: it invalidates, refuses the save ("Not saved: validation could not be confirmed — <reason>") and returns, so no success result and no pipeline row are produced for unvalidated content. The badge gains the third state ("Not validated", `.val-badge.unavailable`, warning color), Run/Dry-Run gate on `validation?.valid` (false when unavailable), and `handleRun` additionally re-checks the verdict so an already-open Run dialog cannot launch from a missing/unavailable state. A genuinely successful validation restores the affordances; a findings response behaves exactly as before (error count badge, error list, refused save). Evidence tests (Playwright against the real server; only `POST /api/pipelines/validate` is stubbed): `a 5xx validation response clears the Valid badge and blocks Run until validation succeeds` (asserts no `.val-badge.valid`, the "Not validated" badge, `VALIDATION_UNAVAILABLE` in the error panel, disabled Run, no pipeline created by the refused save, then recovery), `a network-aborted validation request fails closed the same way, then recovers` (`route.abort`), `a findings response still renders as validation errors and recovers on a clean validation` (unchanged findings semantics, "Not validated" asserted absent) — `frontend/e2e/editor-validation-failclosed.spec.ts`. Negative control: with the pre-fix `PipelineEditor.tsx` swapped back in and the SPA rebuilt, the 5xx and network-abort tests fail on `locator('.val-badge.valid')` resolving to 1 element (the stale badge) while the findings test still passes. Docs: `docs/guide/src/how-to/web-editor.md` → "Validation, dry-run, run" (fail-closed paragraph). Commit binding: `b1fcf560c5ecc58004d06de7c4736898587e6f66`; PR #1200. |
| UX-04 | P1 | "Apply & Retry" does not apply the displayed repair command. | Fixed (#1201) | Probed the repair payload before changing the UI: the diagnostics `fix_action` is advisory by construction — `ConfigChange.old_value`/`new_value` are placeholders ("current" → "increase", `crates/oxo-flow-core/src/diagnostics.rs` doc comment), every producer sets `command: None`, and no server endpoint applies it (`POST /api/runs/{id}/retry` accepts only `skip_succeeded`/`from_rule`; the web crate has no repair route). A client-side "apply" would therefore have been theater, so the monitor now renders the payload as "Suggested fix (apply manually)" (description + `path: old → new` + command when one exists) and labels the action "Retry unchanged" — the hint states it re-executes the original snapshot in the same workdir, so applying the suggestion requires a NEW run, not an edit-then-retry; the AI-alert card's "Fix & Retry" (same false affordance) is likewise "Retry unchanged". Regression tests (fetch-intercepted failure panel and an auto_fixable alert card; assert the advisory payload is displayed, exactly one POST — `/api/runs/{id}/retry` with `{"skip_succeeded":true}` — reaches the server, and the RetryPlan is honored): `frontend/e2e/monitor-apply-retry.spec.ts` → `advisory fix is displayed and Retry unchanged sends only the retry request`, `no repair payload: no suggestion block, Retry unchanged still sends no repair request`, `alert card action is relabelled Retry unchanged without the manual-fix caption`; all three fail on the pre-fix tree (no `retry-unchanged` control; the alert button still reads "Fix & Retry"). Docs: `docs/guide/src/reference/diagnostics-engine.md` → "Failure-panel actions in the web monitor"; `docs/guide/src/reference/web-api.md` → Smart Retry (accepts no repair payload). Commit binding: db315f32 (frontend + tests + docs; the pre-squash code commit of this PR). |
| UX-05 | P1 | AI chat replaces editor content before explicit acceptance. | Fixed (#1206) | Re-confirmed on current `main` @ `f51d0e2ad`: `onPipelineReady` called `setToml(data.toml_content)` with no gate (`frontend/src/pages/PipelineEditor.tsx:880` — the audit's `:923` had drifted to the ChatUI mount site). Mechanism (frontend only, no new dependency): `stagePipelineProposal` queues the generated TOML as a pending `AiProposal`; a line diff (`frontend/src/utils/tomlDiff.ts`, LCS with prefix/suffix trimming and a table-size cap) against the LIVE editor content is rendered above both view modes, and the proposal's TOML is applied only via **Apply to editor** (`acceptActiveProposal`, which also bumps `editSeq` so an in-flight canvas edit cannot overwrite the accepted content). **Discard** removes only the proposal on screen and never touches editor state, so unsaved work survives byte-identically. A proposal arriving while one is pending is appended without changing the displayed one — it never sets `activeProposalId`, and the displayed proposal falls back to the first pending entry, so the panel keeps reporting "Proposal 1 of N" plus an explicit **Review next proposal** switch. **Apply consumes only the accepted entry**: queued proposals survive with stable staging ordinals (a surviving second proposal still reads "Proposal 2 of 2") and the next one becomes active — the initial revision cleared the whole queue, silently dropping the rest (PR #1206 review). Proposals are component state, so nothing is applied on navigation/unmount, and Chat's own Accept no longer pre-applies the TOML to the shared session when the host passes `onPipelineReady` (the editor would otherwise restore it on remount without a diff acceptance). The same review found the stale-writer class still unguarded on Format and revision load/rollback: both now take the existing `editSeq` guarded-writer ticket (`runEdit`'s pattern), so a slow response cannot overwrite a newer edit such as an accepted proposal. Tests (`frontend/e2e/ai-proposal-gate.spec.ts`, Playwright against the real server; only `/api/ai/config` + `/api/chat/send` stubbed at the network layer, plus `/api/pipelines/format` for the delayed-response case): `generated output does not mutate the editor until Apply`, `Discard leaves the editor byte-identical`, `a pending proposal never overwrites unsaved edits without acceptance`, `a second proposal queues without silently replacing the pending one`, `Apply keeps the remaining queue and the next proposal is still reviewable`, `Discard in a queued pair removes only the active proposal`, `a slow format response cannot overwrite an accepted proposal`, `navigating away does not auto-accept a pending proposal` — 8/8 pass. Negative controls (temporary reverts of the uncommitted working tree, SPA rebuilt each time): with the pre-fix direct `setToml(proposed)` reintroduced, all 5 original tests fail (editor mutated before acceptance; the queued proposal overwrote the pending one; the revisited editor restored the AI content); with Apply again clearing the queue, the queue-survival test fails on the vanished proposal; with the format guard removed, the delayed formatted text overwrites the accepted content (`STALE_FORMAT_MARKER`). Adjacent editor specs (canvas / console / validation-failclosed) 12/12 pass. Docs: `docs/guide/src/how-to/web-editor.md` → "AI assistant: proposals require acceptance". Commit binding: `6b56d884d3f9eb1bed0f1eaf591eade047cd0327` + review follow-up `3e6343ae1fdf85bbd37d08c10018aaa81c3f4941` (PR #1206). |
| UX-06 | P1 | Metro grouped editing is not semantics-preserving; stations lack full keyboard access. | Fixed (#1211) | Re-confirmed on `main` @ `4e424f63`: the audit's `:147` is now the zoom-band memo and `:316` is the `onConnect` body; the live defect was the station action set in grouped granularities. At Process/Module a station id is an environment/modality aggregation key, not a rule name, so the station double-click opened the rule inspector for a non-existent rule (and a rule literally named like the key, e.g. `system`, would have been silently targeted instead of the group), and `onConnect` forwarded group keys to the `connect` edit. **Safe route taken: grouped views are read-only.** `MetroCanvas` carries a `MetroInteractionContext` with `interactive = editable && granularity === 'rule'`; grouped stations render `role="group"` + `data-grouped="true"` + a `read-only` badge, non-connectable handles (`isConnectable={false}` — they must stay mounted or React Flow cannot position an edge, so grouped transit edges keep rendering while connections stay impossible), no double-click handler, `nodesConnectable=false`, a toolbar note, and Enter/Space only announces the read-only reason; grouped Delete/Backspace is `preventDefault`ed AND `stopPropagation`ed so React Flow's document-level `deleteKeyCode` (`Backspace`) cannot delete the selected glyph. **Rule granularity is a complete keyboard surface:** stations are the single tab stop per node in rule-declaration order (`nodesFocusable={false}` removes React Flow's inert wrapper stop); `Enter`/`Space` opens the inspector, `c` starts a keyboard connection announced through a `role="status"` live region and completed with `Tab`+`Enter` through the same `connect` edit the drag uses, `Escape` cancels/closes, `Delete`/`Backspace` removes the focused rule (`onRemoveRules` wired through `WorkflowCanvas`); connect/remove announcements are emitted only after the async edit resolves (`runEdit`'s null = rejected or superseded), so a cycle-rejected connect announces "was not applied" rather than a premature success, with `aria-label`/`aria-keyshortcuts`/`aria-describedby`. Two pre-existing CSS defects surfaced while exercising the toolbar: the unscoped `.rf-metro-btn` rule in `frontend/src/index.css` absolutely positioned every button at `top:10px;right:10px`, stacking the three granularity buttons (only the topmost was mouse-clickable) and, in the standard view, burying Metro View under Auto layout so the metro canvas was mouse-unreachable in the editor; both toggles are now normal flex children of their (single, absolutely positioned) containers, and the read-only note sits on its own bounded toolbar row so the toolbar cannot widen past the canvas over the left rail. **Tests** (`frontend/e2e/metro-canvas.spec.ts`, real personal-mode server, serialized `CI=1`): `keyboard-only: Tab order reaches stations, Enter/Space open the editor, Escape closes it` (includes the mouse toggle round-trip), `keyboard connect and delete edit exactly the focused rule; Escape cancels a pending connection`, `grouped granularities are read-only; the advanced document survives every grouped interaction` (fixture with `when` + `depends_on` + `[engine]` + `[config]` + `[[values]]`; asserts grouped roles/flags, non-zero transit edges with no `.connectable` handles, the read-only note, double-click/Enter/Space/C/Delete/Backspace leave the TOML byte-identical, a selected grouped station survives Backspace/Delete, and Rule granularity opens the exact rule; the connect test also starts a cycle-forming connection and asserts the settled failure announcement). Fail-first: with `MetroCanvas.tsx`/`WorkflowCanvas.tsx` reverted to pre-fix (CSS/docs retained) and the SPA rebuilt, all three tests fail; a pre-fix probe on the advanced fixture recorded a grouped Process station double-click opening `#rule-inspector-title` = "Rule: system" — the aggregation key, not a rule; review probes recorded handle-less grouped stations dropping every transit edge (2 edges/4 handles → 0/0) and a selected grouped station being deleted by Backspace (2 → 1, TOML untouched), and temporarily dropping the grouped `stopPropagation` reproduces the latter against the new assertion. Docs: `docs/guide/src/how-to/web-editor.md` → keyboard editing paragraph, keyboard connect steps, "Metro view and grouped granularities". Commit binding: `994f850cb630109fbd59061504fde8577e1ca443` (PR #1211). |
| UX-07 | P2 | Monitor process grouping collapses all environments to `system`. | Fixed (#1209, #1210) | Re-confirmed on `main` @ `4e424f633`: the audit's `:1021` had drifted — the hard-coded `environment: 'system'` was at `frontend/src/pages/MonitorReport.tsx:1149`. **Mechanism**: `GET /api/runs/{id}/dag-status` gains a per-node `environment: Option<String>` (`crates/oxo-flow-web/src/domains/execution/{handlers,types}.rs`) resolved from the run's own pipeline snapshot the way the RUN resolves it — `[defaults] environment` folded onto rules by the shared `WorkflowConfig::apply_defaults` (the same call `run`/`dry-run` make), then `rule.environment.kind()`. `env_group` is expanded here since issue #1210: `WorkflowConfig::apply_defaults` now folds the documented precedence (`env_group` → the rule's inline environment → `[defaults] environment`) through the shared `effective_rule_environment` helper that `resolve_environment` also uses, so the monitor's fold is exactly what the executor wraps (the earlier `env list` docker / `dry-run --json` venv measurement was the inconsistency #1210 fixed). A node that no longer maps to a snapshot rule (orphaned instance of an edited workflow, kept by `with_all_rules`) answers `null`; `MonitorReport.tsx` renders it as `unknown` with a neutral dot (`index.css` `.rf-env-unknown`), never a guessed `system`. **Evidence tests**: `dag_status_reports_each_rules_real_environment` (multi-environment fixture: defaults venv + inline docker + inline singularity + downstream inheriting defaults; asserts each node and that none is `system`), `dag_status_reports_system_only_for_a_rule_that_runs_in_system`, `dag_status_leaves_the_environment_null_for_an_orphan_instance` (`crates/oxo-flow-web/tests/dag_status_environment.rs`, production router) — all three fail on the pre-fix handler (nodes carry no `environment`); `every node shows the environment resolved from the run snapshot` (`frontend/e2e/monitor-dag-environment.spec.ts`) drives a REAL `POST /api/runs` dry-run with system/conda/docker rules, asserts each painted `.rf-env-label` and that `system` appears exactly once — fails on the pre-fix canvas (rebuilt with the hard-coded line restored: `conda_align` rendered `system`). **Docs**: `docs/guide/src/reference/web-api.md` (DAG Status: field, derivation, `env_group` caveat, orphan `null`), `docs/guide/src/reference/web-system-architecture.md` (SPA pattern "Truthful environment labels on the monitor canvas"). Commit binding: `638561dedd4dd8eaca98906631ed0331180392bf`; PR #1209. **#1210 addendum**: commit binding `5243d3e6c9105c848555087cbe916d96e68fd3ba`; PR #1215 — `env_group` precedence materialized in `apply_defaults` via the shared `effective_rule_environment` helper (`resolve_environment` delegates to the same function), so `env list`/`env check`, `run`, `dry-run`, cluster submission, and this DAG-status value agree. Evidence tests: `apply_defaults_folds_env_group_over_inline_environment` (core), `env_list_and_dry_run_json_agree_on_env_group_precedence` (CLI), `dag_status_reports_the_env_group_backend_the_run_executes` (web) — all three fail with the pre-fix fold restored. Docs: `docs/guide/src/reference/workflow-format.md` (Named Environment Groups), `docs/guide/src/commands/env.md`, `docs/guide/src/reference/web-api.md` + `web-system-architecture.md` (this caveat). |
| UX-08 | P2 | VS Code writes AI output before validation and diagnoses disk content while unsaved buffers are shown. | Fixed (#1212) | Re-confirmed on `main` @ `4e424f63`: the audit's `:559` had drifted to ~`:572` (editor-model path wrote the TOML to the destination before `oxo-flow validate`, and a repair round could write an unvalidated file) and `providers/diagnostics.ts:71` (now `:76`) ran `validate`/`lint` on `doc.uri.fsPath` while the buffer could be dirty — type mode made stale-disk reporting the norm, and the Validate command analyzed the saved path with only a warning. **Mechanism**: one snapshot primitive, `editors/vscode/src/core/contentSnapshot.ts` (`withContentSnapshot`), stages content as a hidden sibling `.oxoflow` (same directory keeps relative `[[include]]`/sample-pattern resolution; the staging write is inside the cleanup guard, so a partial file from ENOSPC/quota is removed too, and the snapshot is always deleted on throw). AI (`core/aiDraft.ts` `generateValidatedDraft`): model response → `extractToml` → the draft is validated on a snapshot through the EXISTING CLI path (`validateArgs` + `runCli`, no duplicated validation logic) → on failure exactly one repair round receives the STRUCTURED findings parsed from the `validate --json` report on stdout (stderr only carries the "validation failed: N errors" bail summary; code, message, rule and hint are rendered, so the model is not asked to fix blind) → the repaired draft is re-validated under the same gate → only a PASSING pipeline reaches the injected `apply` callback; a rejected draft (including a repair that returns no pipeline) is surfaced with the same findings and the full rejected TOML in the oxo-flow output channel, and the destination — including an existing file — is left untouched. Unfixable conditions — spawn failure (bad `oxo-flow.executablePath`), validate timeout, staging I/O failure — are classified `unavailable`: the real cause is surfaced, nothing is written, and the single repair round is NOT consumed on them. Diagnostics (`providers/diagnostics.ts` `diagnose`): `doc.isDirty` selects the BUFFER (staged via the same primitive) instead of the disk path, findings anchor against `doc.getText()`, and if staging fails an explicit `UNSAVED` save-first warning replaces the results — it never falls back to the saved file. `runQualityCommand` in `extension.ts` routes the Validate/Lint commands through the same snapshot when the buffer is dirty (its old "(document has unsaved changes — the saved file was analyzed)" warning is gone). The `**/*.oxoflow` file watcher ignores hidden snapshot basenames through the shared `SNAPSHOT_PREFIX`/`isSnapshotBasename` predicate, so staging never triggers a CLI probe via `statusBar.refresh()`. **Evidence tests**: unit `src/test/unit/aiDraft.test.ts` — `a passing first draft is validated on a snapshot BEFORE apply` (asserts the CLI receives the snapshot content, the target does not exist during validation, snapshot removed), `invalid draft findings reach the repair prompt and the user report` (real-shaped `validate --json` stdout: E007 code, message, `(rule: r1)` and the hint must reach BOTH the repair prompt and the rejected-draft report, and the stderr bail summary must not be the finding), `an invalid repair is rejected with its own structured findings`, `a repaired, re-validated draft is the one applied` (two CLI runs), `a repair round with no pipeline is rejected, not applied`, `a response without a pipeline is surfaced without validation or apply`, `a spawn failure is unavailable, names the cause, and burns no repair round`, `a validation timeout is unavailable, not a rejected draft`, `an unstageable draft (read-only/missing directory) is unavailable`, `validateDraftContent separates invalid findings from infra failures`, `formatFindings renders code, rule, message and hint`; unit `src/test/unit/contentSnapshot.test.ts` — path shape/uniqueness, exact content exposed, cleanup on success and on throw, `withContentSnapshot removes a partial snapshot when the staging write fails` (write seam simulates ENOSPC after a partial write; the body never runs and the file is removed), `isSnapshotBasename recognizes staging snapshots, never user workflows`; unit `src/test/unit/jsonReport.test.ts` — `saveFirstFinding is an explicit refusal, not a stale disk result` (pins the UNSAVED code, warning severity, save-first wording and null rule). Headless integration `src/test/integration/extension.test.ts` → `diagnoses the unsaved buffer instead of the stale saved file` (diagnosticMode `type`; a buffer-only `depends_on = [\"ghost_rule\"]` must yield E007 anchored to the buffer's rule line while the on-disk fixture is asserted still valid). Fail-first: restoring HEAD's `providers/diagnostics.ts` makes that integration test time out (60 s, no E007) and the fixed tree passes 10/10; a temporary stderr-first detail variant fails `invalid draft findings reach the repair prompt and the user report`; moving the staging write outside the guard fails `withContentSnapshot removes a partial snapshot when the staging write fails`; the AI ordering itself is pinned by the unit guard (the headless host exposes no language models, so the command cannot be driven there). **Docs**: `docs/guide/src/reference/vscode-extension.md` → Diagnostics ("Unsaved buffers are diagnosed as they are shown", `UNSAVED` refusal) and new "AI pipeline generation" (validate-before-apply, structured findings from `--json`, repair re-validation, infra failures reported as such, destination untouched); settings and command-table rows updated. Commit binding: `49f6d2bc88c5d11a703b7362d935195651f111f2` (review-fix code commit, rebased onto current `main`); PR #1212. |
| UX-09 | P2 | Mobile, accessibility, Metro performance, and desktop lifecycle coverage is insufficient. | Planned / blocked | Add browser E2E, mobile viewport, WCAG, frame-time, keyboard graph navigation, and lifecycle tests. |

### 5.5 Evaluation, Hardware, and Science

| ID | Severity | Finding | Current maturity | Evidence and remediation |
| :--- | :---: | :--- | :--- | :--- |
| EV-01 | P1 | Detection, attribution, repair proposal, repair application, verification, and successful rerun were previously conflated. | Measured on a live campaign (2026-11-18, GLM via school proxy, 10 seeded cases) | Stages reported separately; the verified-repair loop (#1185) measured repair applied 3/10 and verified reruns 3/10 — 3/3 of the repairable class — after the fence-corrective round (#1187) fixed the measured failure class (correct diagnosis, safe verdict, no toml fence). The other 7 cases carry per-case WHYs (4 abort at the BioTypes admission gate before any rule runs; 3 SCI preflight findings are advisory by design). Raw logs per case: eval/frontier/results/fault_repair_logs/; measured table: eval/frontier/PILOT_FINDINGS.md. |
| EV-02 | P1 | Statistical reports could present synthetic data as inferential evidence. | Implemented and verified for fail-closed tooling | Missing matched three-arm data now fails closed; synthetic mode is explicitly non-inferential. |
| EV-03 | P1 | Hardware projections were presented beside host measurements. | Implemented and verified for labeling | Model output now says `MODEL PROJECTION` and `NOT MEASUREMENT`. No measured hardware study exists. |
| EV-04 | P1 | Cohort DAG counts were not derived from the real plan. | Implemented and verified for fail-closed parsing | Real `dry-run --json` plans are required; the current pilot is `N=10`, 30 nodes. |
| EV-05 | P1 | The implementation checklist marked unverified cloud, hardware, UX, and manuscript work complete. | Fixed in this audit wave | Replaced with maturity-labeled, unchecked gates. |
| EV-06 | P1 | Synthetic superiority and human-factors claims appeared in reports. | Fixed in this audit wave | Reports are now evidence-status documents, not result claims. |
| EV-07 | P1 | Fixtures could encode stale semantics: a passing suite certifies expectations that trace only to current engine output, not to a contract (audit wave 4). | Audited 2026-11-18 (#1168); four stale families corrected, one open suspect justified | Inventory: `docs/superpowers/audits/fixture-contract-inventory.md` — 20 fixture families, 18 traceable / 1 annotated / 1 suspect (open: `eval/gold/workflow.csv` `wf-004`, whose empty edge list traces to the unexpanded `graph -f dot` surface while the `scatter`/`gather` reference carries five runtime edges; fixing it needs the checker to use `--expanded` plus a human-attested evidence re-mint). Demonstrated stale fixture: `eval/interpretation/seeds/s04-silent-failure` expected `run` exit 0, but the fail-closed missing-source-input contract makes the silent exit-0 pipeline unreachable (the engine aborts with exit 1) and the benchmark runner marked the seed `broken` (excluded from the denominator) instead of failing. Corrected by producing the declared input upstream and planting the fault on an undeclared read (`gzip -cd {input[0]} data/reads/sample_R2.fastq.gz | wc -l`); the new provider-free gate `crates/oxo-flow-cli/tests/interpretation_seeds.rs` fails on the pre-fix seed (proven) and passes 12/12 after. Found and corrected an additional stale inline assertion: `tests/cli_integration.rs` expected the pre-#1122 prose (`AI provider not configured`) instead of the AI-09 structured degraded reason (`AI degraded [AI_NOT_CONFIGURED]` + `ai setup`); CI caught it on PR #1214 and the flake retry re-failed, so it was a stale failing fixture, not the passing-defect evidence (that remains `s04`). Also corrected stale stated counts against their fixtures: `eval/frontier/README.md` 29→36 intents / 10→12 domains, now gated by `eval/frontier/test_intents_contract.py` (fails on the old text; proven), and `benches/comparative/README.md` Snakefile 34→41 lines. Boundary: historical benchmark numbers (AI hit-rates, comparative timings, j-suggestion makespans) were not re-measured and no provider was called; the inventory records them as historical. Commit binding: ece15a26a28828f9608f5aa1cbd0fa10eaac5219 (PR #1214). |

### 5.6 CLI, AI, Provider, and MCP

| ID | Severity | Finding | Current maturity | Evidence and remediation |
| :--- | :---: | :--- | :--- | :--- |
| AI-01 | P0 | A model-approved partial TOML block can replace the whole workflow. | Fixed (#1117) | `--ai-recover` now rejects a patch that shrinks the original's rules/includes/references (checked through `WorkflowConfig::parse`) and writes accepted fixes atomically. See RB-11. |
| AI-02 | P1 | Rule shells inherit ambient credentials. | Planned / blocked | RB-15. |
| AI-03 | P1 | Workflow-scoped AI provider and model settings are resolved but ignored. | Fixed (#1122) for the CLI task surfaces | The CLI resolves the full chain before building the provider and applies the resolved provider/model (`create_provider_from_config_chain`) for `template --ai`, `env create --ai`, `ai explain`, and the `validate`/`lint`/`dry-run`/`debug`/`report`/`run`-recovery analyses; a saved config's endpoint/model/key never cross into a different provider. Scope note: the status commands (`oxo-flow ai`, `ai test`) take no workflow and report environment + saved config only; the web/desktop provider registry keeps its own precedence and is not covered. E2E precedence tests in `crates/oxo-flow-cli/tests/ai_policy.rs`. See RB-14. |
| AI-04 | P1 | A generic MCP token is sent to any activated public skill endpoint. | Fixed (#1123) | The generic `OXO_FLOW_MCP_TOKEN` is released only to exact origins listed in `OXO_FLOW_MCP_TRUSTED_ORIGINS` (unset/empty = never released; unparseable entries warned and ignored); per-server tokens are destination-scoped by construction. Tests cover unapproved, lookalike, and approved origins (wire-level for the first and the approved case) plus per-server precedence; the SSRF screen is unchanged. See RB-13. |
| AI-05 | P1 | Auto-approved project reads plus outbound fetch form an exfiltration chain. | Fixed (#1123) | Outbound egress is classified per invocation (`Tool::egress_target`; `fetch_url` = destination origin, MCP = opaque) and no longer inherits the read-only auto-approval: the call needs an explicit human approval, or its exact origin must be listed in `OXO_FLOW_AI_FETCH_TRUSTED_ORIGINS`; non-interactive sessions refuse. Refusals name the origin only. Tests: `egress_data_release_policy`, `read_then_fetch_cannot_reach_the_network_without_approval`, `egress_refusal_names_only_the_origin_and_never_the_payload`. See RB-13. |
| AI-06 | P1 | CLI and web admission evaluate different graphs when includes or overrides change the workflow. | Implemented but unproven | Direct TOML BioTypes now share the core gate; include/override admission parity is still absent. RB-04 defines the exit seam. |
| AI-07 | P2 | Malformed tool arguments are converted to `{}` and remain executable. | Implemented and verified for model-emitted arguments | Provider parsers pass model argument strings through raw (never repaired to `{}`), the orchestrator's JSON-validity gate rejects them with a fail-closed tool-result message before the registry, and `ToolRegistry::execute` re-checks. Wire-format echoes (Anthropic `tool_use.input`, Ollama `arguments`) keep a protocol-valid `{}` fallback but are never executed. MCP-side argument handling is unchanged. |
| AI-08 | P2 | `[ai] auto_fix` is documented but has no runtime consumer. | Fixed (verified in this wave) | Refuted by shipped code: `run/failure.rs` consumes `auto_fix_decision` (`never`/`ask`/`always` produce distinct write behavior). The RB-14 prose claiming otherwise was stale and is corrected there. |
| AI-09 | P2 | Enabled AI can silently become unavailable during analysis and recovery. | Fixed (#1122) | Enabled-AI surfaces now emit a structured degraded-mode reason (`AiDegradedReason`: stable code + provider/credential detail + suggestion) instead of silently skipping AI; hard-fail surfaces return it as the command error. See RB-14. |
| AI-10 | P2 | AI template generation overwrites existing destinations non-atomically. | Fixed (#1122) | `template --ai` refuses an existing destination without `--force`/interactive confirmation before any paid generation, and writes through the shared `oxo_flow_core::atomic_file` staged-write helper. See RB-14. |
| AI-11 | P2 | README and provider guide overstated autonomous safety. | Fixed in this audit wave | User-facing claims now describe repair as experimental; malformed arguments are fail-closed end to end (see AI-07). |
| AI-12 | P3 | Oversized-file guidance refers to an unavailable shell channel. | Planned / blocked | Direct the model to a bounded supported alternative and add a standard-registry test. Evidence: `crates/oxo-flow-ai/src/tools/builtin.rs:177`. |

The independent read-only review did not execute a live provider, MCP server, scheduler, cloud
backend, or remote host. Provider streaming termination, partial SSE frames, quota behavior,
non-text responses, and arbitrary secret transformations remain unproven.

### 5.7 Documentation and Claims

| ID | Severity | Finding | Current maturity | Remediation |
| :--- | :---: | :--- | :--- | :--- |
| DOC-01 | P1 | Phase 12 marked hardware, cloud, human-factors, and manuscript work complete. | Fixed in this audit wave | Roadmap now uses maturity labels and explicit exit gates. |
| DOC-02 | P1 | The specification described aspirational features as current implementation. | Fixed in this audit wave | Current baseline and evidence requirements are separated. |
| DOC-03 | P1 | The implementation plan checked nearly every task complete. | Fixed in this audit wave | All completion boxes remain open until evidence is attached. |
| DOC-04 | P1 | Synthetic reports asserted superiority. | Fixed in this audit wave | Replaced with non-inferential protocol/status reports. |
| DOC-05 | P2 | Cohort and hardware reports lacked provenance. | Fixed in this audit wave | Pilot scope and model-only status are explicit. |
| DOC-06 | P2 | The Linux validation command omitted the required simulation flag. | Fixed in this audit wave | Protocol now requires an explicit evidence-producing path. |

---

## 6. Stage Exit Gates

These gates govern progression. A stage cannot be skipped because a later prototype exists.

Every gate that claims clinical or regulatory fitness must additionally have
an approved traceability matrix linking requirements and risks to IQ/OQ/PQ
test IDs, evidence artifacts, deviations, and sign-off. The generic
`docs/VALIDATION_PROTOCOL.md` template is not such a matrix, and no IQ/OQ/PQ
execution or approval has occurred.

### Gate 0: Truthful Local Execution

- Atomic create, stage, persist, and spawn.
- Immutable run snapshots.
- Monotonic lifecycle transitions.
- Atomic uploads and verified output invalidation.
- BioType and config validation shared by CLI and web admission.
- Fault-injection evidence for every claim.

### Gate 1: Safe Team Operation

- OS/container execution isolation and minimal environment.
- Enforced CPU, memory, process, filesystem, and network limits.
- Durable quotas and authenticated downloads.
- Tenant-scoped templates, DAG history, drafts, chat, and artifacts.
- Fail-closed auth, audit, and revision writes.

### Gate 2: HPC and Cloud Qualification

- Live submit/queue/run/cancel/fail/retry/retrieve campaigns for every claimed backend.
- Per-element outcomes and truthful accounting.
- Restart-safe cancellation and artifact reconciliation.
- PostgreSQL parity, concurrency, crash-recovery, and horizontal-instance tests.

### Gate 3: Scientific Validity

- Pre-registered hypotheses and exclusions.
- Matched three-arm ablation data with run-level provenance.
- Direct same-workflow, same-data, same-host comparisons against pinned
  Nextflow and Snakemake implementations, including unsupported-construct
  coverage and negative results.
- Independent biological validation against truth sets.
- False-positive and false-negative analysis for every static contract.
- Reproduction on an independent host and software environment.

Wave-4 fixture evidence (2026-11-18, #1168): the fixture-contract inventory
(`docs/superpowers/audits/fixture-contract-inventory.md`) traces every
fixture family's expected value to its contract instead of to engine output.
False-positive/false-negative status per static contract is unchanged and
still requires the dedicated campaign above; what the inventory establishes
is narrower — that the fixtures which measure those contracts still reach
them. One open suspect (`wf-004` scatter/gather edge) is documented, not
silently accepted.

### Gate 4: Publication

- Measured hardware and cohort study, not projections.
- Real human-subjects protocol with ethics approval and complete analysis.
- Negative and null results reported.
- Independent replication or a declared replication package.
- Manuscript and artifact package reviewed against journal reporting standards.

---

## 7. Next Audit Waves

The following work remains serial and must not be hidden behind roadmap completion language.

1. Web remediation wave: isolation boundary, immutable run ledger, atomic admission, lifecycle CAS, safe uploads, and remote artifact verification.
2. Frontend remediation wave: semantic round-trip, diff acceptance, tenant-scoped browser state, stale-validation blocking, accessibility, and E2E.
3. CLI/AI remediation wave: block partial-file repair, isolate secrets and ambient environment,
   bind MCP credentials to trusted endpoints, close read-to-fetch egress, enforce `auto_fix`, and
   test provider precedence plus recovery through verified rerun.
4. Tests/benches/examples wave: proof that fixtures exercise real contracts and do not encode stale semantics. **Audited 2026-11-18 (#1168).** Artifact: `docs/superpowers/audits/fixture-contract-inventory.md` (20 families: 18 traceable, 1 annotated, 1 open suspect). Five families were suspect at audit time; four were corrected against their contracts (`eval/frontier/README.md` counts, `eval/interpretation/seeds/s04-silent-failure`, `benches/comparative/README.md` Snakefile line count, and the stale AI-explanation assertion in `tests/cli_integration.rs`) and one (`eval/gold/workflow.csv` `wf-004`) is annotated open because its correction needs a checker-surface change plus a human evidence re-attestation under #172. Demonstrated stale fixture and fail-first regression: `EV-07` in §5.5; gate `crates/oxo-flow-cli/tests/interpretation_seeds.rs`. Commit binding: ece15a26a28828f9608f5aa1cbd0fa10eaac5219 (PR #1214).
5. Scripts/data wave: provenance, reproducible regeneration, and removal of synthetic claims from publication-facing artifacts.
6. Linux qualification wave through the designated `tx-ubuntu` host only.
7. Scientific campaign design review before any large-scale compute or human-subjects expenditure.

---

## 8. Change Discipline

- Keep PR #997 in draft with the `[DO NOT MERGE]` title guard.
- Never enable auto-merge.
- Do not rewrite another branch or pull request as part of this program.
- Every remediation must add a regression test that fails on the original defect.
- Every benchmark artifact must record command, commit, host, software versions, seeds, inputs, exclusions, and raw run-level data.
- Do not translate a model projection, mock scheduler result, synthetic fixture, or smoke test into an empirical claim.
- Do not claim industrial-grade, Nature-tier, clinically qualified, formally verified, or production-ready status until the corresponding gate has passed.
