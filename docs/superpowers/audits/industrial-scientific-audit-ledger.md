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

**Maturity**: Prototype; blocks scientific-validity claims

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

**Maturity**: Prototype

A failed result pull is logged while the remote exit code is still used to finalize the run. Archive extraction trusts the remote tar stream, and cancellation relies on an in-memory registry and wrapper-only process targeting.

Evidence:

- `crates/oxo-flow-web/src/domains/clusters/remote.rs:132`
- `crates/oxo-flow-web/src/domains/clusters/remote.rs:152`
- `crates/oxo-flow-web/src/domains/clusters/remote.rs:389`
- `crates/oxo-flow-web/src/domains/clusters/remote.rs:392`
- `crates/oxo-flow-web/src/domains/clusters/remote.rs:27`

Required exit evidence:

- Pull and extraction integrity checks must pass before a run can become `completed`.
- Malicious path traversal in an archive is rejected.
- Restart-safe cancellation terminates the remote process tree or reports the exact unresolved jobs.

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

**Maturity**: Prototype

Cancel writes a terminal row before signaling and can overwrite a racing completion. Pause sends `SIGSTOP` before a fallible database write. Resume can write `running` when no process group exists.

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

**Maturity**: Prototype

The destination is truncated before multipart consumption succeeds. Oversize or read errors remove the destination, and write errors can leave a partial replacement.

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

**Maturity**: Prototype

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

**Maturity**: Prototype

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
  GCP artifacts or child argv.
- Each backend obtains the value through a restricted secret-injection path.
- Artifact inspection, restart, and cancellation tests prove no secret is recoverable.

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

- `crates/oxo-flow-core/src/executor/process.rs:3223`

Required exit evidence:

- A rule with no declared environment cannot observe sentinel ambient secrets.
- Explicit declarations expose only the named variables.
- Local, container, HPC, and cloud execution paths share the same tested allowlist policy.

---

## 5. Detailed Findings

### 5.1 Workflow, Validation, and Engine

| ID | Severity | Finding | Current maturity | Evidence and remediation |
| :--- | :---: | :--- | :--- | :--- |
| WF-01 | P1 | Load-path parsing bypassed `extends`, so inherited rules were absent. | Implemented but unproven | Multi-level inheritance tests exist; exact commit-bound evidence still needs to be attached. |
| WF-02 | P1 | Empty scatter domains and zero/invalid `transform.split.n` could produce a misleading successful plan. | Implemented but unproven | Fail-closed behavior and regression tests exist; exact commit-bound evidence still needs to be attached. |
| WF-03 | P1 | Transform-generated rules lost environment, hooks, retries, staging, tags, benchmarks, and related semantics. | Implemented but unproven | Preserved semantics and tests exist; broad corpus evidence remains open. |
| WF-04 | P1 | BioType validation can still false-accept undeclared producers, missing required builds, and declared attributes. | Planned / blocked | The checker defect is RB-04A; resolved web include/override admission remains RB-04. |
| WF-05 | P2 | Conda-family environment handling had duplicated and divergent paths. | Implemented but unproven | Consolidation landed; cross-backend lifecycle evidence remains incomplete. |
| WF-06 | P1 | `job.sh` can still contain expanded secrets. | Planned / blocked | Secret-store or minimal-environment routing is required before team/HPC use. |

### 5.2 Cluster, Remote, and Scheduler

| ID | Severity | Finding | Current maturity | Evidence and remediation |
| :--- | :---: | :--- | :--- | :--- |
| CL-01 | P1 | Post-submit failures could orphan accepted jobs before local job registration. | Implemented but unproven | Best-effort cancel covers the registration window; success must not be claimed when cleanup itself fails. |
| CL-02 | P1 | Cluster artifacts and commands could expose sensitive values. | Implemented but unproven | Artifact permissions tightened and command logging masked; scheduler/container secret artifacts remain RB-12. |
| CL-03 | P1 | Cancellation and per-element accounting remain unreliable. | Planned / blocked | See RB-06. |
| CL-04 | P1 | Remote pull and extraction can claim success with missing artifacts. | Planned / blocked | See RB-05. |
| CL-05 | P2 | Cloud backend tests are mostly mock and CLI-wrapper tests. | Prototype | Live cloud campaigns are required before cloud-native claims. |

### 5.3 Web, API, Auth, and Storage

| ID | Severity | Finding | Current maturity | Evidence and remediation |
| :--- | :---: | :--- | :--- | :--- |
| WEB-01 | P0 | No OS-level tenant execution boundary. | Planned / blocked | RB-01. |
| WEB-02 | P0 | No immutable per-run artifact snapshot. | Planned / blocked | RB-02. |
| WEB-03 | P1 | Non-atomic create/stage/persist/spawn. | Planned / blocked | RB-03. |
| WEB-04 | P1 | Lifecycle transitions can contradict process state. | Planned / blocked | RB-07. |
| WEB-05 | P1 | Failed re-upload can destroy the previous valid file. | Planned / blocked | RB-08. |
| WEB-06 | P1 | Remote pull failure does not prevent `completed`. | Planned / blocked | RB-05. |
| WEB-07 | P1 | Scheduler cancellation and accounting collapse outcomes. | Planned / blocked | RB-06. |
| WEB-08 | P1 | Web admission still depends on a permissive BioType checker and lacks resolved include/override parity. | Planned / blocked | Direct TOML routes through the core checker, but the checker false-accepts (RB-04A) and resolved admission remains RB-04. |
| WEB-09 | P1 | Quotas are racy and process-local. | Planned / blocked | RB-09. |
| WEB-10 | P1 | OAuth can return a token when session insertion fails; logout can report success when deletion fails. | Planned / blocked | Fail closed or use a durable revoke/retry protocol. Evidence: `crates/oxo-flow-web/src/domains/auth/handlers.rs:760`, `:454`. |
| WEB-11 | P1 | Pipeline edits can silently lose updates; revision insertion is best-effort. | Planned / blocked | Add optimistic concurrency and transactional revisions. Evidence: `crates/oxo-flow-web/src/domains/workflow/handlers.rs:775`, `:853`. |
| WEB-12 | P1/P2 | DAG edit history is unowned, process-global, unbounded, and non-durable. | Planned / blocked | Scope by tenant, bound memory, and define restart semantics. Evidence: `crates/oxo-flow-web/src/domains/dag/service.rs:36`, `:466`. |
| WEB-13 | P2 | Template visibility is ambiguous across tenants. | Planned / blocked | Define and test tenant versus shared visibility. Evidence: `crates/oxo-flow-web/src/domains/workflow/handlers.rs:962`, `:1007`. |
| WEB-14 | P2 | Browser downloads cannot attach the Bearer session header. | Planned / blocked | Use authenticated fetch or short-lived download tickets. Evidence: `frontend/src/api/client.ts:277`, `frontend/src/pages/MonitorReport.tsx:837`. |
| WEB-15 | P2 | AI provider metadata, chat persistence, and audit writes overstate privacy or durability. | Planned / blocked | Role-gate metadata, define best-effort behavior, and fail closed for required audit events. Evidence: `crates/oxo-flow-web/src/domains/ai/handlers.rs:650`, `crates/oxo-flow-web/src/audit.rs:52`. |
| WEB-16 | P2 | OAuth redirect and state are not fully bound server-side. | Planned / blocked | Bind state to provider and redirect URI; enforce a server allowlist. Evidence: `crates/oxo-flow-web/src/domains/auth/service.rs:215`. |
| WEB-17 | P2 | PostgreSQL run lifecycle code exists, but live production parity is unproven and the middleware/error naming still says SQLite. | Implemented but unproven | Rename the capability boundary after evidence; run contract and crash tests on both backends. Evidence: `crates/oxo-flow-web/src/server.rs:498`, `crates/oxo-flow-web/src/infra/db/mod.rs:75`. |
| WEB-18 | P2 | Integration tests disable the run limiter through a debug-build seam rather than testing the production budget composition. | Implemented but unproven | Keep the seam hidden from release builds, but add a router-level test with a test-scoped clock/store that exercises enabled rate limiting without process-global overrides. Evidence: `crates/oxo-flow-web/src/domains/execution/handlers.rs:259`. |

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
| EV-01 | P1 | Detection, attribution, repair proposal, repair application, verification, and successful rerun were previously conflated. | Implemented and verified for tooling separation | Tooling now reports the stages separately; the actual simulation currently shows 0 applied/verified repairs and 0 reruns. |
| EV-02 | P1 | Statistical reports could present synthetic data as inferential evidence. | Implemented and verified for fail-closed tooling | Missing matched three-arm data now fails closed; synthetic mode is explicitly non-inferential. |
| EV-03 | P1 | Hardware projections were presented beside host measurements. | Implemented and verified for labeling | Model output now says `MODEL PROJECTION` and `NOT MEASUREMENT`. No measured hardware study exists. |
| EV-04 | P1 | Cohort DAG counts were not derived from the real plan. | Implemented and verified for fail-closed parsing | Real `dry-run --json` plans are required; the current pilot is `N=10`, 30 nodes. |
| EV-05 | P1 | The implementation checklist marked unverified cloud, hardware, UX, and manuscript work complete. | Fixed in this audit wave | Replaced with maturity-labeled, unchecked gates. |
| EV-06 | P1 | Synthetic superiority and human-factors claims appeared in reports. | Fixed in this audit wave | Reports are now evidence-status documents, not result claims. |

### 5.6 CLI, AI, Provider, and MCP

| ID | Severity | Finding | Current maturity | Evidence and remediation |
| :--- | :---: | :--- | :--- | :--- |
| AI-01 | P0 | A model-approved partial TOML block can replace the whole workflow. | Planned / blocked | RB-11. |
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
