# PR-01 — Primary and Secondary Outcomes, Direction, and Statistical Models

| Freeze record | Value |
| :--- | :--- |
| Artifact | PR-01 (Task 4.0 required artifact 1 of 6) |
| Version | 1.0.2 |
| Freeze status | FROZEN |
| Freeze date | 2026-11-18 (original freeze; amendment 1 same date) |
| Amendment | 1 (2026-11-18): review fixes D1–D5 and clarifications; 2 (2026-11-27): `τ_FA = 0.05`, held-out confirmatory corpus, `N_min = 73`, zero non-verdict limit for #1163 — see [`AMENDMENTS.md`](AMENDMENTS.md) entries 1–2 |
| Content binding | SHA-256 recorded in [`MANIFEST.sha256`](MANIFEST.sha256) |
| Campaigns covered | #1163, #1164, #1165, #1166 |
| Change control | Post-freeze edits only through [`AMENDMENTS.md`](AMENDMENTS.md) |
| Index and freeze protocol | [`README.md`](README.md) |

**Frozen-before-data statement.** This document was frozen before any
confirmatory data collection for #1163, #1165, and #1166. For #1164 the
freeze is retrospective: a ten-case pilot had already been measured before
this document existed (disclosed in §C2). No confirmatory result exists for
any campaign. The freeze does not itself pass any gate; independent sign-off
is still pending (PR-05).

## 0. Shared analysis rules

These rules apply to every campaign in this document and may not be varied
per campaign after freeze.

1. **One primary endpoint per campaign.** The primary endpoint named in its
   campaign section is the only endpoint that can support a capability
   claim. Every other endpoint is secondary and is reported descriptively;
   secondary endpoints cannot be promoted to primary after outcomes are
   seen.
2. **Unit of analysis.** The unit is the pre-registered case (one truth-set
   case, one seeded fault case, one corpus workflow, or one declared output
   artifact). Repeated runs of the same case are not independent units and
   are summarized as within-case replicates.
3. **Estimation over testing.** None of the four campaigns has a
   randomized treatment-versus-control contrast on the primary endpoint.
   The primary analysis is therefore estimation: a proportion with a
   two-sided 95% confidence interval (CI). A decision threshold, where one
   exists, is stated in PR-06 and is compared against the CI, not against a
   p-value.
4. **Interval method.** Proportions use the exact Clopper–Pearson interval.
   As a pre-declared sensitivity analysis, the same quantity is recomputed
   with the 2000-resample percentile bootstrap already implemented in
   `eval/frontier/statistical_report.py` (`bootstrap_ci`, seed 42,
   alpha 0.05). If the two intervals disagree about the decision threshold,
   the exact interval decides and the disagreement is reported.
5. **Paired binary comparisons.** Where a campaign compares two paired
   binary outcomes, the test is McNemar's test with the Edwards continuity
   correction, as implemented in `eval/frontier/statistical_report.py`.
   Exact binomial McNemar is the sensitivity analysis for discordant
   counts below 25.
6. **Paired continuous comparisons.** Wall time, peak RSS, CPU time, disk
   I/O, and token counts use the two-sided Wilcoxon signed-rank test with
   the Hodges–Lehmann estimator and a 2000-resample bootstrap 95% CI for
   the paired difference. Means and standard deviations are reported beside
   medians and interquartile ranges; the median is the primary summary.
   This follows the statistical reporting requirement in
   `docs/superpowers/plans/industrial-scientific-implementation-plan.md`
   §1.4.
7. **Multiplicity.** Any family of more than one inferential comparison is
   Holm step-down adjusted over exactly the pre-declared family, as in
   `statistical_report.py::holm_adjust_p_values`. Descriptive endpoints are
   not added to a family after the fact. No unadjusted p-value may be
   described as significant.
8. **No interim analysis.** The case set and its size are fixed before the
   first confirmatory run; there is no interim efficacy analysis and no
   early stopping for a favorable result. The only permitted stops are
   listed in PR-02 §0.3.
9. **Analysis code is sealed.** The script that computes every primary
   endpoint must be committed and hashed in the pre-run manifest
   (`PR-03` §0) before the first confirmatory run. An analysis script
   written after outcomes were seen makes the affected endpoint
   exploratory, and the publication must say so.
10. **No silent imputation.** Every excluded or missing case is counted and
    reported in a flow table (included, excluded with reason, missing with
    reason, analyzed). The per-campaign missing-data rules and sensitivity
    analyses are in PR-02.
11. **Direction is pre-declared.** A higher value is better only where the
    campaign section says so. The direction may not be reinterpreted when
    the CI lands on the other side of the threshold.

## C1 — #1163: BioTypes broad truth-set validation

**Estimand.** The false-accept behavior of the BioTypes admission checker
(`validate` / preflight `TYPE-*` diagnostics) on a truth set authored
independently of the checker.

**Truth-set cells.** Each case has an expected verdict (accept or reject)
recorded by its independent author with a rationale, and for reject cases
an expected `TYPE-*` code. The four cells are:

| Cell | Expected | Actual | Meaning |
| :--- | :--- | :--- | :--- |
| TA | accept | accept | true accept |
| FR | accept | reject | false reject |
| TR | reject | reject | true reject |
| FA | reject | accept | **false accept — the dangerous cell (WF-04)** |

`TYPE-008` and `TYPE-009` are advisory by design: `TYPE-001`–`TYPE-007`
fail `validate` and abort a run, while `TYPE-008`/`TYPE-009` surface as
warnings and never fail a run
(`docs/guide/src/reference/diagnostics-engine.md`). A case whose expected
result is an advisory finding is therefore an **expected-accept** case
carrying a required warning code. It stays in the TA/FR cells for the
binary endpoint and is analyzed separately in the advisory-detection
table; it is never counted as a reject case and never enters the
false-accept denominator.

**Primary endpoint.** False-accept proportion,
`FA / (FA + TR)`, over cases whose expected verdict is reject. Direction:
lower is better. The pre-registered threshold is PR-06 §C1.

**Secondary endpoints.**

- False-reject proportion, `FR / (FR + TA)` (lower is better).
- Exact `TYPE-*` attribution accuracy among TR cases: the reported code
  equals the expected code (higher is better).
- Advisory detection: among expected-accept cases that declare an
  `expected_warning_code` in `TYPE-008`/`TYPE-009`, the proportion where
  the advisory code is emitted (higher is better).
- Per-code confusion table for `TYPE-001` through `TYPE-009`, with the
  advisory codes reported on their own rows.
- Accept/reject agreement proportion over all analyzed cases.
- Coverage table by contract dimension (format, sort order, reference
  build, index/companion, read group, compression) so a result cannot be
  generalized past the dimensions the truth set exercises.

**Statistical model.** Two proportions with exact 95% CIs (rule 4). One
primary endpoint means no multiplicity family. Per-code and per-dimension
tables are descriptive; no per-cell significance claim is made. Because
the checker is deterministic, repeated execution of the same case is a
consistency check, not replication: any disagreement between two runs of
the same binary and case is an infrastructure failure and voids that case.

**Confirmatory corpus and minimum sample (amendment 2, frozen
2026-11-27).** The threshold is `τ_FA = 0.05` (PR-06 §C1). The
confirmatory analysis set is the held-out governed corpus of
**N_min = 73 expected-reject cases**, plus at least 30 expected-accept
acceptance controls, authored after the amendment from the published
contract and disjoint from the 54-case pilot. The pilot remains
**exploratory/exposure** under the C1 pilot firewall in PR-06 §C1; it is
reported separately and never pooled. Fewer than 73 expected-reject cases
is INVALID with sub-reason `minimum_sample_not_reached`
(UNDERPOWERED). Zero non-verdicts are permitted (PR-02 §C1).

**What the result can and cannot support.** A pass supports the checker
only on the frozen truth set and its declared coverage. It does not
establish biological truth, does not validate pipelines outside the
coverage table, and does not close WF-04 globally.

## C2 — #1164: applied-repair verified by successful reruns

> **Disclosure — the outcome has already been observed once.** A ten-case
> pilot was measured on 2026-11-18, before this document was frozen:
> repair applied 3/10, verified rerun 3/10, and 3/3 of the structurally
> recovery-eligible class closed the loop (GLM via the school proxy; raw
> logs under `eval/frontier/results/fault_repair_logs/`; summary in
> `eval/frontier/PILOT_FINDINGS.md`). Those numbers are pilot evidence.
> They are **not** confirmatory evidence under this pre-registration, and
> they may not be cited as a pre-registered result. A confirmatory #1164
> analysis must use a held-out fault-case set that was not used in the
> pilot. Re-running the ten published cases is a **replication of a known
> outcome** and must be labeled as such; it cannot become confirmatory by
> being run after the freeze.

**Eligibility is structural, not outcome-based.** A seeded case is
recovery-eligible when its declared runtime class is `rule_failure` — a
rule actually exits non-zero, so the `--ai-recover` path can engage. Cases
whose runtime class is `admission_abort` (the BioType gate stops the run
before any rule executes) or `advisory_warning` (preflight warns but the
run succeeds) are recorded and reported, but they cannot close a repair
loop by construction; counting them in the primary denominator would
measure code-path reachability, not repair. This eligibility rule comes
from `eval/frontier/fault_injection.py` and was visible in the pilot; it is
declared here so it cannot be changed after a confirmatory outcome.

**Primary endpoint.** Full-loop verified-repair proportion,

`cases with repair_applied AND repair_verified AND rerun_exit_0 / recovery-eligible cases`

measured without a human editing the workflow. Direction: higher is
better. A case counts only when the engine's own recovery produced the
applied patch; `executability_verified` (the harness's reference repair
runs) is not evidence of engine repair and is never substituted.

**Secondary endpoints.**

- The same full-loop proportion over **all** seeded cases, including the
  structurally ineligible classes (the conservative headline number).
- Stage cascade counts: static detection, diagnostic attribution, repair
  proposal, application, verification, successful rerun.
- Per-class breakdown (BioTypes, preflight, infrastructure).
- WHY completeness: the proportion of false stages that carry a recorded,
  specific reason. The pre-registered requirement is 100%; a false stage
  without a reason is a protocol violation, not a data point.
- Repeat determinism: the per-case agreement rate across R repeats
  (R is fixed in PR-03; the provider may sample non-deterministically, so
  agreement is measured, not assumed).

**Statistical model.** Exact 95% CI for the primary proportion (rule 4).
No hypothesis test and no comparator arm. The per-class breakdown is
descriptive. Rerun success is judged by exit status only; biological
correctness of the patch is explicitly out of scope and must be stated in
any report (stub tools produce the outputs).

## C3 — #1165: corpus-wide Nextflow/Snakemake semantic equivalence

**Unit and arms.** Each corpus case originates in one engine (Nextflow or
Snakemake) and has an independently reviewed `.oxoflow` counterpart. The
**origin engine is the reference arm**; the `.oxoflow` arm is compared to
it. Where a third-engine implementation exists it is a secondary
comparison, not part of the primary endpoint. All arms run on the same
host, inputs, reference data, container digests, resource limits,
filesystem, and scheduler state, with clean caches between arms and
randomized run order.

**Equivalence predicate (primary).** A declared output is equivalent only
when its file content SHA-256 is identical across arms. A case is
equivalent only when **every** declared output is byte-identical **and
the case declares at least one output**. A case with zero declared outputs
is vacuously "all outputs identical" and is therefore INVALID for this
endpoint: it must be excluded before the run (PR-02 §C3), and if one
nevertheless enters the analysis it counts as non-equivalent, never as
equivalent. Text normalization, header stripping, and format
canonicalization are **not** part of the primary endpoint.

**Primary endpoint.** Strict declared-output equivalence proportion over
the frozen corpus, `equivalent cases / analyzed cases`, where analyzed
cases have at least one declared output. Direction: higher is better.

**Secondary endpoints.**

- Normalized equivalence under a pre-declared, hash-sealed normalizer
  (exact field rules fixed in PR-03 before data collection). This is the
  endpoint that can distinguish "byte-different but semantically equal"
  from "semantically different".
- Completion parity (all cases complete in every compared arm).
- Failure-mode parity: for cases that fail, whether the arm that fails and
  the diagnostic class agree.
- Paired continuous operational metrics: wall time, peak RSS, CPU time,
  disk I/O, with Wilcoxon signed-rank and bootstrap CI (rule 6).
- Restart behavior parity.
- Unsupported-construct coverage: every unsupported construct encountered
  is enumerated with the hard-error message; difficult cells are retained,
  never deleted.

**Statistical model.** Proportions with exact 95% CIs. Pairwise binary
comparisons (`.oxoflow` vs Nextflow, `.oxoflow` vs Snakemake) use McNemar
with Holm adjustment over that two-test family. Continuous metrics use
Wilcoxon with Holm over the pre-declared metric list. The strict primary
endpoint has no multiplicity family. A normalized-equivalence result can
never replace a failed strict primary in a summary; it is reported
separately and labeled as normalization-dependent.

**Intention-to-run.** A case that cannot execute in an arm is a
non-equivalent completion failure, not missing data. Excluding execution
failures from the denominator would turn the measured subset boundary into
an unmeasured one.

## C4 — #1166: cross-architecture reproduction measurement

**Unit and contrast.** The unit is one declared output artifact produced
by the frozen workflow. The primary contrast is the same operating system
and toolchain on two CPU architectures (x86_64 and aarch64), with identical
container digests or identical environment-lock hashes. A comparison that
also changes the OS (for example macOS-arm64 versus Linux-x86_64) is a
declared **confounded** comparison: it measures architecture plus OS plus
toolchain and cannot support a pure architecture claim. It may be reported
as an operational field observation, never as the primary result.

**Primary endpoint.** Architecture-stable artifact proportion,

`artifacts with identical SHA-256 in both arms / artifacts produced in both arms`

over declared outputs of the frozen workflow. Direction: higher is better.

**Secondary endpoints.**

- Per-artifact-class stability (for example BAM, VCF, FASTQ, TSV, HTML
  report), each with its own exact 95% CI.
- Workflow completion parity and exit-status parity.
- Environment-lock hash equality and container-digest equality per arm,
  recorded so a mismatch can be attributed to tooling rather than to
  non-determinism.
- For every differing artifact, the differing set enumerated by path and
  hash pair.
- Wall time and peak RSS as descriptive operational data, not as the
  reproducibility claim.

**Statistical model.** Exact 95% CIs per artifact class. No hypothesis
test and no p-value: the question is which artifact classes are stable,
not whether a difference is significant. Multiple classes are reported
descriptively without a significance family. A class is called
architecture-stable only under the rule in PR-06 §C4, which requires the
pre-declared minimum artifact count and 100% agreement.

## 5. Endpoint summary

| Campaign | Primary endpoint | Direction | Decision threshold |
| :--- | :--- | :--- | :--- |
| #1163 | False-accept proportion on expected-reject cases | lower is better | PR-06 §C1: `τ_FA = 0.05` (frozen 2026-11-27, amendment 2); `N_min = 73` expected-reject cases |
| #1164 | Full-loop verified-repair proportion, recovery-eligible cases | higher is better | PR-06 §C2 (τ value UNDECIDED; owner: campaign lead + independent reviewer; deadline: before data collection) |
| #1165 | Strict declared-output equivalence proportion | higher is better | PR-06 §C3 (τ value UNDECIDED; owner: campaign lead + independent reviewer; deadline: before data collection) |
| #1166 | Architecture-stable artifact proportion | higher is better | PR-06 §C4 (per-class minimum count and τ UNDECIDED; owner: campaign lead + independent reviewer; deadline: before data collection) |

An UNDECIDED threshold is a deliberate, visible gap. It must be filled by a
pre-data amendment (PR-05/README freeze protocol) before the first
confirmatory run; leaving it unfilled blocks the campaign. It must not be
filled after outcomes are seen.
