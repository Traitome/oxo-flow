# PR-02 — Inclusion, Exclusion, Stopping, Outlier, and Missing-Data Rules

| Freeze record | Value |
| :--- | :--- |
| Artifact | PR-02 (Task 4.0 required artifact 2 of 6) |
| Version | 1.0.2 |
| Freeze status | FROZEN |
| Freeze date | 2026-11-18 (original freeze; amendment 1 same date; amendment 2 on 2026-11-28) |
| Amendment | 1 (2026-11-18): review fixes D1–D5 and clarifications — see [`AMENDMENTS.md`](AMENDMENTS.md) entry 1. 2 (2026-11-28): #1166 corpus, `n_min`, host, negative control, and missingness limit frozen — see entry 2 |
| Content binding | SHA-256 recorded in [`MANIFEST.sha256`](MANIFEST.sha256) |
| Campaigns covered | #1163, #1164, #1165, #1166 |
| Change control | Post-freeze edits only through [`AMENDMENTS.md`](AMENDMENTS.md) |

**Frozen-before-data statement.** These rules were fixed before
confirmatory data collection for #1163, #1165, and #1166. #1164's ten-case
pilot was already observed before this freeze; its disclosure in PR-01
§C2 governs how that pilot may and may not be used.

## 0. Rules common to all campaigns

### 0.1 Inclusion

A case is included only when all of the following hold:

1. It is listed in the pre-run case manifest, whose SHA-256 is sealed
   before the first confirmatory run (PR-04 §2.2).
2. Its expected outcome or reference result was authored or independently
   reviewed before the run, with a recorded rationale.
3. It exercises a capability the campaign names, and its provenance
   (source, author, commit or upstream URL) is recorded.
4. It is independent: no duplicate content under a renamed file, and no
   two cases that assert the same thing in the same coverage cell. Duplicate
   detection is by content hash plus normalized case body.
5. Its inputs are available and its execution requirements are within the
   frozen hardware and budget manifest (PR-03).

### 0.2 Exclusion

A case is excluded **before** the run only for a documented, outcome-blind
reason: invalid provenance, duplication, a defect in the case itself, or a
missing prerequisite that cannot be supplied without changing the case.
Pre-run exclusions and their replacements (drawn from a sealed reserve
list) are recorded in the pre-run manifest amendment. A case is never
excluded after its outcome is seen, except under the campaign-specific
missing-data rule below, which must be applied mechanically to every case.

### 0.3 Stopping

There is no interim efficacy analysis and no early stopping for a favorable
result. A campaign stops early only for:

1. **Infrastructure invalidation** — the measurement host, container
   digests, or binary cannot satisfy the frozen manifest, so the run would
   not measure the frozen protocol. Report as STOPPED, never as a result.
2. **Leakage breach** — PR-04 detects held-out material in a prompt,
   knowledge base, or training-exposed surface. The affected cases are
   replaced before any outcome is analyzed; after an outcome is seen, the
   campaign is marked exploratory.
3. **Protocol deviation** — a deviation that changes an endpoint. It must
   be recorded as an amendment and the campaign restarts under the amended
   protocol if the deviation is discovered before outcomes; otherwise the
   affected endpoint is exploratory.

Stopping does not relax the publication rule: a stopped campaign reports
that it stopped and why, and makes no capability claim.

### 0.4 Outliers

Binary endpoints have no outlier rule: a case verdict is a verdict.

Continuous endpoints (only #1165's operational metrics and any descriptive
timing) are never trimmed, winsorized, or dropped from the primary
analysis. Values outside `Q1 − 3×IQR` or `Q3 + 3×IQR` are flagged and
retained; a pre-declared sensitivity analysis recomputes the summary
without the flagged values. If the flagged and unflagged summaries
disagree, both are reported and the primary (all-values) summary governs.

### 0.5 Missing data

Missingness is classified, counted, and reported in a flow table. No case
is silently dropped. Every campaign also runs the worst-case sensitivity
analysis named in its section, so a missing-data assumption cannot rescue
a result.

## C1 — #1163 truth-set rules

**Inclusion.**

- Case is a typed or untyped DAG in `.oxoflow` form, or a transplant that
  has been converted to `.oxoflow`, with an expected verdict (accept or
  reject) and, for reject cases, an expected `TYPE-*` code and a written
  rationale. A case that should trigger an advisory `TYPE-008`/`TYPE-009`
  finding is recorded as expected-accept with an `expected_warning_code`;
  it is not a reject case (see PR-01 §C1).
- Expected verdicts are authored independently of the checker: the author
  must not be the author of the checker change under test, and must record
  that the verdict was derived from the schema, workflow semantics, or the
  upstream pipeline, not from running the checker.
- Coverage must include every blocking code (`TYPE-001`–`TYPE-007`) that
  the frozen case set can exercise, with reject cases and accept-side
  contrasts where such a contrast exists; every advisory code
  (`TYPE-008`/`TYPE-009`) needs an expected-accept case declaring the
  expected warning; and untyped workflows that should remain acceptable
  (TA/FR cells) must be present, because a checker that rejects
  everything is not validated by reject cases alone.

**Exclusion.**

- Cases whose expected verdict depends on a construct outside the schema
  version under test.
- Cases that duplicate another case's contract assertion.
- Cases whose rationale cannot be stated without reference to the checker
  output.

**Concrete numbers.** The truth-set size is **UNDECIDED** (owner:
campaign lead; deadline: before data collection) and must be fixed in the
pre-run manifest. Minimum coverage is fixed by the rule above, not by a
count: every exercised `TYPE-*` code needs both a reject case and an
accept-side contrast where such a contrast exists. The number of cases per
code is **UNDECIDED** in the same amendment.

**Missing data.**

- A case for which the binary cannot produce a verdict (crash, timeout,
  toolchain failure) is a **non-verdict**, excluded from the primary
  denominator with its reason. If non-verdicts exceed **UNDECIDED**
  percent (owner: campaign lead; deadline: before data collection) the
  campaign is INVALID, not negative.
- Worst-case sensitivity: all non-verdicts are counted as false accepts.
  If that changes the PR-06 decision, the result is reported as
  threshold-fragile.

**Stopping / outlier.** No interim analysis; no continuous endpoint, so the
common outlier rule applies only to any descriptive timing.

## C2 — #1164 repair-campaign rules

**Inclusion (confirmatory set).**

- A held-out seeded fault case whose runtime class is declared in the case
  manifest, whose expected diagnostic code and expected repair class are
  recorded, and whose reference repair is executable against the frozen
  stub toolchain (`executability_verified`).
- The case set must not have been used in the 2026-11-18 pilot. If the
  only available case set is the published ten (4 BioTypes, 3 preflight,
  3 infrastructure), the campaign is labeled a replication, not a
  confirmatory test.
- Provider identity, model identifier, endpoint, timeout, token ceiling,
  and retry budget are fixed in PR-03 before the run. A provider change is
  a protocol deviation.

**Exclusion.**

- A case whose fault does not manifest in the seeded run
  (`fault_manifested = false`) is reported but cannot demonstrate repair;
  for the primary endpoint it is ineligible. The reason must be recorded.
- A case with `admission_abort` or `advisory_warning` runtime class is
  outside the primary denominator by the structural rule in PR-01 §C2.
  It remains in the all-case secondary denominator.

**Concrete numbers (defined by the harness and pilot).**

- Pilot case set: **10** cases = 4 BioTypes + 3 preflight + 3
  infrastructure.
- Recovery budget: `--ai-max-retries 1` — one diagnosis round plus at most
  one corrective round per case (frozen in
  `eval/frontier/fault_injection.py` invocation and PR-03).
- Confirmatory held-out case count: **UNDECIDED** (owner: campaign lead;
  deadline: before data collection). It must include at least the same
  three fault classes and must be fixed by hash before the run.

**Missing data.**

- If the provider is unavailable, misconfigured, or returns a transport
  error for a case, the case is **NOT_RUN**, not a repair failure. Any
  NOT_RUN voids the campaign; there is no imputation and no partial-
  denominator repair rate.
- If the engine crashes without producing `repair_applied` evidence, the
  case is a false stage with reason `engine_error`; it stays in the
  denominator.
- Every false stage must carry a specific `repair_stage_why`; missing WHY
  is a protocol violation reported separately and the case is INVALID.

**Stopping / outlier.** No interim analysis. Binary endpoint, so no
outlier trimming. Rerun success is exit-status-only; repeated runs are
summarized as within-case agreement (PR-01 §C2), not as independent
replicates.

## C3 — #1165 equivalence-corpus rules

**Inclusion.**

- A workflow that the importer **claims** to support: it uses only
  constructs inside the documented Nextflow DSL2 / Snakemake subset, and
  the import completes without a hard error.
- An independently reviewed `.oxoflow` counterpart exists, produced from
  the source workflow and reviewed before the run by someone other than
  the importer author. The review record names every manual correction.
- Origin engine and version, container digests or environment locks,
  inputs, reference data, resource limits, and expected declared outputs
  are recorded in the case manifest.
- The case declares **at least one output**. A workflow with zero declared
  outputs cannot be counted as vacuously equivalent; it is excluded before
  the run as INVALID for this endpoint (PR-01 §C3), with the exclusion
  recorded. If one nevertheless enters the analysis, it counts as
  non-equivalent.
- The corpus must include workflows whose sources are Nextflow and
  workflows whose sources are Snakemake; the existing
  `benches/comparative` echo chain is **not** a sufficient corpus (it is
  one synthetic family with one Nextflow implementation and one Snakemake
  implementation) and cannot carry a corpus-wide claim.

**Exclusion.**

- Workflows outside the claimed subset are **not excluded from reporting**:
  they are recorded in the unsupported-construct table with the importer's
  hard-error message. They are outside the equivalence denominator by
  definition, but the boundary is reported as measured, not silently
  deleted.
- A case whose `.oxoflow` counterpart required a semantic change to import
  (not a mechanical translation) is excluded from the equivalence
  denominator and listed with the change; calling it equivalent would
  measure the human correction, not the importer.

**Concrete numbers.** Corpus size is **UNDECIDED** (owner: campaign lead;
deadline: before data collection); so are the number of repeats per arm,
the run-order randomization seed, and the minimum number of cases per
origin engine. The existing repository comparative corpus contributes
**2** engine implementations of one synthetic chain; it is a seed, not the
target size.

**Randomization and caches.** Run order is randomized with a seed recorded
before the first run; every arm starts from a clean cache and a fresh
output directory. The same host, inputs, container digests, and resource
limits are enforced per case.

**Missing data / execution failures.**

- Intention-to-run: a case that fails to execute in an arm is
  non-equivalent, not missing. No exclusion for engine failure.
- Only a pre-run harness defect (host failure, corrupted corpus copy)
  permits exclusion; the replacement is drawn from the sealed reserve list
  and the exclusion is recorded with the failing command.
- Worst-case sensitivity: every excluded case is counted as non-equivalent.

**Stopping / outliers.** No interim analysis. Continuous metrics follow the
common outlier rule (flag and retain). If a source engine cannot run at all
on the frozen host, that is infrastructure invalidation, not a negative
result.

## C4 — #1166 reproduction rules

**Inclusion.**

- A frozen workflow with declared outputs, run with `--provenance`, on both
  arms of the architecture contrast.
- Both arms record the environment-lock hash and container digest (or the
  conda-family lock identity), the engine commit, and the command.
- Declared outputs only: artifacts the workflow declares as outputs.
  Engine-internal logs, checkpoints, session archives, and temporary files
  are not reproducibility artifacts.
- Same inputs and reference data by content hash on both arms.

**Exclusion.**

- An artifact not produced in both arms is not a stability observation; it
  is missing data (below).
- The frozen negative control (`crossarch-governed-a/control/os_arch.txt`,
  produced by `uname -m`) is a declared output but is **not** a primary
  artifact and has no class. It is excluded from the primary endpoint and
  from every class-level statement; it must differ across arms (see
  **Concrete numbers**).
- An artifact whose path differs between arms for a declared reason is
  reported under path parity, not silently matched.
- A workflow whose environment cannot be pinned identically on both arms
  (for example a different solver resolving different builds) is still run,
  but the environment mismatch is recorded; it cannot support a
  reproducibility claim and is reported as a tooling-attribution case.

**Concrete numbers (frozen 2026-11-28; amendment 2).**

- **2 workflows**: `scripts/cross-arch-provenance-governed-a.oxoflow` and
  `scripts/cross-arch-provenance-governed-b.oxoflow`, bound together by the
  frozen corpus manifest `scripts/cross-arch-provenance-governed-corpus.json`
  (its SHA-256 is recorded in the pre-run manifest).
- **24 primary declared artifacts**, 12 per workflow, in **8 artifact
  classes of 3** each: pure-text, raw-binary, derived-text,
  byte-exact-compressed, checksum-manifest, ieee754-float,
  integer-arithmetic, and json-structured.
- **`n_min = 3`**: every class contains exactly 3 artifacts and is therefore
  eligible for a class-level statement. A class is architecture-stable only
  if all 3 of its artifacts are SHA-256 identical.
- **x86_64 host**: the designated `tx-ubuntu` Linux host (native docker).
  **aarch64 host**: the local Apple Silicon macOS host running Docker
  Desktop's **native linux/arm64 VM** (no qemu on either leg). Both container
  legs execute the same multi-arch manifest-list digest; the host OS/kernel
  difference is infrastructure, because the executed toolchain is the pinned
  Linux container.
- **Negative control**: `crossarch-governed-a/control/os_arch.txt`, produced
  by `uname -m`. It must **differ** across the two arms. If it does not, the
  harness cannot detect a known architecture-dependent output and the
  campaign is INVALID (a harness failure, never a stable artifact).
- The architecture count is fixed at **2** (x86_64 and aarch64); a third
  architecture is optional and secondary.

**Missing data (frozen 2026-11-28; amendment 2).**

- Artifacts produced in only one arm are excluded from the primary
  denominator and reported in the flow table with the arm that produced
  them. The missingness limit is **0%**: any missing primary artifact in
  either arm makes the campaign INVALID.
- Worst-case sensitivity: all missing artifacts are counted as differing
  (reported beside the INVALID verdict, never as a stability result).

**Stopping / outliers.** No interim analysis. No continuous primary
endpoint; timing is descriptive only. If the container digest or
environment lock differs between arms, the run is a tooling-attribution
case, not an architecture-stability measurement, and the primary claim is
blocked until an identical-environment rerun exists.
