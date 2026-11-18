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

**Maturity**: Prototype

Admission checks and records are separate operations. Counters and reservations are process-local, reset on restart, and are not shared across instances. Declared resources are not enforced by cgroups or rlimits.

Evidence:

- `crates/oxo-flow-web/src/infra/quota.rs:61`
- `crates/oxo-flow-web/src/infra/quota.rs:85`
- `crates/oxo-flow-web/src/infra/quota.rs:128`
- `crates/oxo-flow-web/src/domains/execution/handlers.rs:420`
- `crates/oxo-flow-web/src/domains/execution/handlers.rs:605`

Required exit evidence:

- Atomic admission under concurrent requests.
- Durable and shared quota state.
- Enforced resource ceilings with observable rejection behavior.

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
  (SLURM/PBS/SGE/LSF) still materialize the literal — open.]
- Each backend obtains the value through a restricted secret-injection path. [Kubernetes carries a
  `secretKeyRef` to the documented `oxoflow-secrets` Secret; docker/singularity forward the routed
  name through the child environment. AWS Batch and GCP Batch have no engine-visible injection
  field (AWS: job-definition `containerProperties.secrets`; GCP: submission-time
  `secretVariables` needs the operator's project), so the operator-side mapping is documented and
  the reference fails closed rather than leaking. Not yet proven by a live campaign.]
- Artifact inspection, restart, and cancellation tests prove no secret is recoverable. [OPEN.]

### RB-13: AI tool and MCP trust boundaries fail open

**Severity**: P1

**Maturity**: Prototype

A generic MCP bearer token is sent to any activated public MCP endpoint unless a per-server token
exists, with no operator trust decision. Automatically approved project reads and outbound fetches
form a data-exfiltration path for project `.env` files and unpublished results. Malformed tool-call
arguments are repaired to `{}` and remain executable, which can turn a truncated destructive call
into a valid defaults invocation.

Evidence:

- `crates/oxo-flow-ai/src/mcp.rs:177`, `:423`, `:545`
- `crates/oxo-flow-ai/src/tools/builtin.rs:12`, `:140`, `:177`
- `crates/oxo-flow-ai/src/tools/mod.rs:30`
- `crates/oxo-flow-ai/src/agent/orchestrator.rs:160`, `:498`
- `crates/oxo-flow-ai/src/provider.rs:1097`, `:1256`
- `crates/oxo-flow-cli/src/commands/ai_runtime.rs:70`, `:88`

Required exit evidence:

- Generic credentials are sent only to an explicit operator-approved origin.
- Read followed by network egress requires an explicit data-release decision and visible summary.
- Invalid tool arguments are rejected or retried; no executable `{}` call reaches approval.
- Adversarial project-skill, MCP-output, and prompt-injection tests cover each path.

### RB-14: Workflow-scoped AI policy is parsed but not enforced

**Severity**: P1

**Maturity**: Prototype

The CLI selects its provider before resolving workflow-scoped settings, while `auto_fix` has no
runtime consumer outside parsing and tests. Enabled AI can silently degrade after a provider
resolution error. AI template generation can overwrite an existing reviewed workflow
non-atomically.

Evidence:

- `crates/oxo-flow-cli/src/commands/ai_runtime.rs:36`, `:39`, `:143`
- `crates/oxo-flow-ai/src/config.rs:217`, `:238`, `:346`
- `crates/oxo-flow-cli/src/commands/run/failure.rs:257`, `:303`
- `crates/oxo-flow-cli/src/commands/run/dry_run.rs:102`
- `crates/oxo-flow-cli/src/commands/ai_template.rs:107`, `:124`, `:161`, `:574`

Required exit evidence:

- Documented global/project/workflow provider and model precedence is tested end to end.
- `never`, `ask`, and `always` produce distinct write behavior.
- Enabled AI with unusable credentials emits a structured degraded-mode reason.
- Generation refuses an existing destination without confirmation and uses atomic replacement.

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
| WEB-09 | P1 | Quotas are racy and process-local. | Planned / blocked | RB-09. |
| WEB-10 | P1 | OAuth can return a token when session insertion fails; logout can report success when deletion fails. | Planned / blocked | Fail closed or use a durable revoke/retry protocol. Evidence: `crates/oxo-flow-web/src/domains/auth/handlers.rs:760`, `:454`. |
| WEB-11 | P1 | Pipeline edits can silently lose updates; revision insertion is best-effort. | Fixed (#1136) | Baseline re-confirmed on main @ ccf1d4ffa: the update path wrote the row unconditionally (`crates/oxo-flow-web/src/domains/workflow/handlers.rs:802` at that base — the issue's `:775` had drifted) and `record_revision` (`:31`) logged-and-continued on DB failure, so the request reported success without its snapshot. Mechanism: a new monotonic `pipelines.revision` column (default 1) is the concurrency token (distinct from the TOML-derived `version`); it travels as a strong HTTP validator — `ETag: "<revision>"` on GET/create/PUT/rollback responses and back in `If-Match` on PUT/rollback — so the mirrored JSON response contract (`frontend/src/api/types.ts`) is unchanged. The storage layer performs a single-statement compare-and-swap (`UPDATE ... SET revision = revision + 1 WHERE id = ? AND (? IS NULL OR revision = ?)`, rows-affected checked) implemented in both the SQLite and PostgreSQL `StorageBackend` impls (`create_pipeline_with_revision`, `update_pipeline_cas`), so the guard is the atomic write, not a read-then-write. A stale validator writes nothing and answers `409 {code:"PIPELINE_VERSION_CONFLICT", detail:"expected revision N but the stored revision is M", suggestion:"Re-read GET /api/pipelines/{id} for its current ETag and retry with If-Match ..."}`; a malformed validator answers `400 INVALID_IF_MATCH`; omitting the header keeps the historical unconditional update (documented backward compatibility, token still advances). Atomicity statement: create (row + initial snapshot) and update/rollback (CAS row write + pre-write snapshot + 50-row prune) each run in ONE storage transaction per backend, so either both commit or the request fails; failure injection (dropping `pipeline_revisions` on a dedicated in-memory DB, no production test hooks) proves the update/create rolls back and cannot report success. Remaining boundary (documented): the fork/import `save_pipeline` upsert path writes no revision rows (copies start at revision 1 with no history), and the permission/default read before the CAS is outside the transaction — the write itself is guarded by the token, not by that read. Evidence tests: `interleaved_writers_one_conflicts_and_the_winner_survives`, `update_without_if_match_stays_backward_compatible`, `malformed_if_match_is_rejected_not_ignored`, `quoted_and_unquoted_if_match_both_work`, `cas_applies_once_and_records_the_pre_write_snapshot`, `stale_token_is_a_conflict_and_writes_nothing`, `failed_revision_insert_rolls_back_the_pipeline_update`, `failed_initial_revision_insert_rolls_back_the_create` (`crates/oxo-flow-web/tests/pipeline_concurrency.rs`). Docs: `docs/guide/src/reference/web-api.md` → "Optimistic concurrency (`ETag` / `If-Match`)" and Version History transaction semantics. Commit binding: 755259911e93dc68feb6c64daf0761c5e7aa90ba. |
| WEB-12 | P1/P2 | DAG edit history is unowned, process-global, unbounded, and non-durable. | Fixed (#1191) | Commit binding: `7636a2ab67b683fa6f68a8e27795729fe2cca5e1` (PR #1191, issue #1137). Stacks are now keyed by `(CurrentUser.id, pipeline id)` — the same `resolve(Option<Extension<CurrentUser>>)` identity the other pipeline handlers use, with the `default` pseudo-user in personal mode — so two identities editing the same pipeline id own independent stacks. Bounds are documented constants in `crates/oxo-flow-web/src/domains/dag/service.rs`: `MAX_EDIT_HISTORY_DEPTH` = 50 transitions per stack (oldest dropped) and `MAX_EDIT_HISTORY_PIPELINES` = 256 stacks process-wide (LRU eviction on a monotonic use clock). Restart semantics are fail-closed and documented, not persisted: history remains in RAM, undo/redo return a typed `HistoryOutcome`, and the HTTP layer answers `409 EDIT_HISTORY_UNAVAILABLE` (no stack: never edited, restart, or LRU loss) or `409 EDIT_HISTORY_STALE` (supplied TOML is not the history tip) with remediation suggestions; `404 NO_UNDO`/`NO_REDO` remain only for a genuinely exhausted direction. Tests: `domains::dag::service::tests::{edit_history_is_scoped_by_owner, edit_history_depth_cap_drops_the_oldest_transition, edit_history_lru_evicts_the_oldest_pipeline, undo_and_redo_without_history_are_unavailable, exhausted_undo_is_distinguished_from_unavailable_history}`; `domains::dag::handlers::tests::{undo_is_scoped_to_the_acting_user, undo_after_restart_fails_closed_with_structured_code, redo_after_restart_fails_closed_with_structured_code, stale_undo_fails_closed_without_consuming_history, undo_with_no_remaining_transitions_keeps_no_undo}`. Docs: `docs/guide/src/reference/web-api.md` (DAG editing scope/caps/lifetime/codes), `docs/guide/src/reference/dag-edit-api.md` ("History scope, caps, and lifetime" + failure-code table), `docs/guide/src/reference/web-system-architecture.md` (error-code table). |
| WEB-13 | P2 | Template visibility is ambiguous across tenants. | Fixed (#1193) | Commit binding: `ce0d3026f229c26524d7b31921ee5c47d45010dc` (PR #1193, issue #1138). One exported predicate — `can_read_pipeline` plus its write sibling `can_write_pipeline` in `crates/oxo-flow-web/src/domains/workflow/visibility.rs`, re-exported from the workflow domain root — now encodes the policy every path evaluates: an owner sees their own pipeline; `workspace`-visibility (shared-library/template) pipelines are readable by every signed-in user; another tenant's `private`/`link` pipeline is invisible (`link` is a share-token capability, not a read grant); the pre-existing admin cross-tenant override is documented; unknown visibility values fail closed; deletion is a hard delete in both backends, so a deleted pipeline has no row, is invisible to everyone (admins included), and every read answers 404. List/read convention: `GET /api/pipelines` filters its candidate window and `POST /api/pipelines/search` filters returned rows with the same predicate, while `GET`/`PUT`/`DELETE /api/pipelines/{id}`, both revision routes, rollback, fork, share, saved-id `diff`/`export`, `POST /api/runs` with `pipeline_id`, and `POST /api/ai/optimize` with `pipeline_id` answer `404 NOT_FOUND` (never 403) when it returns false. Storage `list_pipelines_visible`/`search_pipelines` are documented candidate windows (SQL scoping before `LIMIT`, audit #664), not the policy source. Documented exemptions: share landing/import (the token is the capability), the global `/api/templates` library (no per-tenant visibility column; writes owner/admin-gated), and the TOML-only DAG edit endpoints. Evidence tests (real team-mode router with Bearer identities): `own_pipeline_is_visible_in_list_and_read`, `other_tenants_private_pipeline_is_absent_from_list_and_404_on_read`, `workspace_shared_pipeline_is_visible_to_other_tenants_read_only`, `deleted_pipeline_is_invisible_in_list_and_read`, `foreign_pipeline_update_rollback_and_delete_are_refused_404` (also fork/share/run-create; asserts the row is intact), `admin_override_sees_foreign_private_pipeline`, `share_token_is_the_authorization_exemption` (`crates/oxo-flow-web/tests/pipeline_visibility.rs`); predicate unit matrix `domains::workflow::visibility::tests::{owner_sees_own_pipeline_in_every_visibility, other_tenants_private_pipeline_is_invisible, other_tenants_link_pipeline_is_invisible, workspace_shared_pipeline_is_visible_to_any_signed_in_user, unknown_visibility_fails_closed, admin_sees_every_tenant, only_owner_or_admin_may_write}`. Docs: `docs/guide/src/reference/web-api.md` → "Visibility policy (WEB-13 / issue #1138)"; `docs/guide/src/how-to/collaboration.md` permissions table aligned with the enforced policy. |
| WEB-14 | P2 | Browser downloads cannot attach the Bearer session header. | Planned / blocked | Use authenticated fetch or short-lived download tickets. Evidence: `frontend/src/api/client.ts:277`, `frontend/src/pages/MonitorReport.tsx:837`. |
| WEB-15 | P2 | AI provider metadata, chat persistence, and audit writes overstate privacy or durability. | Planned / blocked | Role-gate metadata, define best-effort behavior, and fail closed for required audit events. Evidence: `crates/oxo-flow-web/src/domains/ai/handlers.rs:650`, `crates/oxo-flow-web/src/audit.rs:52`. |
| WEB-16 | P2 | OAuth redirect and state are not fully bound server-side. | Planned / blocked | Bind state to provider and redirect URI; enforce a server allowlist. Evidence: `crates/oxo-flow-web/src/domains/auth/service.rs:215`. |
| WEB-17 | P2 | PostgreSQL run lifecycle code exists, but live production parity is unproven and the middleware/error naming still says SQLite. | Implemented but unproven | Rename the capability boundary after evidence; run contract and crash tests on both backends. Evidence: `crates/oxo-flow-web/src/server.rs:498`, `crates/oxo-flow-web/src/infra/db/mod.rs:75`. |
| WEB-18 | P2 | Integration tests disable the run limiter through a debug-build seam rather than testing the production budget composition. | Fixed (#1192) | The limiter is now an instance (`RunRateLimiter`, `crates/oxo-flow-web/src/domains/execution/run_limit.rs`) with an injectable `Clock`: `server::build_router` installs the process-global production instance (`run_limit::global()`) as a request extension, and every spawn handler (`create_run`, `retry_run`, `resume_checkpoint`) resolves it from extensions and fails closed with a structured `500 INTERNAL_ERROR` when the wiring is missing. The debug-only `RunRateLimiter::for_tests(max, clock)` + `server::build_router_with_run_limiter(mode, limiter)` entry points feed the router-level test `enabled_run_limiter_is_composed_through_the_router` (`crates/oxo-flow-web/tests/run_limiter_composition.rs`), which drives the real team-mode router with real auth identities and an enabled limiter (injected budget 2/min, env budget set to 100 to prove the instance wins): two allowed attempts, then `429 RUN_RATE_LIMITED` with the countdown in `message` and no `Retry-After` header, a second identity keying its own budget, and `TestClock` advance past 60 s refilling the window. Negative controls verified locally: removing the extension layer fails the test (`500 Server misconfiguration: run rate limiter unavailable`); removing the `check_spawn_budget` call fails it (no 429). Unit tests: `run_limit::tests::{fixed_budget_allows_up_to_limit_then_blocks_with_wait, budgets_are_keyed_per_identity, advancing_the_clock_refills_the_window, zero_budget_disables_the_limiter}`. The `#[cfg(debug_assertions)]` `set_run_rate_limit_for_tests` seam stays because the existing multi-run integration suites still use it to zero the process-wide window; its docs point at the router-level test for the real composition. Docs: `docs/guide/src/reference/web-api.md` (Create Run rate-limit contract). Commit binding: `97a8dd8ef4b00a999b58e8bf6a540b32a2a15ce5`; PR #1192. |

### 5.4 Frontend, Editor, Desktop, and UX

| ID | Severity | Finding | Current maturity | Evidence and remediation |
| :--- | :---: | :--- | :--- | :--- |
| UX-01 | P1 | Guided editor round-trip silently discards advanced TOML semantics. | Planned / blocked | The card serializer emits only workflow name/version plus basic rule fields. Evidence: `frontend/src/components/GuidedRuleBuilder.tsx:47`, `:73`. |
| UX-02 | P1 | Browser-wide drafts and chat state survive logout and can leak between accounts. | Planned / blocked | `oxo_session` is not identity-scoped and logout removes only `oxo_token`. Evidence: `frontend/src/context/PipelineSession.tsx:59`, `frontend/src/components/Layout.tsx:373`. |
| UX-03 | P1 | Validation-request failure can leave a stale successful validation authorizing execution. | Planned / blocked | The save path catches validation failure and keeps the prior result. Evidence: `frontend/src/pages/PipelineEditor.tsx:716`. |
| UX-04 | P1 | "Apply & Retry" does not apply the displayed repair command. | Planned / blocked | The action calls `retryRun` directly and ignores `fix_action`. Evidence: `frontend/src/pages/MonitorReport.tsx:953`. |
| UX-05 | P1 | AI chat replaces editor content before explicit acceptance. | Planned / blocked | `onPipelineReady` writes the incoming TOML directly into editor state. Evidence: `frontend/src/pages/PipelineEditor.tsx:923`. |
| UX-06 | P1 | Metro grouped editing is not semantics-preserving; stations lack full keyboard access. | Planned / blocked | Process grouping aggregates by environment and groups arbitrary rule sets; station interactions are mouse-oriented. Evidence: `frontend/src/components/MetroCanvas.tsx:147`, `:316`. |
| UX-07 | P2 | Monitor process grouping collapses all environments to `system`. | Planned / blocked | DAG-status canvas data hard-codes `environment: 'system'`. Evidence: `frontend/src/pages/MonitorReport.tsx:1021`. |
| UX-08 | P2 | VS Code writes AI output before validation and diagnoses disk content while unsaved buffers are shown. | Planned / blocked | AI output is written before validation, and validation analyzes the saved path with only a warning about unsaved changes. Evidence: `editors/vscode/src/extension.ts:559`, `:360`; `editors/vscode/src/providers/diagnostics.ts:71`. |
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

### 5.6 CLI, AI, Provider, and MCP

| ID | Severity | Finding | Current maturity | Evidence and remediation |
| :--- | :---: | :--- | :--- | :--- |
| AI-01 | P0 | A model-approved partial TOML block can replace the whole workflow. | Fixed (#1117) | `--ai-recover` now rejects a patch that shrinks the original's rules/includes/references (checked through `WorkflowConfig::parse`) and writes accepted fixes atomically. See RB-11. |
| AI-02 | P1 | Rule shells inherit ambient credentials. | Planned / blocked | RB-15. |
| AI-03 | P1 | Workflow-scoped AI provider and model settings are resolved but ignored. | Planned / blocked | RB-14. |
| AI-04 | P1 | A generic MCP token is sent to any activated public skill endpoint. | Planned / blocked | RB-13. |
| AI-05 | P1 | Auto-approved project reads plus outbound fetch form an exfiltration chain. | Planned / blocked | RB-13. |
| AI-06 | P1 | CLI and web admission evaluate different graphs when includes or overrides change the workflow. | Implemented but unproven | Direct TOML BioTypes now share the core gate; include/override admission parity is still absent. RB-04 defines the exit seam. |
| AI-07 | P2 | Malformed tool arguments are converted to `{}` and remain executable. | Implemented and verified for model-emitted arguments | Provider parsers pass model argument strings through raw (never repaired to `{}`), the orchestrator's JSON-validity gate rejects them with a fail-closed tool-result message before the registry, and `ToolRegistry::execute` re-checks. Wire-format echoes (Anthropic `tool_use.input`, Ollama `arguments`) keep a protocol-valid `{}` fallback but are never executed. MCP-side argument handling is unchanged. |
| AI-08 | P2 | `[ai] auto_fix` is documented but has no runtime consumer. | Planned / blocked | RB-14; user docs now warn that it is not a safety control. |
| AI-09 | P2 | Enabled AI can silently become unavailable during analysis and recovery. | Planned / blocked | RB-14. |
| AI-10 | P2 | AI template generation overwrites existing destinations non-atomically. | Planned / blocked | RB-14. |
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
4. Tests/benches/examples wave: proof that fixtures exercise real contracts and do not encode stale semantics.
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
