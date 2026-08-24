# Cloud Storage Staging, Pairs Re-entry, Toolchain Upgrade — Design for issue #80 + selected #67 items

Design for [issue #80](https://github.com/Traitome/oxo-flow/issues/80) (the three
deliberate leftovers of #78) plus selected implementable items of the
[#67 checklist](https://github.com/Traitome/oxo-flow/issues/67).
Status: **approved-by-goal** (2026-08-14, autonomous session; the goal authorizes
design → implement → verify → document end to end).

## 1. Context and coordination

- **#79** is tracked by a peer session with **uncommitted web-crate changes** in the
  working tree (`web/src/{db.rs,domains/mod.rs,infra/db/*,lib.rs,main.rs,server.rs}`,
  new `config.rs`, `domains/clusters/`). This session commits **only its own paths**
  (`git add <path>…`); web edits here are limited to files the peer session has not
  touched (`domains/execution/service.rs`).
- **#74** (cluster execution as a `run` backend) is owned by @andrewbudge — `run
  --profile` wiring, job arrays, re-attach, real-cluster validation stay out of scope.
  `ExecutorBackend::logs` already exists (landed in #78 P1); wiring the `cluster logs`
  CLI stub onto it (§6.3) is #67 item 4's decision and does not overlap #74.
- Everything lands on `main` in phase commits, each with `make ci` green
  (fmt + clippy -D warnings + build + test + audit), per the repo convention.

## 2. Toolchain upgrade + S3 adapter (#80 item 1)

**Fact check (measured 2026-08-14):** the pinned rustc 1.92.0 cannot compile
`aws-sdk-s3` — the locked `aws-sdk-s3 1.141.0` declares `rust-version = "1.94.1"`.
`gcs-storage` has no such problem (lightweight deps). The `s3-storage` feature has
therefore never compiled in CI or locally (pre-existing condition, acknowledged in
`cloud-storage.md`).

**Decision:**

- Bump `rust-toolchain.toml` from `1.92.0` to `1.97.1` — the current stable release
  (rustc 1.97.1, 2026-07-14), available both on the rustup distribution CI uses
  (`dtolnay/rust-toolchain`) and as a conda-forge package for local builds. The repo
  convention ("bump deliberately — run `make ci` locally on the new version first")
  is followed: the full gate must go green on 1.97.1 before the S3 work lands.
  Clippy lints added between 1.92 and 1.97 are fixed in the same phase (the PR #59
  gate exists precisely to absorb this class of breakage).
- After the gate is green: `cargo test -p oxo-flow-core --features s3-storage,gcs-storage`
  must compile and pass (S3 `head()` from #78 P2 included).
- CLI feature forwarding: `oxo-flow-cli` gains `s3-storage` / `gcs-storage` features
  (default off — aws-sdk build cost stays opt-in, same as today) forwarded to
  `oxo-flow-core`. `run_preview::storage_resolver()` registers `S3Storage`/`GcsStorage`
  behind the matching `#[cfg(feature = …)]` — the placeholder doc comment at
  `run_preview.rs:70-77` is fulfilled. Because `run.rs` and `run_preview.rs` share
  this one function, **run and dry-run cannot drift** (the #77 principle).
- Local toolchain: the machine's base conda env refuses the rust upgrade (solver
  conflicts with conda's own pinned python deps — measured). A dedicated
  `oxo-toolchain` conda env (rust 1.97.1, conda-forge) is used via
  `conda run -n oxo-toolchain …` for local verification; the base env is untouched.

## 3. Remote staging / upload (#80 item 2)

### 3.1 Where it lives

`LocalExecutor::execute_rule_with_config` (`executor/process.rs:653`) only. The
cluster path (`BackendDriver`) keeps today's behaviour: submitted scripts run on
nodes whose shared storage the workflow author controls — staging is a local-executor
concern for now (documented; #74 territory).

### 3.2 Core mechanism: pattern substitution on a copied Rule

The executor already renders `{input[n]}` / `{output[n]}` / `{input}` / `{output}`
by textual replacement of the raw patterns (`render_shell_command`,
`process.rs:1137-1181`), and every gate (`should_skip_rule`, `optional_inputs_missing`,
`validate_outputs`, auto-mkdir in `build_execution_command`, `validate_path_safety`)
joins patterns against the workdir. Instead of threading a staging map through all
of those signatures, the engine builds a **substituted copy of the Rule** (immutable
pattern — the original `Rule` in `config.rules` is never mutated) in which every
remote input/output pattern is replaced by its staged local path:

```
stage_remote_io(rule, workdir, wildcard_values, resolver)
  -> Result<Option<StagedRule { rule: Rule, uploads: Vec<UploadJob{remote, local}> }>>
```

- Each input/output pattern is rendered with `wildcard_values` (config vars + engine
  wildcards) and parsed with `StoragePath::parse`.
- Local patterns are left untouched; `None` is returned when nothing is remote
  (fast path — zero behaviour change for purely-local workflows).
- Remote **globs/dirs are rejected** with a clear error (remote references must be
  exact object URIs — the same boundary the #78 P2 manifest snapshot already
  documents).
- Staged paths are **workdir-relative**: inputs land under
  `.oxo-flow/staged/in/<scheme>/<bucket>/<key>`, outputs under
  `.oxo-flow/staged/out/<scheme>/<bucket>/<key>` — so every existing
  `workdir.join(pattern)` gate keeps working unchanged on the copy.

### 3.3 Staging cache and atomicity

New shared helper `storage::stage_with_cache(backend, path, workdir, dest)` used by
both the S3 and GCS `stage()` impls (DRY):

1. `head()` the object; compare against the sidecar `dest.meta.json`
   (`{size, etag}`). Match → cache hit, return `dest` (its mtime stays old —
   important, see 3.5).
2. Miss → download to `dest.part`, then atomic `rename` to `dest`, then write the
   meta. Any error deletes `dest.part` and leaves the previous cached object (and
   its meta) untouched — **rollback is "never lose a valid cache entry"**.
3. `head()` failure propagates: for non-optional inputs the rule fails with a clear
   error before execution; for `optional` rules a missing remote object counts as
   "input missing" → skip (same contract as local optional inputs).

Cache invalidation is **etag-driven, never mtime-driven**: the run-level manifest
snapshot (`checkpoint.rs:snapshot_input_manifest`) keeps recording the *remote*
`(scheme, key, size, etag)` from the **original** rule patterns — substitution is
executor-internal and invisible to checkpoints. Deleting `.oxo-flow/staged/` merely
forces re-download (possibly a redundant re-execution, never a wrong result).

### 3.4 Upload

After execution succeeds and `validate_outputs` passes (it validates the substituted
local paths), each `UploadJob` copies the local staged output to its remote URI via
`StorageBackend::upload`. Upload failure **fails the rule** with a clear error — a
declared remote output that did not land is a broken contract, same as a missing
local output. S3 `PutObject` is atomic per object; no multi-object transaction is
attempted or promised.

### 3.5 Freshness semantics (why the order works)

Staging runs **after** the `when` condition and **before** `build_execution_command`
and the freshness gate (a small reorder: the `when` block moves above the command
build). Consequences, verified by the test matrix in §8:

- **etag unchanged, outputs exist** → cache hit keeps the old staged mtime →
  `should_skip_rule` skips (rule truly up-to-date).
- **etag changed** → run-level invalidation puts the rule in `to_run` AND the
  re-download refreshes the staged mtime → the executor gate agrees: run.
- **`--force`/`--force-rules`** → existing force flags bypass the gate, unchanged.
- **Remote outputs**: `should_skip_rule` compares the local staged-out file against
  staged-in files — the upload is authoritative for existence (3.4). If the remote
  object was deleted behind the engine's back while the rule is in `to_run`, the
  gate's head() check (see below) forces a re-run that re-uploads.
- `should_skip_rule`/`optional_inputs_missing` are called with the **substituted**
  rule, so no signature changes anywhere.

Remote-output existence at gate time: when *all* outputs of a rule are remote and
the local staged-out file exists, one `head()` per remote output confirms the object
still exists remotely before skipping (cheap; prevents "cache says done, cloud
says gone").

### 3.6 Rendering corrections

- `build_execution_command`'s auto-mkdir (`process.rs:1079-1096`) runs on the
  substituted rule → it creates the `.oxo-flow/staged/out/…` parents, and can no
  longer create literal `s3:` directories.
- `warn_if_remote_paths` stops being a stub for the local executor: with a backend
  registered, staging is logged at debug level (the warning only fires when a
  **raw URI literal** appears in the shell text, or when no backend is registered —
  the existing degrade-gracefully warning).
- Shells that reference remote data must use `{input[n]}` / `{output[n]}` —
  the substituted patterns are what those placeholders render. Raw URIs typed
  directly into the shell text are the workflow author's responsibility
  (documented).

### 3.7 dry-run and cluster paths

`dry_run_rules` stays read-only: no staging, no downloads (documented). The
cluster driver path is unchanged (§3.1).

## 4. [[pairs]]-driven checkpoint re-entry (#80 item 3)

### 4.1 Manifest surface (additive)

```toml
[reentry]
group = "batch"            # existing sample semantics, unchanged
sample = ["S4"]            # existing, unchanged
pairs = [                  # NEW: optional
  { pair_id = "CASE_007", experiment = "T7", control = "N7", experiment_type = "tumor" },
]
```

- `pairs` entries mirror `ExperimentControlPair` (`config.rs:515-546`):
  `pair_id`, `experiment` (alias `tumor`), `control` (alias `normal`),
  `experiment_type` (alias `tumor_type`), optional `metadata`.
- A manifest may announce samples and pairs in one round (superset — the engine
  processes both). An empty manifest stays a valid no-op; a missing/unparsable
  manifest still fails the checkpoint rule (existing contract E013).
- Parse errors in a pair entry (missing `pair_id`/`experiment`) fail the rule with
  the offending entry named.

### 4.2 Merge semantics — identity is `pair_id`

Pairs are a flat list (`WorkflowConfig.pairs`), not groups, so there is no group
resolution: new values are **appended to `config.pairs`**:

- New `pair_id` → append; re-expansion creates `{rule}_{pair_id}` instances.
- Existing `pair_id` with identical content → no-op (dedup).
- Existing `pair_id` with **different** content → error E015 (conflicting pair
  re-entry) — silent supersede would corrupt already-run pair outputs.
- **Same sample in multiple pairs is not an ambiguity**: pair instances are keyed by
  `pair_id` (`config.rs:2023-2027`), so one sample in two pairs is two distinct
  instances by construction — documented, not special-cased.

### 4.3 Record, replay, revoke

- `ReentryRecord` gains `#[serde(default)] pairs: Vec<ExperimentControlPair>`
  (backward compatible — legacy checkpoints load with an empty list).
  `record_reentry` supersedes per rule exactly as today.
- `apply_reentry` merges samples *and* pairs, then the existing
  `reexpand_from_templates` runs — **one** expansion covers both kinds, because
  `expand_wildcards` already rebuilds pair instances from `config.pairs`
  (`config.rs:1944`). `expansion_samples` attribution (#63) is updated by the same
  expansion.
- `replay_valid_reentries` replays pair records under the same validity rule
  (checkpoint rule up-to-date); revocation drops both samples and pairs of an
  invalidated record — the plan shrinks back deterministically.
- Round cap `MAX_REENTRY_ROUNDS = 32` applies to pair rounds identically.

### 4.4 Validation and collisions

- E014 (checkpoint rule must not be parameterized) is extended: the rule must not
  use pair wildcards (`{pair_id}`, `{experiment}`/`{tumor}`,
  `{control}`/`{normal}`, `{experiment_type}`/`{tumor_type}`, or pair `metadata`
  keys) — bounded re-entry, same rationale as `{sample}`/`{group}`.
- **Name-collision guard**: pair instance names `{rule}_{pair_id}` and group
  instance names `{rule}_{group}_{sample}` share a namespace; a runtime-discovered
  `pair_id` containing `_` could collide. `apply_reentry` detects duplicate
  instance names after re-expansion and errors (E016) instead of silently
  overwriting — the discovery rule wrote something the workflow author must fix.

### 4.5 run / resume / dry-run

The existing single hook (`run.rs` `process_reentry`) handles both kinds — the
manifest parser returns samples+pairs and `apply_reentry` does the rest, so the
event loop, resume replay, and dry-run preview (which reconstructs from
`reentries`) all gain pairs support with **no additional control-flow edits**.

## 5. #67 selected items (implementable without cluster hardware)

### 5.1 `resource_bottlenecks` real metering (#67 §4)

- **Core measurement**: in `LocalExecutor`, the spawned rule process already runs
  under its own process group (kill-process-tree semantics). A sampler task polls
  `ps -o rss= -g <pgid>` at 200 ms intervals while the child runs; the summed peak
  (KiB → MB, rounded up) is recorded. This is portable across macOS and Linux
  (the two supported dev platforms). Sampling caveat is documented: the metric is
  a sampled peak, not an exact `getrusage` max — sufficient for bottleneck
  detection, which is about sustained pressure.
- `JobRecord` gains `#[serde(default)] max_rss_mb: Option<u64>` (additive);
  `run.rs` copies it into `BenchmarkRecord.max_memory_mb` (today hardcoded
  `None`). CLI `status --timing` / report display gain real numbers for free.
- **Web diagnostics**: `diagnose_run` (`web/src/domains/execution/service.rs`)
  stops returning `vec![]`: a rule counts as a bottleneck when its measured
  `max_memory_mb` reaches ≥ 80% of its declared memory limit (`threads`/`memory`
  resolved via the existing `effective_*` convention) — metric `"max_memory_mb"`,
  `actual` = measured MB, `limit` = declared MB. The data flows from the
  checkpoint's `benchmarks` (the web run already has a workdir + checkpoint per
  #69). File touched: only `domains/execution/service.rs` (not in the #79 peer's
  modified set).

### 5.2 Webhook signature → real HMAC-SHA256 (#67 §4)

`core/webhook.rs` currently computes **keyed SHA-256** (`sha256=hex(sha256(secret‖body))`)
— documented as such, but the name "HMAC" in docs/webhooks.md is a trap for
consumers who implement RFC-2104 and get a different signature. Change:

- `WebhookConfig` gains `signature_scheme: SignatureScheme` (default
  `HmacSha256`). `HmacSha256` → RFC-2104 HMAC-SHA256, header
  `X-OxoFlow-Signature: hmac-sha256=<hex>`. `Sha256Keyed` → the legacy scheme,
  header `sha256=<hex>`, kept for existing consumers, emits a `warn!` once per
  webhook fire.
- Implementation uses the `hmac` + `sha2` crates (`hmac` is already a dependency,
  currently behind `gcs-storage`); the Cargo change promotes it to a non-optional
  dependency of core's default features.
- Deprecation path: docs state the legacy scheme is frozen and will be removed in
  a future major version; consumers should switch to `hmac-sha256`. No inbound
  verification endpoint exists in-tree, so this is purely outbound.

### 5.3 `cluster logs` — the last CLI stub (#67 §1/§4 decision)

**Decision: implement**, on top of `ExecutorBackend::logs` (already in `core`):
`ClusterAction::Logs` constructs the backend-specific `ClusterExecutor` and prints
its `logs(job_id)` result. SLURM → `sacct --format=JobID,State,ExitCode,Elapsed,MaxRSS`;
PBS/SGE/LSF stay best-effort (already implemented in the backend). Tested against
the mock-scheduler `sacct` shim (already in tree from #78 P1), plus a no-scheduler
error path. This does not overlap #74 (which owns `run --profile` wiring and job
arrays).

### 5.4 Not selected (documented in the #67 reply)

Real-cluster validation, real-tool gallery runs, GPU allocation, AI-quality runs,
and SSE-under-load all need cluster hardware, real bioinformatics tools, or deep
web-crate changes in the #79 peer's territory. They remain open in #67 with the
rationale attached.

## 6. China network mirror verification (#67 §5) — measurement, no code

From this machine (China network): probe TUNA/USTC/Aliyun/Tencent bioconda
channels, rsproxy, and Docker registry mirrors (`/v2/` challenge, where 401 =
reachable/auth-challenge, 403/timeout = blocked or dead), plus pixi
`[mirrors]`/`[pypi-config]` endpoints. Results land in #67 as a findings comment
with per-endpoint status and timestamps. No code changes.

## 7. Testing (TDD per repo rules)

- **Toolchain**: `make ci` green on 1.97.1 (first phase, before any feature work);
  then `cargo test -p oxo-flow-core --features s3-storage,gcs-storage` green.
- **Staging (unit)**: `stage_with_cache` — cache hit/miss, atomic rename on error,
  meta mismatch re-download; `stage_remote_io` — substitution correctness (Vec and
  Map patterns), glob rejection, optional-input skip path, no-remote fast path
  returns `None`.
- **Staging (integration, MinIO — real server, no docker)**: a real S3-compatible
  endpoint via the MinIO binary with `AWS_ENDPOINT_URL`/path-style env. E2E:
  workflow with `s3://` input → run → local execution reads the staged file →
  remote output uploaded → rewrite the cloud object (same size, new etag) →
  re-run invalidates **exactly** that rule (the #80-2 acceptance scenario).
  Gated on the `s3-storage` feature so the default CI suite stays aws-free.
- **Pairs re-entry (unit)**: manifest parsing (pairs incl. aliases/metadata),
  merge dedup/conflict E015, collision guard E016, replay/revoke with pairs,
  record round-trip + legacy checkpoint load.
- **Pairs re-entry (integration)**: discovery rule announces two new pairs → round-2
  pair instances execute → outputs exist; resume reconstructs the same plan and
  re-runs nothing; invalidating the discover rule revokes the pair instances;
  mixed sample+pairs manifest in one round.
- **resource_bottlenecks**: executor test with a memory-eating shell
  (`perl`/`python` chunk allocation) asserts `max_rss_mb` is recorded and
  monotone-plausible (≥ some floor, bounded above); web `diagnose_run` unit test
  with synthetic benchmarks at 50%/80%/120% of limit.
- **Webhook**: RFC-2104 test vectors (published HMAC-SHA256 vectors from RFC 4231
  test cases 1–2) for the new scheme; legacy vector regression test; scheme config
  round-trip.
- **cluster logs**: mock-scheduler sacct shim returns recorded job rows; command
  prints them; unknown backend errors clearly.

## 8. Documentation sync

- `reference/cloud-storage.md`: drop the "S3 needs a toolchain bump" caveat;
  document staging/upload semantics (cache layout, etag-driven cache, `{input[n]}`/
  `{output[n]}` convention, remote glob/dir rejection, dry-run/cluster scope,
  sampled-RSS caveat if placed here); keep GCS divergence note.
- `reference/workflow-format.md`: checkpoint re-entry gains the `pairs` manifest
  surface, identity/dedup/conflict rules, E015/E016 codes, and drops the
  "[[pairs]]-driven re-entry is not supported" line.
- `reference/webhooks.md`: HMAC-SHA256 scheme + deprecation of the keyed scheme.
- `reference/execution-backends.md` (from #78): `cluster logs` no longer a stub.
- Diagnostics API docs (web reference page): `resource_bottlenecks` semantics +
  sampling caveat.
- Error-code table (E013–E016) wherever it lives in the docs.
- `mkdocs build --strict` must pass.
- CHANGELOG: conventional commits per phase (git-cliff generates from history).

## 9. Issue management on completion

- #80: per-item comments with test evidence (toolchain bump commit, MinIO E2E
  transcript, pairs integration test names) and checklist updates.
- #67: findings comments for §4 (bottlenecks metering shipped, webhook HMAC
  shipped, cluster logs decision+implementation) and §5 (mirror probe results);
  explicitly re-scope the rest (cluster hardware / real tools / #74 ownership).
- Memory files updated (`issue-80-*`, `issue-67-*` notes) per session convention.

## 10. Risks

- **Toolchain jump 1.92 → 1.97**: new clippy lints on ~1500 tests — bounded,
  mechanical fixes; the phase is isolated as its own commit.
- **aws-sdk build weight**: CLI features stay opt-in; default build unaffected
  (verified by `cargo check -p oxo-flow-cli` timings in the phase).
- **Peer-session working tree**: all commits use explicit paths; `git status` is
  re-checked before every `git add`.
- **Staging + freshness interplay**: the reorder (stage before gate) is the subtle
  part — the test matrix in §3.5 pins it down; the shared resolver keeps run and
  dry-run on the same code path.
- **MinIO path-style**: aws-sdk-s3 needs `force_path_style` for MinIO; resolved at
  implementation time via SDK config (env var if supported, else a documented
  `OXO_S3_FORCE_PATH_STYLE` env read in `S3Storage`).
