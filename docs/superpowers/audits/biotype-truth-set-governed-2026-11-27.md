# BioType governed truth-set campaign — POSITIVE (#1163 / #1396)

> **Status: GOVERNED POSITIVE, on a held-out corpus, with independent
> reanalysis and PR-05 sign-off still PENDING.** The frozen pre-registration
> (PR-06 amendment 2, 2026-11-27) fixed `τ_FA = 0.05` and `N_min = 73`
> **before** any case of this corpus was authored or run. On the sealed
> 116-case held-out corpus the false-accept rate over expected-reject cases
> was **0/83 = 0.0000%**, exact two-sided 95% Clopper–Pearson CI
> **[0.0000%, 4.3471%]**. The upper bound is at or below `τ_FA = 0.05`, and
> `N = 83 ≥ N_min = 73`, so PR-06 §C1's ordered rule returns **POSITIVE**.
>
> The 54-case operational set measured on 2026-11-20/22 was observed while
> `τ_FA` was UNDECIDED. It is **exploratory/exposure**, is not pooled with
> this corpus, and is reported separately in §8. The governed claim is
> bounded to the held-out corpus and the exact `τ_FA`; it is not a
> biological-truth claim and does not pass ledger Gate 3.
>
> A post-run pilot-disjointness audit (§8.1) identifies two cases that
> arguably re-assert a pilot index cell under a strict reading. They were
> not excluded before the run; PR-02 §0.2 forbids post-outcome exclusion,
> so the sealed N=83 set is the primary analysis. The descriptive
> sensitivity with every plausible exclusion (down to N=79) is still
> POSITIVE with a 95% upper bound of 4.56%. The independence
> interpretation is a judgment call for PR-05 review.

## 1. What was measured

The BioType checker (`.oxoflow` `[types.*]` / `[rules.types]` contracts) was
exercised through the real CLI admission surface,
`oxo-flow validate <case> --json`, on a **held-out governed corpus of 116
cases** authored **after** the amendment from the published contract:

| Role | N | Meaning |
| :--- | :--- | :--- |
| Expected-reject (primary analysis set) | 83 | asserts a blocking `TYPE-001`–`TYPE-007` violation |
| Expected-accept acceptance controls | 33 | advisory `TYPE-008`/`009`/`010` shapes, multi-dimensional compatible contracts, untyped pipelines, real gallery examples |

Every case's expected verdict, expected `TYPE-*` code set, and one-line
rationale were recorded in its header **before the run**. The case list and
expectations were then sealed in the pre-run manifest; the campaign runner
reads the manifest's `cases` array as its single EXPECT source, so the
sealed list and the executed list cannot drift.

The classification of a case as *expected-reject* or *expected-accept* was
authored from:

- the format / sort-order / index / reference-build / compression /
  read-group / attribute semantics in
  `docs/guide/src/reference/workflow-format.md` (Biological Type Contracts),
- the `TYPE-001`–`TYPE-010` catalogue in
  `docs/guide/src/reference/diagnostics-engine.md`,
- the `bioTypeSpec` field enum in `docs/schema/oxoflow-v1.schema.json`, and
- published tool/format contracts (SAM/BAM/CRAM, BGZF vs gzip/zstd, GATK
  `@RG` requirements, tabix/CSI index semantics).

The checker source was **not** read to author any expected verdict, and no
case body was edited after an outcome was seen.

## 2. Governance chain (timestamps are commit timestamps)

| Step | Commit | Timestamp (UTC) | Artifact |
| :--- | :--- | :--- | :--- |
| Pre-data amendment 2 | `89c4127d78925959cbb6a718595eb78cd2fff9a8` | `2026-11-27T16:46:43Z` | `docs/superpowers/preregistration/` (`AMENDMENTS.md` entry 2; PR-01/02/03/04/06 v1.0.2; manifest `7aeb3ebb…7afb18`) |
| Held-out corpus + sealed runner/analysis | `20ad630be42ec248e2e96c3729630d849e7c6539` | `2026-11-27T16:49:53Z` | 110 `.oxoflow` files (107 cases + 3 include fixtures) + `scripts/biotype-truthset-governed{,-analysis}.py` |
| Pre-run manifest sealed | `6fbf38c1441c966aee49aa96ba2a211cbf5bfb3f` | `2026-11-27T16:50:17Z` | `preregistration/prerun/C1-governed-2026-11-27.json` (+`.sha256`) |
| Confirmatory run | tree at `6fbf38c1` | `2026-11-27T16:50:33Z` | results JSON (§10) |
| Results + decision committed | `365e563ac5bc35c31860182552a3571131937573` | `2026-11-27T16:51:17Z` | `audits/data/biotype-truthset-governed-{results,decision}.json` |

The amendment commit precedes the corpus commit, which precedes the seal,
which precedes the run. The pre-run manifest's hash and the analysis
script's hash were fixed before the first confirmatory execution.

### 2.1 Pre-run manifest (the seal)

| Field | Value |
| :--- | :--- |
| Manifest | `docs/superpowers/preregistration/prerun/C1-governed-2026-11-27.json` |
| Manifest SHA-256 | `cab5cac4990a5755a86ec7029cfb9a6ebea9be920230c1949ea0915f2c892ff3` |
| `engine_commit` | `20ad630be42ec248e2e96c3729630d849e7c6539` |
| `engine_version` | `oxo-flow 0.32.0` |
| `binary_sha256` | `463d15e618e28d4e694bc3aab472fe53a11188266e8f3f61a344e3099a820b0a` |
| `schema_sha256` | `7f68d09a481f28a1568fb9a37eb14a75bc6ef57d2d504891ef8ce5aeebca47e0` (`docs/schema/oxoflow-v1.schema.json`) |
| `case_manifest_sha256` | `b5db36530550ebda1f11478e7edecc2fe5e514225e669ebad6940203d7d1f3e1` |
| `analysis_script_sha256` | `c5c0311b4f509f93eb4a8f1e40a1ff97bd9536065271d85d5e7263ea04739fbb` |
| `runner_script_sha256` | `e7a8e5fde464faebeedfff220ca75f730c2ddf752f52725027cebbbbffffd0ea` |
| Model / prompt / seed | none — deterministic static checker, no model call, no seed |
| Dataset | 116 sealed cases: 83 expected-reject + 33 controls; pilot excluded |
| Budget | 60 s per case, 7200 s total, static-only, no network |
| Decision constants | `τ_FA = 0.05`, `N_min = 73`, non-verdict limit 0, Clopper–Pearson exact 95% |

The `.sha256` companion verifies:

```bash
cd docs/superpowers/preregistration/prerun
shasum -a 256 -c C1-governed-2026-11-27.json.sha256
# C1-governed-2026-11-27.json: OK
```

## 3. Environment identity (ledger §8)

| Field | Value |
| :--- | :--- |
| hostname | `ShixiangWang-WorkSpacedeMacBook-Pro.local` |
| `uname -srm` | `Darwin 27.2.0 arm64` |
| CPU | Apple M5 |
| RAM | 25,769,803,776 bytes (24 GiB) |
| rustc | `1.98.0 (88d9e12ae 2026-08-18)` |
| oxo-flow | `0.32.0` (debug build `target/debug/oxo-flow`) |
| binary SHA-256 | `463d15e618e28d4e694bc3aab472fe53a11188266e8f3f61a344e3099a820b0a` |
| run commit | `6fbf38c1441c966aee49aa96ba2a211cbf5bfb3f` |
| run timestamp (UTC) | `2026-11-27T16:50:33Z` |

Single arm. The checker is platform-independent pure Rust, so a single arm
is the correct design for a *logic* truth set; no cross-architecture
contrast is claimed (unlike #1166).

## 4. Exact commands

```bash
# 1. build the CLI the harness drives (from the sealed engine commit)
cargo build -p oxo-flow-cli

# 2. run the governed corpus through the real admission surface; the sealed
#    pre-run manifest is the EXPECT source
python3 scripts/biotype-truthset-governed.py \
  --binary target/debug/oxo-flow \
  --manifest docs/superpowers/preregistration/prerun/C1-governed-2026-11-27.json \
  --out docs/superpowers/audits/data/biotype-truthset-governed-results.json

# 3. apply the frozen PR-06 §C1 decision rule (sealed exact-CI script)
python3 scripts/biotype-truthset-governed-analysis.py \
  docs/superpowers/audits/data/biotype-truthset-governed-results.json \
  --out docs/superpowers/audits/data/biotype-truthset-governed-decision.json
```

The runner invokes, per case, `oxo-flow validate <case> --json` (the
verdict surface) and `oxo-flow lint <case> --json` (only to union the
advisory `TYPE-008`/`009`/`010` codes that the admission gate drops). A
case whose `validate --json` output cannot be parsed is a **non-verdict**;
the frozen limit is zero.

## 5. Results

### 5.1 Confusion matrix (verdict scoring)

```
  2x2 confusion matrix (rows = expected, cols = actual)
                     accept   reject
    EXPECT-ACCEPT    33       0
    EXPECT-REJECT    0        83

  true-accept=33  false-accept=0  true-reject=83  false-reject=0
```

All 116/116 cases match their pre-registered verdict. The expected-accept
controls include the nine real gallery examples not used by the pilot
(`01`, `02`, `03`, `04`, `05`, `10`, `11`, `12`, `15`), all accepted.

### 5.2 Diagnostic-code cross-check

`codes-correct = 116`, `codes-fail = 0` (set equality of the expected and
actual `TYPE-*` code sets). Actual code distribution over the corpus:

| Code | Reject cases | Coverage |
| :--- | ---: | :--- |
| `TYPE-001` format | 39 | 38 distinct format pairs (including the transform edge's BED→GTF) + an `[[include]]`-contributed edge |
| `TYPE-002` sort order | 9 | SAM/CRAM queryname/coordinate/unsorted pairs + an include-contributed sort edge |
| `TYPE-003` index | 7 | missing index and bai/csi/tbi/crai kind boundaries |
| `TYPE-004` reference build | 11 | GRCm38/GRCm39, dm6/dm3, T2T-CHM13/GRCh38, GRCh37/GRCm38, GRCh38/GRCm39, GRCm38/dm6, T2T/GRCm39 |
| `TYPE-005` read groups | 3 | missing `@RG` against two- and four-field requirements |
| `TYPE-006` compression | 9 | plain/gzip/zstd vs bgzip, bgzip vs zstd, BAM vs zstd, CRAM vs zstd |
| `TYPE-007` attributes | 5 | `gvcf` alias, `library_type`, `strandedness`, missing `variant_type`, BAM `assay` |
| `TYPE-008` advisory | 1 accept | typed VCF consumer fed by an untyped producer |
| `TYPE-009` advisory | 1 accept | GTF producer omits build, consumer names GRCh38 |
| `TYPE-010` advisory | 1 accept | three-channel producer, no matching consumer channel |

Coverage: every `BioFormat` enum variant except the `generic` sentinel is
driven; every blocking code has at least one expected-reject case; every
advisory code has an expected-accept control; a `transform.*`
scatter/combine GENERATED typed edge and an `[[include]]`-contributed typed
edge are present in both verdict directions where an accept-side contrast
exists; 7 untyped assay pipelines and 9 real gallery examples are included.

### 5.3 Per-case evidence

The machine-readable per-case rows (expected/actual verdict, expected and
actual code sets, verdict/code correctness, note) are in
`docs/superpowers/audits/data/biotype-truthset-governed-results.json`
(SHA-256 `d7e38fed85c37806467ca63f4e447b46302b612147b4219fcf2739a7c5858de4`).
The analysis output, including the filled statement, is in
`docs/superpowers/audits/data/biotype-truthset-governed-decision.json`
(SHA-256 `5e0462d3fb597a9a3829baa9fb3d422bfc1030589b08aa4bc88301274214bd32`).

## 6. Exact interval and governed decision

Primary endpoint: `FA / (FA + TR)` over expected-reject cases.
Direction: lower is better. Method: exact two-sided Clopper–Pearson 95%,
computed by the sealed analysis script (binomial CDF by deterministic
bisection; verified against the pilot report's known bounds, e.g.
`Beta(0.975;1,22) = 0.1544`).

| Quantity | Value |
| :--- | :--- |
| False accepts / expected-reject | `0 / 83` |
| Point estimate | `0.0000%` |
| Exact 95% CI | `[0.0000%, 4.3471%]` |
| Frozen `τ_FA` | `0.05` |
| Frozen `N_min` | `73` |
| `N` (primary) | `83 ≥ 73` |
| Non-verdicts | `0` |
| **Governed category (PR-06 §C1)** | **POSITIVE** |

The exact upper bound is `1 − 0.025^(1/83) = 0.043471…`, i.e. `4.3471%`,
comfortably at or below `τ_FA = 0.05`; the lower bound is `0`.

### 6.1 Filled positive statement (PR-06 §C1 template)

> On the frozen held-out truth set of 83 expected-reject cases, the
> BioTypes checker false-accepted 0 of 83 expected-reject cases (0.0000%,
> exact 95% CI [0.0000%, 4.3471%]), within the pre-registered bound
> `τ_FA = 0.05`. This validates the checker on this set; it does not
> establish biological truth or coverage outside the listed dimensions.

### 6.2 Flow table

| Stage | N | Note |
| :--- | ---: | :--- |
| Authored and sealed | 116 | 83 expected-reject + 33 expected-accept controls |
| Pre-run exclusions | 0 | no case was removed before the run |
| Non-verdicts | 0 | every case produced a parsable `validate --json` verdict |
| Analyzed (primary denominator) | 83 | expected-reject cases |
| Acceptance controls (reported descriptively) | 33 | not in the primary denominator |
| Pilot set (separate, exploratory) | 54 | excluded as exposure; not pooled |

## 7. Determinism and custody

The campaign was run twice against the same binary and sealed cases. The
per-case rows, the confusion matrix, and the code tally are byte-identical
(evidence:
`docs/superpowers/audits/data/biotype-truthset-governed-repeat-check.txt`).
PR-01 §C1 defines repeated execution as a consistency check: any
disagreement would be an infrastructure failure and would void the case;
none occurred.

The analysis script recomputes the interval from the committed per-case
JSON, so the decision can be reproduced without rerunning the checker:
`python3 scripts/biotype-truthset-governed-analysis.py <results.json>`.

## 8. The pilot is exploratory, not confirmatory

The 54-case operational set (2026-11-20/22) was measured while `τ_FA` was
UNDECIDED: 32 true-accept / 0 false-accept / 22 true-reject / 0
false-reject, codes 54/54, exact 95% upper bound on the false-accept rate
`1 − 0.025^(1/22) = 15.4%`. Under PR-06 §1 rule 2 it cannot decide a
governed category, cannot set the threshold, and is not pooled with the
held-out corpus. It remains a valid **operational** sanity result and is
reported as such in `biotype-truth-set-2026-11-20.md`; its cases are not
reused here.

The amendment-2 interpretation, recorded because the process needed one:
the C1 pilot firewall mirrors C2's. The exposure of the pilot makes the
pilot non-confirmatory; it does not retroactively invalidate a held-out
dataset whose outcomes did not exist when the threshold was frozen. The
amendment entry records the exposure state explicitly.

### 8.1 Pilot-disjointness audit and sensitivity

Amendment 2 excludes a confirmatory case that re-asserts a contract
boundary already asserted by the pilot. The rule is applied at the
**coverage-cell** level: a case's cell is its format, violated dimension,
value pair, edge direction, construct class (plain / `transform` /
`[[include]]`), and the additional contract dimensions it declares. This
follows PR-02 §0.1 item 4 ("the same thing in the same coverage cell") and
the pilot's own treatment of edge direction as a distinct unit (`a06`
queryname→coordinate and `b14` coordinate→queryname are separate pilot
cases on the same BAM sort pair). Format
is a substantive dimension, not incidental context: the checker's
semantics are format-dependent (BAM implies BGZF while CRAM does not), so
a sort- or compression-rule case on a new format is new evidence about the
checker rather than a copy of the pilot's BAM/VCF case.

A post-run audit under this interpretation flags:

| Case | Audit finding | Disposition |
| :--- | :--- | :--- |
| `g049` | re-asserts the pilot `t04` VCF index cell (none → tbi) with a different (implicit-none) compression context | retained; sensitivity below |
| `g052` | re-asserts the pilot `t04` VCF index cell (none → tbi) with matching bgzip compression | retained; sensitivity below |
| `g048` | reverses `b07`'s BAM index pair (csi → bai); direction is a distinct unit under the pilot's `a06`/`b14` precedent | distinct |
| `g066` | adds sort/index/build dimensions to the pilot `a07` read-group cell | distinct |
| `g067`, `g072`–`g074` | vary the format cell of a pilot compression value pair | distinct |
| `g038`–`g041`, `g081`–`g083` | vary format or construct (`transform`/`[[include]]`) | distinct |

PR-02 §0.2 forbids post-outcome exclusion, so these cases remain in the
primary analysis set; the audit is disclosed rather than acted on. The
descriptive sensitivity (the sealed interval function on filtered
denominators) is:

| Exclusions | N | FA | exact 95% upper bound | category |
| :--- | ---: | ---: | ---: | :--- |
| none (as sealed) | 83 | 0 | 4.3471% | POSITIVE |
| `g049`, `g052` | 81 | 0 | 4.4520% | POSITIVE |
| `g049`, `g052`, `g066` | 80 | 0 | 4.5064% | POSITIVE |
| `g049`, `g052`, `g048`, `g066` | 79 | 0 | 4.5621% | POSITIVE |

Evidence: `data/biotype-truthset-governed-sensitivity.json`. The category
is unchanged under every exclusion, so this disclosed authoring
imperfection does not affect the governed decision.

**Interpretation judgment.** A stricter reading that ignores the format,
construct, and direction dimensions (treating, for example, a SAM
sort-order case as a duplicate of the pilot's BAM sort-order case) would
exclude most of the corpus and leave well under 73 independent boundary
cells, making the campaign UNDERPOWERED. That reading is recorded and not
adopted, for the reasons above. An independent reviewer who adopts it
would reach UNDERPOWERED; that disagreement is exactly what PR-05 review
must settle, and it is one reason the result is not publication-ready.

## 9. Defects surfaced

None. Every expected verdict and every expected code set matched. No
checker defect, documentation defect, or harness defect was surfaced by
this campaign, so no separate fix or regression test was required. The
one defect surfaced by the earlier pilot (`b18`, a documentation
overstatement about CRAM/BGZF) was already corrected in the pilot wave
and is unaffected here.

## 10. Raw-data lineage and reproduction

- Cases: `tests/truthset/biotype/governed/g001…g107_*.oxoflow` (107 case
  bodies, each with its pre-registered EXPECT header) plus three include
  fixtures (`incmod_bam_producer`, `incmod_queryname_producer`,
  `incmod_gtf_grch38`, no EXPECT rows of their own), and the nine real
  gallery examples listed in §5.1.
- Sealed expectations: `docs/superpowers/preregistration/prerun/C1-governed-2026-11-27.json`
  (`cases` array; SHA-256 `cab5cac4…2ff3`).
- Runner: `scripts/biotype-truthset-governed.py`; analysis:
  `scripts/biotype-truthset-governed-analysis.py`.
- Results: `docs/superpowers/audits/data/biotype-truthset-governed-results.json`
  (`commit` = `6fbf38c1`, the tree the run executed on; `engine_commit` =
  `20ad630b`, the tree the binary was built from).
- Decision: `docs/superpowers/audits/data/biotype-truthset-governed-decision.json`.
- Determinism evidence:
  `docs/superpowers/audits/data/biotype-truthset-governed-repeat-check.txt`.
- Independence sensitivity:
  `docs/superpowers/audits/data/biotype-truthset-governed-sensitivity.json`
  (descriptive, not the primary analysis).

## 11. Honesty boundary — what this does NOT measure

- **Static validation, not execution.** No real data was staged and no
  bioinformatics tool was run. The result is about the checker's
  accept/reject logic on declared contracts, not about pipeline outputs.
- **Bounded claim.** POSITIVE on this frozen corpus and these contract
  dimensions only. It is not a claim about biological truth, about
  workflows outside the listed dimensions, or about runtime behaviour.
- **Independent reanalysis PENDING.** PR-01 §0 rule 5 requires an
  independent analyst to reproduce the primary endpoint from the raw
  per-case data and the sealed script. That record does not yet exist, so
  the result is a governed campaign result, not a publication-ready
  package.
- **PR-05 sign-off PENDING.** The independent reviewer record for #1163 is
  still incomplete. Task 4.0 and ledger Gate 3 remain unpassed; Gate 3
  additionally requires independent biological validation, matched
  same-host comparisons, and reproduction on an independent environment.
- **Single arm** (macOS arm64, rustc 1.98.0). No cross-platform or
  cross-version contrast is claimed.
- **Author independence.** The corpus was authored by the maintainer's
  delegated agent from the published contract without reading the checker
  source. The maintainer is also the campaign lead; the independent
  reviewer identity remains PENDING in PR-05, which is exactly the
  governance gap above.
- **Pilot exposure disclosed** (§8). The 54-case pilot remains exploratory
  and is never pooled with this corpus.
- **Independence interpretation disclosed** (§8.1). The POSITIVE category
  rests on the coverage-cell interpretation of pilot-disjointness; a
  stricter dimension-only reading would make the campaign UNDERPOWERED.
  Two cases arguably re-assert a pilot index cell and were not excluded
  before the run; the category is unchanged when they (and every other
  plausible exclusion) are removed, but an independent reviewer must
  settle the interpretation.

## 12. Documentation changes accompanying this report

- `README.md` §Research program / BioTypes: the governed POSITIVE result
  is added beside the pilot's operational result; the pilot stays labelled
  exploratory and the bounded claim is stated.
- `ROADMAP.md` (BioTypes maturity row): the truth-set exit criterion moves
  from UNMET to met for this bounded, governed corpus; Gate 3 and
  publication readiness remain open.
- `docs/guide/src/vision.md`: "broad truth-set validation outstanding" is
  replaced with the governed result and its boundary.
- `docs/superpowers/audits/industrial-scientific-audit-ledger.md`:
  WF-04 and §7 wave 7 record the campaign, its commit bindings, and the
  remaining independent-reanalysis/sign-off gaps.
- `docs/superpowers/preregistration/README.md` §3 and `AMENDMENTS.md`
  entry 3: the #1163 confirmatory-data status is updated with an explicit
  "no rule changed" post-result status entry.

## 13. References

- `docs/superpowers/preregistration/PR-06-publication-decision-rule.md`
  (v1.0.2, amendment 2) §C1 — `τ_FA`, `N_min`, the C1 pilot firewall, and
  the decision rule.
- `docs/superpowers/preregistration/PR-01-outcomes-and-statistical-models.md`
  (v1.0.2) §C1 and §0 rule 4 — endpoints, direction, exact interval.
- `docs/superpowers/preregistration/PR-02-case-rules.md` (v1.0.2) §C1 —
  inclusion/exclusion, expansion protocol, zero non-verdict limit.
- `docs/superpowers/preregistration/PR-03-fixed-manifests.md` (v1.0.2)
  §C1 — host, budget, dataset, and the pre-run manifest requirement.
- `docs/superpowers/preregistration/PR-04-leakage-and-custody.md` (v1.0.2)
  §C1 — custody record and seal.
- `docs/superpowers/preregistration/AMENDMENTS.md` entries 2–3.
- `docs/superpowers/audits/biotype-truth-set-2026-11-20.md` — the
  exploratory 54-case operational report.
- `docs/guide/src/reference/workflow-format.md` — Biological Type Contracts.
- `docs/guide/src/reference/diagnostics-engine.md` — TYPE-001–010 catalogue.
