# PR-06 — Publication Decision Rule (Negative Results Included)

| Freeze record | Value |
| :--- | :--- |
| Artifact | PR-06 (Task 4.0 required artifact 6 of 6) |
| Version | 1.0.1 |
| Freeze status | FROZEN |
| Freeze date | 2026-11-18 (original freeze; amendment 1 same date) |
| Amendment | 1 (2026-11-18): review fixes D1–D5 and clarifications — see [`AMENDMENTS.md`](AMENDMENTS.md) entry 1 |
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
| **POSITIVE** | The 95% CI lies entirely on the favorable side of the threshold, including the boundary | Publish the measured value, CI, coverage, exclusions, and the bounded claim. No claim beyond the frozen coverage. |
| **NEGATIVE** | The 95% CI lies entirely on the unfavorable side of the threshold, excluding the boundary | Publish with the same prominence as a positive result. The negative statement in the campaign section is the required result statement; it may not be buried, softened to "inconclusive", or omitted. |
| **INCONCLUSIVE** | The CI is neither entirely favorable nor entirely unfavorable under the boundary rules in §0.1: it contains the threshold in its interior or on its unfavorable edge | Publish the estimate and CI; explicitly state that no capability claim is supported. This is not a negative result and not a positive one. |
| **INVALID** | A pre-declared validity condition failed (missingness limit, non-verdict limit, minimum sample not reached, leakage breach, protocol deviation, provider outage) | Publish the invalidity reason and the flow table; no endpoint estimate may be presented as the campaign result. |
| **STOPPED** | A permitted stop under PR-02 §0.3 occurred before a complete primary-endpoint dataset existed | Publish the stop reason; no capability claim. |

A negative result is a **complete** result. The research program's change
discipline (ledger §8) requires it to be reported, and this rule makes the
negative branch the default outcome statement when the threshold is not
met, rather than an afterthought.

### 0.1 Ordered evaluation — exactly one category applies

The five categories are decided by the following ordered algorithm. An
earlier step decides the category and the later steps are not evaluated,
so the categories are mutually exclusive and exhaustive.

1. **INVALID first.** If any validity condition in PR-02 or the campaign
   section holds — including a missingness/non-verdict limit breach, the
   pre-declared minimum sample not being reached, a leakage breach, a
   system-identity change, or a provider outage — the campaign is
   **INVALID**, regardless of the point estimate or CI.
2. **STOPPED second.** If a permitted stop under PR-02 §0.3 occurred and
   no complete primary-endpoint dataset exists, the campaign is
   **STOPPED**. A stop condition that is itself an invalidity condition is
   decided at step 1 as INVALID.
3. **Campaign-specific precedence.** For #1166, an unstable eligible
   artifact class decides **NEGATIVE** before the interval partition in
   step 4 (see §C4). No other campaign has a class-level override.
4. **Interval partition.** With `τ` fixed before data collection, `L` the
   exact 95% lower bound, and `U` the exact 95% upper bound:
   - **Higher is better:** POSITIVE iff `L ≥ τ`; NEGATIVE iff `U < τ`;
     otherwise INCONCLUSIVE (`L < τ ≤ U`).
   - **Lower is better:** POSITIVE iff `U ≤ τ`; NEGATIVE iff `L > τ`;
     otherwise INCONCLUSIVE (`L ≤ τ < U`).

   The three outcomes are mutually exclusive and cover every possible
   interval with `L ≤ U`. The boundary is inclusive for POSITIVE and
   exclusive for NEGATIVE. Concretely, higher-is-better: `L = τ` is
   POSITIVE, and `U = τ` with `L < τ` is INCONCLUSIVE. Lower-is-better:
   `U = τ` is POSITIVE, and `L = τ` with `U > τ` is INCONCLUSIVE.
5. **Point estimates never decide.** The point estimate is reported, but
   it cannot move the category. There is no separate "threshold-fragile"
   category: an observation that the point estimate meets `τ` while the CI
   is wide is INCONCLUSIVE under the interval rules above, and a point
   estimate on the unfavorable side with a CI that clears the boundary is
   NEGATIVE.

The same algorithm is applied independently to each campaign; the
campaign sections below add only their threshold name and their explicit
negative statement.

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

**Decision rule (lower is better; §0.1 order).**

- INVALID first: PR-02 §C1 missingness/non-verdict limit, pre-declared
  minimum sample not reached, or a PR-04 breach.
- POSITIVE iff the exact 95% upper bound `U ≤ τ_FA`.
- NEGATIVE iff the exact 95% lower bound `L > τ_FA`.
- INCONCLUSIVE iff `L ≤ τ_FA < U`.
- The point estimate is reported; it cannot decide the category.

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

**Decision rule (higher is better; §0.1 order).**

- INVALID first: PR-02 §C2 (any NOT_RUN, missing WHY, provider change,
  or leakage breach) or the pre-declared minimum sample not reached.
- POSITIVE iff the exact 95% lower bound `L ≥ τ_R`.
- NEGATIVE iff the exact 95% upper bound `U < τ_R`.
- INCONCLUSIVE iff `L < τ_R ≤ U`.
- The point estimate is reported; it cannot decide the category.

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

**Decision rule (higher is better; §0.1 order).**

- INVALID first: PR-02 §C3 (corpus seal breach, mid-campaign system
  change, pre-declared missingness limit) or any case with zero declared
  outputs entering the denominator (PR-01 §C3).
- POSITIVE iff the exact 95% lower bound `L ≥ τ_EQ`.
- NEGATIVE iff the exact 95% upper bound `U < τ_EQ`.
- INCONCLUSIVE iff `L < τ_EQ ≤ U`; no replacement claim.
- The point estimate is reported; it cannot decide the category.

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

**Thresholds.** Two values are **UNDECIDED** (owner: campaign lead +
infrastructure owner + independent reviewer; deadline: before data
collection), and they govern different things:

- `τ_ARCH` is the **campaign-level** threshold for the primary endpoint
  (the overall architecture-stable artifact proportion over all declared
  artifacts). It is a free parameter fixed before data collection.
- `n_min` is the minimum number of artifacts a class must contain to be
  eligible for a class-level statement.

Class-level stability is **not** parameterized: a class is
**architecture-stable only if every artifact in it is SHA-256 identical**.
There is no threshold that can call a class with a known differing
artifact stable. `τ_ARCH` never relaxes class membership.

**Decision rule (higher is better; §0.1 order).**

1. INVALID first: the environment lock or container digest differs
   between arms (tooling confound), the missingness limit is exceeded, a
   pre-declared minimum sample is not reached, or only the confounded
   macOS-arm64 versus Linux-x86_64 contrast is available for the primary
   claim. An environment mismatch is a measurement failure, not a weak
   stability result, and no INCONCLUSIVE category replaces it.
2. NEGATIVE precedence: if any eligible artifact class (`≥ n_min`
   artifacts produced in both arms) contains at least one differing
   artifact, the campaign is NEGATIVE **even if** the overall proportion
   satisfies the POSITIVE interval rule. The differing set is enumerated
   by path and hash pair. This is the class-level override in §0.1 step 3.
3. Otherwise apply the §0.1 interval partition to the campaign-level
   primary proportion: POSITIVE iff `L ≥ τ_ARCH`; NEGATIVE iff
   `U < τ_ARCH`; INCONCLUSIVE iff `L < τ_ARCH ≤ U`.
4. Classes with fewer than `n_min` artifacts are ineligible for a
   class-level statement; they remain in the campaign-level primary
   proportion and are reported with their counts. They cannot create a
   class-level NEGATIVE, and they cannot be called stable.

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
