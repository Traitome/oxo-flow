# PR-05 — Independent Reviewer Sign-Off

| Freeze record | Value |
| :--- | :--- |
| Artifact | PR-05 (Task 4.0 required artifact 5 of 6) |
| Version | 1.0.1 |
| Freeze status | FROZEN (sign-off fields deliberately PENDING) |
| Freeze date | 2026-11-18 (original freeze; amendment 1 same date) |
| Amendment | 1 (2026-11-18): review fixes D1–D5 and clarifications — see [`AMENDMENTS.md`](AMENDMENTS.md) entry 1 |
| Content binding | SHA-256 recorded in [`MANIFEST.sha256`](MANIFEST.sha256) |
| Campaigns covered | #1163, #1164, #1165, #1166 |
| Change control | Post-freeze edits only through [`AMENDMENTS.md`](AMENDMENTS.md) |

**Status: NO SIGN-OFF HAS BEEN RECORDED.** Every reviewer field below is a
required field, not a completed one. The value `PENDING` means the field
is empty; it is not a placeholder for a name that was agreed informally.
No campaign may collect confirmatory data, and no result may be described
as pre-registered, until the corresponding row is completed and recorded
under the amendment procedure in §4.

## 1. Required reviewer qualifications

The independent reviewer must satisfy all of the following, and each must
be declared in the sign-off record:

1. **Independence.** Not an author of the engine change, checker,
   importer, or analysis script under test; not the campaign lead; no
   financial or authorship interest in a positive result.
2. **Competence.** Able to assess the statistical model, the case rules,
   and the leakage/custody protocol. A second reviewer with domain
   expertise (bioinformatics or workflow-engine semantics) is required for
   #1163 and #1165; a second reviewer with reproducibility or systems
   expertise is required for #1166. The same person may satisfy both roles
   only if the dual competence is declared.
3. **Access.** Has seen the raw protocol, the pre-run manifest schema, and
   the analysis script, and has had the opportunity to request changes
   before freeze.
4. **No outcome access.** Has not seen a confirmatory outcome before
   signing. If the reviewer has seen one (for example #1164's pilot), the
   sign-off may still be given, but it must be labeled
   `POST-OUTCOME REVIEW` and the campaign cannot be confirmatory without a
   held-out re-run.

## 2. Review checklist

The reviewer completes every item and records the evidence path. A single
`REJECT` blocks the campaign; `APPROVE WITH CONDITIONS` blocks until the
conditions are recorded as a pre-data amendment.

| # | Check | Evidence expected |
| :--- | :--- | :--- |
| 1 | PR-01 names exactly one primary endpoint per campaign, with direction | Artifact section reference |
| 2 | Every secondary endpoint is marked secondary and cannot be promoted | PR-01 §0 |
| 3 | Statistical models are named, including CI method, paired tests, and multiplicity family | PR-01 §0, campaign sections |
| 4 | Thresholds are either fixed or visibly `UNDECIDED` with owner and pre-data deadline | PR-01 §5, PR-06 |
| 5 | Inclusion, exclusion, stopping, outlier, and missing-data rules are complete per campaign | PR-02 |
| 6 | Every undefined number is marked `UNDECIDED` or `TBD` with owner and deadline; none is silently omitted | PR-02, PR-03 |
| 7 | Model, prompt, tool, seed, hardware, dataset, and budget dimensions are all present per campaign | PR-03 |
| 8 | Leakage vectors L1–L7 are addressed and custody roles are assigned | PR-04 |
| 9 | Sealing and breach-handling sequences are executable as written | PR-04 §2.2–2.4 |
| 10 | The publication rule contains an explicit negative branch per campaign and forbids post-hoc threshold changes | PR-06 |
| 11 | #1164's already-observed pilot is disclosed and excluded from confirmatory claims | PR-01 §C2, PR-04 §C2 |
| 12 | No artifact claims a result, a pass, or a gate completion | All artifacts |
| 13 | The manifest hashes verify (`shasum -a 256 -c MANIFEST.sha256`) | Command output pasted into the record |

## 3. Sign-off records

Each campaign needs its own completed row. All fields are mandatory.

### C1 — #1163

| Field | Value |
| :--- | :--- |
| Campaign | #1163 — BioTypes broad truth-set validation |
| Artifacts reviewed (version) | PR-01 1.0.1, PR-02 1.0.1, PR-03 1.0.1, PR-04 1.0.1, PR-06 1.0.1 |
| Manifest hash reviewed | PENDING |
| Reviewer role | Independent scientific reviewer — BioTypes/bioinformatics |
| Reviewer name | PENDING |
| Affiliation / independence declaration | PENDING |
| Conflicts of interest | PENDING |
| Outcome access at review | PENDING (NONE / PILOT SEEN / CONFIRMATORY SEEN) |
| Verdict | PENDING (APPROVE / APPROVE WITH CONDITIONS / REJECT) |
| Conditions | PENDING |
| Date | PENDING |
| Signature or cryptographic attestation | PENDING |

### C2 — #1164

| Field | Value |
| :--- | :--- |
| Campaign | #1164 — applied-repair verified by successful reruns |
| Artifacts reviewed (version) | PR-01 1.0.1, PR-02 1.0.1, PR-03 1.0.1, PR-04 1.0.1, PR-06 1.0.1 |
| Manifest hash reviewed | PENDING |
| Reviewer role | Independent scientific reviewer — AI repair / statistics |
| Reviewer name | PENDING |
| Affiliation / independence declaration | PENDING |
| Conflicts of interest | PENDING |
| Outcome access at review | PENDING (pilot outcome is already public and must be declared) |
| Verdict | PENDING |
| Conditions | PENDING |
| Date | PENDING |
| Signature or cryptographic attestation | PENDING |

### C3 — #1165

| Field | Value |
| :--- | :--- |
| Campaign | #1165 — corpus-wide Nextflow/Snakemake semantic equivalence |
| Artifacts reviewed (version) | PR-01 1.0.1, PR-02 1.0.1, PR-03 1.0.1, PR-04 1.0.1, PR-06 1.0.1 |
| Manifest hash reviewed | PENDING |
| Reviewer role | Independent scientific reviewer — workflow engines |
| Reviewer name | PENDING |
| Affiliation / independence declaration | PENDING |
| Conflicts of interest | PENDING |
| Outcome access at review | PENDING (NONE / PILOT SEEN / CONFIRMATORY SEEN) |
| Verdict | PENDING |
| Conditions | PENDING |
| Date | PENDING |
| Signature or cryptographic attestation | PENDING |

### C4 — #1166

| Field | Value |
| :--- | :--- |
| Campaign | #1166 — cross-architecture reproduction measurement |
| Artifacts reviewed (version) | PR-01 1.0.1, PR-02 1.0.1, PR-03 1.0.1, PR-04 1.0.1, PR-06 1.0.1 |
| Manifest hash reviewed | PENDING |
| Reviewer role | Independent scientific reviewer — reproducibility / systems |
| Reviewer name | PENDING |
| Affiliation / independence declaration | PENDING |
| Conflicts of interest | PENDING |
| Outcome access at review | PENDING (NONE / PILOT SEEN / CONFIRMATORY SEEN) |
| Verdict | PENDING |
| Conditions | PENDING |
| Date | PENDING |
| Signature or cryptographic attestation | PENDING |

## 4. How a sign-off is recorded

1. The reviewer writes the completed record into a new version of this
   artifact **and** a dated entry in `AMENDMENTS.md` naming the manifest
   hash that was reviewed.
2. The sign-off entry includes the reviewer's name, role, independence
   declaration, outcome-access declaration, verdict, conditions, date, and
   an attestation token (a detached signature over the manifest hash, or a
   named written attestation in the review thread, recorded by URL).
3. A sign-off is valid only for the manifest hash it names. Any change to
   PR-01 through PR-04 or PR-06 invalidates it until the reviewer confirms
   that the change does not affect the signed scope.
4. A conditional approval lists each condition as a numbered item and
   names the artifact section that must change. Conditions are closed by a
   pre-data amendment; the campaign stays blocked until then.
5. Signing after seeing a confirmatory outcome is permitted only as
   `POST-OUTCOME REVIEW`, and the campaign is exploratory, not
   confirmatory.

## 5. Gate effect

Completion of all four rows is necessary but not sufficient for Task 4.0:
the pre-run manifests, case seals, and analysis-script hashes must also
exist. Until then, Task 4.0 remains **Planned / blocked** and ledger
Gate 3 remains unpassed. Artifact creation is not gate passage.
