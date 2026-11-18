# PR-06 — Publication Decision Rule (Negative Results Included)

| Freeze record | Value |
| :--- | :--- |
| Artifact | PR-06 (Task 4.0 required artifact 6 of 6) |
| Version | 1.0.0 |
| Freeze status | FROZEN |
| Freeze date | 2026-11-18 |
| Content binding | SHA-256 recorded in [`MANIFEST.sha256`](MANIFEST.sha256) |
| Campaigns covered | #1163, #1164, #1165, #1166 |
| Change control | Post-freeze edits only through [`AMENDMENTS.md`](AMENDMENTS.md) |

**Frozen-before-data statement.** This decision rule was frozen before
confirmatory data collection for #1163, #1165, and #1166. #1164's pilot
outcome was already observed; §C2 states how it is prevented from deciding
the confirmatory publication.

## 0. Decision categories

Every campaign resolves to exactly one of five categories, using its
pre-registered primary endpoint and the rule in its section:

| Category | Meaning | Publication obligation |
| :--- | :--- | :--- |
| **POSITIVE** | The primary endpoint meets the pre-registered threshold, and its 95% CI supports the direction | Publish the measured value, CI, coverage, exclusions, and the bounded claim. No claim beyond the frozen coverage. |
| **NEGATIVE** | The primary endpoint does not meet the threshold, or the CI supports the opposite direction | Publish with the same prominence as a positive result. The negative statement in the campaign section is the required result statement; it may not be buried, softened to "inconclusive", or omitted. |
| **INCONCLUSIVE** | The CI spans the threshold, or the pre-declared minimum sample was not reached | Publish the estimate and CI; explicitly state that no capability claim is supported. This is not a negative result and not a positive one. |
| **INVALID** | A pre-declared validity condition failed (missingness limit, non-verdict limit, leakage breach, protocol deviation, provider outage) | Publish the invalidity reason and the flow table; no endpoint estimate may be presented as the campaign result. |
| **STOPPED** | A permitted stop under PR-02 §0.3 occurred | Publish the stop reason; no capability claim. |

A negative result is a **complete** result. The research program's change
discipline (ledger §8) requires it to be reported, and this rule makes the
negative branch the default outcome statement when the threshold is not
met, rather than an afterthought.

## 1. Rules common to every campaign

1. **No outcome switching.** The primary endpoint is the one in PR-01. If
   it fails, a secondary endpoint that passes cannot be promoted to the
   headline or the abstract.
2. **No threshold movement.** The decision threshold is fixed before data
   collection. If it is still `UNDECIDED` when outcomes are seen, the
   campaign is INVALID for the confirmatory claim, not "descriptive".
3. **No denominator movement.** The analysis set is the sealed case set
   with the PR-02 missing-data rules applied mechanically. Recomputing on a
   favorable subset is a separate, clearly labeled exploratory analysis.
4. **No reclassification of direction.** "Lower is better" and "higher is
   better" are fixed in PR-01. A result on the wrong side is negative, not
   a reversed hypothesis.
5. **Independent reanalysis.** No primary endpoint is publication-ready
   until an independent analyst reproduces it from the raw per-case data
   and the sealed analysis script. The reproduction record is part of the
   artifact package.
6. **Raw data retention.** Every per-case record, command, binary hash,
   host identity, exclusion, and reason is retained per ledger §8. A
   result without raw data is not publishable.
7. **Bounded claims.** A positive result supports the capability only on
   the frozen case set, coverage table, model, hardware, and versions. The
   abstract must name those boundaries.
8. **Program-level honesty.** Passing one campaign does not pass ledger
   Gate 3. Gate 3 requires all of its bullets, including independent
   biological validation, false-positive/false-negative analysis,
   same-host Nextflow/Snakemake comparisons, and reproduction on an
   independent environment. No single campaign may be described as
   "scientifically valid" in the gate sense.

## C1 — #1163 BioTypes truth set

**Threshold.** `τ_FA` is **UNDECIDED** (owner: campaign lead +
independent reviewer; deadline: before data collection). It is the maximum
acceptable false-accept proportion on expected-reject cases.

**Decision rule.**

- POSITIVE if the exact 95% upper confidence bound for `FA/(FA+TR)` is at
  or below `τ_FA`.
- NEGATIVE if the point estimate exceeds `τ_FA`, or the lower confidence
  bound exceeds `τ_FA`, or the point estimate is at or below `τ_FA` but
  the upper bound exceeds it (threshold-fragile negative; report both).
- INCONCLUSIVE if the interval spans `τ_FA` without the point estimate
  exceeding it and the pre-declared minimum sample was reached; extend
  with a new pre-registered sample if resources allow.
- INVALID under PR-02 §C1 missingness or a PR-04 breach.

**Required negative statement (template).**

> On a truth set of `N` independently authored cases, the BioTypes checker
> false-accepted `FA` of `FA+TR` expected-reject cases (`FA` rate
> `p`%, exact 95% CI `[lo, hi]`), which does not meet the pre-registered
> threshold `τ_FA`. The checker is therefore not validated on this truth
> set; WF-04 remains open, and no accuracy claim beyond the reported
> cells is supported.

**Positive statement (template).**

> On the frozen truth set and its declared coverage (list dimensions), the
> false-accept rate was `p`% (exact 95% CI `[lo, hi]`), within the
> pre-registered bound `τ_FA`. This validates the checker on this set; it
> does not establish biological truth or coverage outside the listed
> dimensions.

## C2 — #1164 applied repair

**Threshold.** `τ_R` is **UNDECIDED** (owner: campaign lead + independent
reviewer; deadline: before data collection). It is the minimum full-loop
verified-repair proportion among recovery-eligible cases.

**Pilot firewall.** The 2026-11-18 pilot (3/10 applied, 3/10 verified,
3/3 recovery-eligible) is already observed. It cannot set `τ_R`, cannot
serve as the confirmatory result, and cannot be pooled with a held-out
confirmatory set. A publication may cite it only as pilot evidence with
that label. If the confirmatory campaign reuses the ten published cases,
the paper must call the analysis a replication, and `τ_R` must have been
fixed before the replication run.

**Decision rule.**

- POSITIVE if the exact 95% lower confidence bound for the primary
  proportion is at or above `τ_R`.
- NEGATIVE if the point estimate is below `τ_R`, or the upper bound is
  below `τ_R`, or the point estimate meets `τ_R` but the lower bound does
  not (threshold-fragile negative; report both).
- INCONCLUSIVE if the interval spans `τ_R` and the point estimate meets
  it; report the width and make no capability claim.
- INVALID under PR-02 §C2 (any NOT_RUN, missing WHY, provider change,
  or leakage breach).

**Required negative statement (template).**

> Across `N` held-out seeded fault cases, the engine closed the full
> repair loop (patch applied by the engine, verified, and rerun to exit 0
> without human editing) in `k` cases (`p`%, exact 95% CI `[lo, hi]`)
> among `E` recovery-eligible cases. This does not meet the pre-registered
> threshold `τ_R`. The measured capability remains "repair proposals with
> experimental application"; the all-case secondary count was `k_all/N_all`
> (`p_all`%). No self-healing claim is supported.

**Positive statement (template).**

> The engine closed the full repair loop in `k` of `E` recovery-eligible
> held-out cases (`p`%, exact 95% CI `[lo, hi]`), meeting `τ_R`. The result
> covers the seeded fault classes, the named model, and the stub toolchain;
> it does not establish biological correctness of the patches or repair of
> admission-gate failures.

## C3 — #1165 semantic equivalence

**Threshold.** `τ_EQ` is **UNDECIDED** (owner: campaign lead + independent
reviewer; deadline: before data collection). It is the minimum strict
declared-output equivalence proportion over the frozen corpus.

**Decision rule.**

- POSITIVE if the exact 95% lower confidence bound for strict
  equivalence is at or above `τ_EQ`.
- NEGATIVE if the point estimate is below `τ_EQ`, or the upper bound is
  below `τ_EQ`, or the point estimate meets `τ_EQ` but the lower bound
  does not (threshold-fragile negative; report both).
- INCONCLUSIVE if the interval spans `τ_EQ`; no replacement claim.
- INVALID under PR-02 §C3 (corpus seal breach, mid-campaign system
  change, or the pre-declared missingness limit).

**Normalized equivalence.** A positive normalized-equivalence result with
a negative strict result is reported as **NEGATIVE for strict equivalence**
and separately as a normalization-dependent observation. It may not be
written as "semantically equivalent" without naming every normalization
rule and showing the differing-field set.

**Required negative statement (template).**

> On a frozen corpus of `N` workflow cases, strict declared-output
> equivalence (SHA-256 identity for every declared output) held in `k`
> cases (`p`%, exact 95% CI `[lo, hi]`). This does not meet `τ_EQ`.
> Unsupported constructs encountered: `U`; cases excluded for required
> semantic changes: `E`. The importer is therefore not shown to preserve
> semantics corpus-wide, and "universal transpiler" remains unverified
> beyond the documented subset.

**Positive statement (template).**

> Strict declared-output equivalence held in `k/N` cases (`p`%, exact 95%
> CI `[lo, hi]`), meeting `τ_EQ`, on the frozen corpus and pinned
> container digests. Manual corrections in the counterparts: `E`. No claim
> is made for workflows outside the documented supported subset.

## C4 — #1166 cross-architecture reproduction

**Threshold.** The minimum artifact count per class `n_min` and the
stability threshold `τ_ARCH` are **UNDECIDED** (owner: campaign lead +
infrastructure owner + independent reviewer; deadline: before data
collection). `τ_ARCH` is expected to equal 1.0 (all artifacts identical)
for a class to be called stable; the value must be fixed explicitly before
the run, not assumed.

**Decision rule.**

- POSITIVE for an artifact class if at least `n_min` artifacts were
  produced in both arms, all are SHA-256 identical (`τ_ARCH = 1.0`), and
  the exact 95% lower bound for the class proportion is reported. The
  campaign-level POSITIVE requires the primary proportion over all
  declared artifacts to meet its fixed threshold.
- NEGATIVE if any artifact class with at least `n_min` artifacts contains
  a differing artifact, or the primary proportion is below threshold. The
  differing set is enumerated by path and hash pair.
- INVALID if the environment lock or container digest differs between
  arms (tooling confound), if the missingness limit is exceeded, or if
  only the confounded macOS-arm64 versus Linux-x86_64 contrast is
  available for the primary claim.
- No INCONCLUSIVE category replaces the tooling-confound INVALID: an
  environment mismatch is a measurement failure, not a weak stability
  result.

**Required negative statement (template).**

> On `N` declared artifacts of the frozen workflow on `x86_64` and
> `aarch64` with identical container digest `D`, `k` artifacts were
> bit-identical (`p`%, exact 95% CI `[lo, hi]`). Classes `C1…Cn` are not
> architecture-stable; the differing set is enumerated in Table `T`.
> Cross-architecture reproducibility is therefore limited to the stable
> classes listed, and no general reproducibility claim is supported.

**Positive statement (template).**

> On `N` declared artifacts with identical container digest `D` and
> matching environment locks, all artifacts were SHA-256 identical across
> `x86_64` and `aarch64` (`p`%, exact 95% CI `[lo, hi]`). The claim is
> limited to this workflow, these tool versions, and these artifact
> classes; it does not establish bitwise reproducibility for arbitrary
> tools or workflows.

## 5. Publication package requirements

A publication drawing on any campaign must include:

1. This pre-registration, the manifest hash record, and the amendment log.
2. The raw per-case data and analysis script, or a declared replication
   package with the same content.
3. The independent reanalysis record.
4. The flow table of included, excluded, missing, and analyzed cases.
5. The campaign's category (POSITIVE / NEGATIVE / INCONCLUSIVE / INVALID
   / STOPPED) and the exact statement template filled with the measured
   values.
6. Every declared limitation, including #1164's pilot exposure and any
   confounded architecture comparison.

A paper that reports only positive campaigns, or that presents a
projection, simulation, or pilot as a confirmatory result, violates ledger
§8 and this rule. A negative campaign is a reportable finding and must
appear in the package with the same prominence as a positive one.
