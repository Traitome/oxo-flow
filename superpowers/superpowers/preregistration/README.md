# Pre-Registration and Evidence Governance — Task 4.0

**Status: artifacts frozen at version 1.0.4 (amendments 1–10; amendment 6 is
the #1166 parameter freeze); independent sign-off PENDING. #1163 and #1166
have collected governed confirmatory data (under amendments 2 and 6
respectively; #1163's erratum/PR-04 caveat are amendments 4–5); #1164 and
#1165 have collected no confirmatory data under this protocol.** Task 4.0 is
not passed, and ledger Gate 3 remains unpassed. Artifact creation is not gate
passage; neither governed result is publication-ready until PR-05 sign-off.

This directory is the governance layer required by
[`docs/superpowers/plans/industrial-scientific-implementation-plan.md`](../plans/industrial-scientific-implementation-plan.md)
Task 4.0 ("Freeze the confirmatory analysis before examining confirmatory
outcomes"). It covers the four empirical campaigns filed in the same wave:

| Campaign | Issue | Primary endpoint | Confirmatory data status at freeze |
| :--- | :--- | :--- | :--- |
| BioTypes broad truth-set validation | [#1163](https://github.com/Traitome/oxo-flow/issues/1163) | False-accept proportion on expected-reject cases | None collected |
| Applied-repair verified by successful reruns | [#1164](https://github.com/Traitome/oxo-flow/issues/1164) | Full-loop verified-repair proportion, recovery-eligible cases | **Pilot already observed (3/10 applied, 3/10 verified, 3/3 recovery-eligible)** — pilot only, not confirmatory |
| Corpus-wide Nextflow/Snakemake semantic equivalence | [#1165](https://github.com/Traitome/oxo-flow/issues/1165) | Strict declared-output equivalence proportion | None collected |
| Cross-architecture reproduction measurement | [#1166](https://github.com/Traitome/oxo-flow/issues/1166) | Architecture-stable artifact proportion | None collected |

The governance issue is [#1167](https://github.com/Traitome/oxo-flow/issues/1167).

## 1. Artifacts

Each artifact covers all four campaigns and maps one-to-one to a Task 4.0
required artifact.

| ID | Artifact | Task 4.0 requirement |
| :--- | :--- | :--- |
| PR-01 | [Outcomes and statistical models](PR-01-outcomes-and-statistical-models.md) | Primary and secondary outcomes, direction, and statistical models |
| PR-02 | [Case rules](PR-02-case-rules.md) | Inclusion, exclusion, stopping, outlier, and missing-data rules |
| PR-03 | [Fixed manifests](PR-03-fixed-manifests.md) | Model, prompt, tool, seed, hardware, dataset, and budget manifests |
| PR-04 | [Leakage and custody](PR-04-leakage-and-custody.md) | Leakage audit and held-out-intent custody |
| PR-05 | [Reviewer sign-off](PR-05-reviewer-signoff.md) | Independent reviewer sign-off (PENDING; not fabricated) |
| PR-06 | [Publication decision rule](PR-06-publication-decision-rule.md) | A decision rule that permits a negative result |

The amendment log is [`AMENDMENTS.md`](AMENDMENTS.md). The content binding
is [`MANIFEST.sha256`](MANIFEST.sha256). The manifest hashes the six
frozen artifacts (PR-01 through PR-06) only; this README and the amendment
log are governance text and are deliberately not hashed, because they are
expected to change when an amendment is recorded.

## 2. Freeze protocol

### 2.1 What "frozen" means

A frozen artifact is a versioned document whose SHA-256 is recorded in
`MANIFEST.sha256` and whose version and freeze date are in its header.
The hash is the pre-outcome proof: an artifact whose current content still
matches the manifest is the text that existed before the campaign ran. A
commit alone is not sufficient, because a later commit can rewrite the
file; the hash record plus the amendment log is.

### 2.2 Freeze sequence

1. **Draft without outcomes.** The six artifacts are written from the
   campaign issues, the implementation plan, and the existing harnesses
   (`eval/frontier/`, `eval/gold/`, `benches/comparative/`). No campaign
   result is examined while drafting.
2. **Fill every value or mark it open.** A value that the campaign issues
   and plan define is written concretely. A value they do not define is
   marked `UNDECIDED` or `TBD` with an owner and the deadline "before data
   collection". It is never silently omitted.
3. **Hash and commit.** Compute SHA-256 for PR-01 through PR-06, write
   `MANIFEST.sha256`, and commit all files together. The commit that adds
   these files is the freeze commit; its identity is recorded in the audit
   ledger.
4. **Verify.** From this directory, run
   `shasum -a 256 -c MANIFEST.sha256`. Every line must report `OK`.
5. **Sign-off is separate.** Freezing the text does not approve it. PR-05
   stays PENDING until an independent reviewer completes the records.

### 2.3 Amendment protocol (post-freeze changes)

Any change after the freeze commit requires a dated entry in
`AMENDMENTS.md` **before or with** the change. The entry records:

| Field | Meaning |
| :--- | :--- |
| Date | Calendar date of the amendment |
| Artifact / section | Exact document and section |
| Change | Old value or text → new value or text |
| Reason | Why the frozen text cannot stand |
| Outcome exposure | Whether any confirmatory outcome was examined before the change |
| Effect on status | Confirmatory / exploratory / invalid for the affected endpoint |
| Approver | Named role that approved the amendment |

Rules:

1. A pre-data amendment may fill a TBD/UNDECIDED value or correct a
   protocol defect. It keeps the campaign confirmatory only if no
   confirmatory outcome was seen.
2. A post-outcome amendment makes the affected endpoint **exploratory**.
   It must be reported as such; it cannot be presented as pre-registered.
3. An amendment that changes a primary endpoint, its direction, its
   denominator, or its threshold after an outcome voids the confirmatory
   claim for that campaign.
4. Every amendment re-computes the manifest. The previous manifest is
   retained in git history and the amendment entry names the old and new
   hashes.
5. Signatures are added through an amendment entry. A signature edits
   PR-05 itself, so it changes that artifact's hash: the manifest **must**
   be recomputed and committed with the amendment entry, and the previous
   manifest remains visible in git history and in the entry. The entry
   names the **pre-signature** manifest hash that the reviewer actually
   signed; the recomputed manifest records the signed artifact. This is
   the only permitted post-freeze edit that does not change analysis
   content, and it still follows the recompute rule in item 4.

### 2.4 Timestamps and prose dates

Task 4.0 requires the freeze to be provable, so the header freeze record
and the amendment log are dated. This is the one intentional exception to
the implementation plan's rule that new plan/protocol prose avoid calendar
dates; the dates exist in the governance record, not as narrative claims.

## 3. Honest status ledger

This section is the authoritative statement of what exists and what does
not. It must be updated only by amendment.

| Item | Status |
| :--- | :--- |
| PR-01 through PR-06 written | Yes, version 1.0.4 (amendments 1–10), frozen; PR-05 remains 1.0.1 with sign-off PENDING |
| Manifest hashes recorded | Yes; verify with `shasum -a 256 -c MANIFEST.sha256`. Current manifest `7e3984771249522314d360a617132609c741daae96b8d11cd1f2e8615121bbfe`; the v1.0.0–v1.0.3 and pre-merge #1166 branch manifest hashes are retained in `AMENDMENTS.md` §2.1. |
| Independent reviewer sign-off (#1163) | PENDING |
| Independent reviewer sign-off (#1164) | PENDING |
| Independent reviewer sign-off (#1165) | PENDING |
| Independent reviewer sign-off (#1166) | PENDING |
| #1163 confirmatory data | **Collected 2026-11-27 under amendment 2; erratum 2026-11-28 (amendments 4–5).** Sealed 116-case corpus: 83 expected-reject + 24 post-freeze authored controls + 9 pre-existing gallery controls whose accept outcome was already known (disclosed exposure). Result: 0/83 false accepts, exact 95% upper bound 4.3471% ≤ `τ_FA = 0.05`, POSITIVE ([report](../audits/biotype-truth-set-governed-2026-11-27.md); [erratum](../audits/biotype-truthset-governed-erratum-2026-11-28.md)). The ≥30 control floor is met by the 33 sealed controls but not by the held-out authored subset (24); PR-05 adjudicates. A separately sealed post-hoc supplement adds acceptance controls, including the missing `TYPE-005` contrast (descriptive only; not in the FA denominator). Independent reanalysis and PR-05 sign-off remain PENDING; the earlier 54-case set is exploratory/exposure. |
| #1164 confirmatory data | None; a ten-case pilot was measured on 2026-11-18 **before this freeze** (3/10 applied, 3/10 verified, 3/3 recovery-eligible). It is pilot evidence and cannot be cited as pre-registered or confirmatory. A held-out re-run is required. |
| #1165 confirmatory data | None collected |
| #1166 confirmatory data | One governed campaign measured 2026-11-28: 24/24 declared artifacts SHA-256 identical (100%, exact 95% CI [85.75%, 100%]), all 8 classes architecture-stable, the `uname -m` negative control differed (`aarch64` vs `x86_64`), category POSITIVE under the frozen rule. PR-05 C4 sign-off remains PENDING. Evidence: [`cross-arch-provenance-governed-2026-11-27.md`](../audits/cross-arch-provenance-governed-2026-11-27.md). |
| Task 4.0 gate | **Not passed** — PR-05 sign-off is outstanding; #1163 and #1166 have their pre-run manifests, case seals, and analysis-script hashes, but the #1164/#1165 pre-run manifests/case seals and all reviewer signatures remain outstanding. #1163 also has a post-result control-provenance erratum and a separately sealed post-hoc supplement. |
| Ledger Gate 3 (Scientific Validity) | **Not passed** |

This directory was frozen before any campaign ran and claims no empirical
result of its own. The #1163 governed campaign result, its control-provenance
erratum, and its post-hoc supplement are recorded in the status table above
and in the linked report/erratum (2026-11-28).

## 4. Cross-references

- Implementation plan Task 4.0:
  [`../plans/industrial-scientific-implementation-plan.md`](../plans/industrial-scientific-implementation-plan.md)
- Audit ledger §6 Gate 3 and §7 wave 7:
  [`../audits/industrial-scientific-audit-ledger.md`](../audits/industrial-scientific-audit-ledger.md)
- Research-program claims and evidence boundaries:
  [`README.md`](../../../README.md) and [`ROADMAP.md`](../../../ROADMAP.md)
  Phase 12
- Evaluation harnesses:
  [`eval/frontier/README.md`](../../../eval/frontier/README.md),
  [`eval/frontier/PILOT_FINDINGS.md`](../../../eval/frontier/PILOT_FINDINGS.md),
  [`eval/README.md`](../../../eval/README.md),
  [`benches/comparative/README.md`](../../../benches/comparative/README.md)

## 5. What this layer does not do

- It does not make a campaign pre-registered until the pre-run manifest
  and case seals exist and PR-05 is signed.
- It does not turn a pilot into confirmatory evidence (#1164).
- It does not pass Gate 3, which additionally requires matched three-arm
  ablation data, direct same-host Nextflow/Snakemake comparisons,
  independent biological validation, false-positive/negative analysis for
  every static contract, and reproduction on an independent environment.
- It does not permit a negative result to be described as an incomplete
  deliverable. PR-06 defines the negative branch as a publishable result.
