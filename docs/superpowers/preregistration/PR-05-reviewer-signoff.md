# PR-05 — Independent Reviewer Sign-Off

| Freeze record | Value |
| :--- | :--- |
| Artifact | PR-05 (Task 4.0 required artifact 5 of 6) |
| Version | 1.1.0 |
| Freeze status | FROZEN (sign-off fields deliberately PENDING) |
| Freeze date | 2026-11-18 (original freeze; amendment 1 same date) |
| Amendment | 1 (2026-11-18): review fixes D1–D5 and clarifications — see [`AMENDMENTS.md`](AMENDMENTS.md) entry 1; 11 (2026-11-28): pre-sign-off data status, claim vocabulary, and sign-off adjudication — see [`AMENDMENTS.md`](AMENDMENTS.md) entry 11 |
| Content binding | SHA-256 recorded in [`MANIFEST.sha256`](MANIFEST.sha256) |
| Campaigns covered | #1163, #1164, #1165, #1166 (the four PR-06 campaigns; the AI-synthesis ablation #1395/#1412 is not a PR-06 campaign and is out of scope, see §2.4) |
| Change control | Post-freeze edits only through [`AMENDMENTS.md`](AMENDMENTS.md) |

**Status: NO SIGN-OFF HAS BEEN RECORDED.** Every reviewer field below is a
required field, not a completed one. The value `PENDING` means the field
is empty; it is not a placeholder for a name that was agreed informally.
No campaign may present data as confirmatory, and no result may be
described as a pre-registered result (unqualified) or used for a
publication claim, until the corresponding row is completed with an
approving disposition and recorded under §5.

Data collected under a frozen pre-registration **before** the corresponding
row records an approving disposition is **not** confirmatory and does not
become confirmatory automatically. §2 defines that status — "governed
pre-sign-off data collection" / "governed pre-sign-off measurement" — its
claim vocabulary, and how a completed sign-off disposition adjudicates it.
The no-publication-before-sign-off rule is unchanged: pre-sign-off status
is a disclosure category, never a publication authorization.

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
4. **Outcome access.** Declares exactly one of `NONE`, `PILOT SEEN`,
   `PRE-SIGN-OFF DATA SEEN`, or `CONFIRMATORY SEEN`, and names what was
   seen. Seeing this campaign's **pre-sign-off data** does not disqualify
   the reviewer and does not by itself make the data exploratory: the
   reviewer adjudicates whether the freeze-before-data chain holds (§2.3).
   That declaration replaces the pre-data no-access condition, which
   cannot be met once pre-sign-off data exist. Seeing a pre-freeze pilot
   (for example #1164's) is declared; a pilot cannot be promoted and a
   held-out re-run is required. Seeing a previously signed campaign's
   confirmatory result is declared and does not affect this campaign's
   sign-off.

## 2. Data status: pre-sign-off measurement vs confirmatory result

This section was added by amendment 11 (2026-11-28) to resolve a governance
contradiction. The original text forbade describing any result as
confirmatory before sign-off, but the frozen protocol had no name for data
that are collected *validly under the frozen pre-registration* before
sign-off. Naming that status does not promote those data and does not
relax the publication gate.

### 2.1 Definitions

**Governed pre-sign-off data collection.** Data collected for one of the
four campaigns after the protocol freeze, under the frozen PR-01–PR-06
text, with all of:

1. every campaign parameter closed by a pre-data amendment;
2. a pre-run manifest and case/analysis seal committed before the first
   execution (or, where the campaign defines a different seal, that
   campaign's frozen seal);
3. the analysis script sealed before the run, used unmodified for the
   reported analysis; a disclosed post-outcome hardening that reproduces
   the same reported category on the same endpoint, denominator, and
   interval (as recorded for #1163 and #1166) does not invalidate the
   collection;
4. no PR-05 row disposing of the campaign as promoted (an `APPROVE`
   disposition, or `APPROVE WITH CONDITIONS` after every condition is
   closed and recorded, §2.3).

Its reported status is **governed pre-sign-off measurement**. It is not a
pilot and not exploratory merely because it precedes sign-off; it is data
collected under the frozen protocol whose compliance and governance
disposition are decided at sign-off. A row that invalidates the data
(§2.3) removes it from both categories: it is reported only as
invalidated/exploratory.

Data that do not meet all four conditions are not pre-sign-off data: a
pilot measured before the freeze (#1164), a set measured while a threshold
was `UNDECIDED` (the #1163 54-case set), and a post-hoc supplement
authored after the primary outcome are pilot/exploratory/exposure and
keep that label.

**Confirmatory result.** A result becomes confirmatory only when the
corresponding PR-05 sign-off row is completed **with an `APPROVE`
disposition** — or with `APPROVE WITH CONDITIONS` after every condition
has been closed and recorded under §5 — and only for the manifest hash
that sign-off names. A row completed with `REJECT`, `FREEZE CHAIN NOT
DEMONSTRABLE`, or open conditions does not create a confirmatory result.
The approving completion changes:

1. **The row.** Every field in §4 is filled: reviewer identity, role,
   independence declaration, competence declaration, outcome-access
   declaration, verdict, conditions, date, and signature/attestation.
2. **The outcome-access declaration.** A reviewer of #1163 or #1166 will
   necessarily have seen the pre-sign-off measurement; `PRE-SIGN-OFF DATA
   SEEN` is declared and is not by itself a disqualification (§2.3).
3. **The adjudication.** The verdict classifies the pre-sign-off data as
   *promoted*, *conditionally held*, or *invalidated* (§2.3).
4. **The manifest rebinding.** The amendment entry names the
   pre-signature manifest hash that was reviewed and signed; the
   recomputed manifest records the signed PR-05. A sign-off is valid only
   for the named manifest hash. Any change to PR-01–PR-04 or PR-06
   invalidates it until the reviewer confirms that the change does not
   affect the signed scope; a scope-changing change makes the affected
   data pre-sign-off again.
5. **The relabeling.** The affected report, status ledger, and
   user-facing summaries replace "governed pre-sign-off measurement" with
   "confirmatory result (PR-05-signed <date>, manifest <hash>)" and
   record the sign-off amendment. Result values and the sealed analysis
   are not recomputed: promotion changes governance status, not data.

**No automatic promotion.** Pre-sign-off data remain pre-sign-off
indefinitely if no approving row is completed. Elapsed time, repetition, a
different campaign's sign-off, or internal citation never promote them. A
completed row with a non-approving disposition does not promote them
either. They may be invalidated by sign-off (§2.3), and they must not be
described as confirmatory in the meantime.

### 2.2 Claim vocabulary

The following vocabulary is normative. It applies to reports, ledgers,
README/ROADMAP/vision text, release notes, issue comments, and any
external communication.

| Status | Permitted | Forbidden |
| :--- | :--- | :--- |
| Governed pre-sign-off measurement (§2.1) | "governed pre-sign-off measurement"; "governed decision under the frozen rule (pre-sign-off)"; a decision category **with the qualifier**, e.g. "POSITIVE (pre-sign-off)"; "collected under pre-registration <version>, pre-sign-off"; "not confirmatory"; "not publication-ready" | "confirmatory" or "confirmatory data/result/evidence" (unqualified); "pre-registered result" or "pre-registered finding" unqualified; "publication-ready"; "validated result"; "proven"; any publication, press, or venue claim |
| Conditionally held (`APPROVE WITH CONDITIONS`, conditions open) | "governed pre-sign-off measurement (conditionally held; conditions N open)"; "pre-sign-off approval with conditions"; "not confirmatory"; "not publication-ready" | "confirmatory"; "approved" (unqualified); "publication-ready"; every confirmatory phrase in the row below |
| Confirmatory (**approving** signed row, conditions closed) | "confirmatory result (PR-05-signed <date>, manifest <hash>)"; "pre-registered result" within the signed scope; publication claims within the signed scope | "confirmatory" beyond the signed scope; any confirmatory claim from a `REJECT` or `FREEZE CHAIN NOT DEMONSTRABLE` row or from a conditionally held row; any confirmatory claim after a scope-changing amendment; an independence claim the record does not state |
| Pilot / exploratory / exposure | "pilot", "exploratory", "exposure", "descriptive", "operational measurement" | every pre-sign-off and confirmatory phrase above; pre-sign-off status requires a frozen pre-registration |

"Confirmatory" may appear in governance text only to define the status or
to state what a result is not. A decision category (POSITIVE, NEGATIVE,
INCONCLUSIVE, INVALID, STOPPED) may be reported before sign-off, but every
such report carries the pre-sign-off qualifier.

### 2.3 Sign-off disposition over pre-sign-off data

The reviewer records one disposition for each campaign:

| Disposition | Effect on the pre-sign-off data |
| :--- | :--- |
| `APPROVE` | The data may be promoted to confirmatory as of the sign-off date, for the signed manifest hash, after the relabeling in §2.1(5). |
| `APPROVE WITH CONDITIONS` | The data stay pre-sign-off until every condition is closed and recorded by an amendment that changes no analysis content. A condition that changes analysis content (endpoint, denominator, threshold, case set, or analysis script) voids promotion; a held-out re-run is required. |
| `REJECT` | The data are invalidated for the affected endpoint. They may be reported only as invalidated/exploratory; a held-out re-run is required. |
| `FREEZE CHAIN NOT DEMONSTRABLE` | The data are invalid for the affected endpoint because the four conditions in §2.1 cannot be demonstrated. No promotion is permitted. |

The reviewer must also adjudicate any campaign-specific deviation the
frozen text assigns to PR-05 (for example #1163's ≥30 control floor and
pilot-disjointness interpretation). That adjudication is part of the
verdict.

### 2.4 Scope

This section covers the four PR-06 campaigns only:

- **#1163** and **#1166** collected governed pre-sign-off data on
  2026-11-27/28 under amendments 2 and 6. Their reported categories stay
  POSITIVE with the pre-sign-off qualifier until C1/C4 sign-off.
- **#1164**'s ten-case pilot was measured before the freeze. It is
  pilot/exploratory evidence, not pre-sign-off data; a held-out re-run is
  required.
- **#1165** has collected no data.

The AI-synthesis ablation (#1395, report #1412) is **not** a PR-06
campaign and is out of scope. It is a non-governed operational ablation
with no held-out intents; it must not be relabeled as pre-sign-off data,
folded into #1164, or used to satisfy any PR-05 row.

### 2.5 Publication gate (unchanged)

PR-05 sign-off remains mandatory before publication. Pre-sign-off status:

- does not authorize publication, a preprint, a press claim, or a venue
  submission;
- does not satisfy Task 4.0 or ledger Gate 3;
- does not substitute for independent reanalysis (PR-01 §0 rule 5) or
  independent replication;
- may be cited internally, and in this governance record, with its
  qualifier.

The original prohibition stands: no result may be presented as
confirmatory, or as a pre-registered result unqualified, until the
corresponding row is completed with an approving disposition and recorded
under §5.

### 2.6 Relation to the other frozen artifacts

PR-01–PR-04 and PR-06 use "confirmatory data collection", "confirmatory
dataset", and "confirmatory result" in their original sense: data
collected for a campaign intended to support a confirmatory claim, with
the analysis frozen before the run. This amendment changes none of those
artifacts (their hashes are unchanged) and none of their analysis content.
Read under this section, such data are governed pre-sign-off data until
the corresponding PR-05 row records an approving disposition, and the
phrases authorize neither a confirmatory label nor a publication claim
before then. Where a
frozen artifact says a result cannot be presented as a pre-registered
confirmatory result until a condition is met, that prohibition is
preserved.

## 3. Review checklist

The reviewer completes every item and records the evidence path. A single
`REJECT` blocks the campaign; `APPROVE WITH CONDITIONS` blocks until the
conditions are closed and recorded by an amendment that changes no
analysis content.

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

## 4. Sign-off records

Each campaign needs its own completed row. All fields are mandatory.

### C1 — #1163

| Field | Value |
| :--- | :--- |
| Campaign | #1163 — BioTypes broad truth-set validation |
| Artifacts reviewed (version) | PR-01 1.0.4, PR-02 1.0.4, PR-03 1.0.4, PR-04 1.0.4, PR-05 1.1.0, PR-06 1.0.4 (current at amendment 11; the reviewer records the versions actually reviewed) |
| Manifest hash reviewed | PENDING |
| Reviewer role | Independent scientific reviewer — BioTypes/bioinformatics |
| Reviewer name | PENDING |
| Affiliation / independence declaration | PENDING |
| Conflicts of interest | PENDING |
| Outcome access at review | PENDING (NONE / PILOT SEEN / PRE-SIGN-OFF DATA SEEN / CONFIRMATORY SEEN) |
| Verdict | PENDING (APPROVE / APPROVE WITH CONDITIONS / REJECT / FREEZE CHAIN NOT DEMONSTRABLE) |
| Conditions | PENDING |
| Date | PENDING |
| Signature or cryptographic attestation | PENDING |

### C2 — #1164

| Field | Value |
| :--- | :--- |
| Campaign | #1164 — applied-repair verified by successful reruns |
| Artifacts reviewed (version) | PR-01 1.0.4, PR-02 1.0.4, PR-03 1.0.4, PR-04 1.0.4, PR-05 1.1.0, PR-06 1.0.4 (current at amendment 11; the reviewer records the versions actually reviewed) |
| Manifest hash reviewed | PENDING |
| Reviewer role | Independent scientific reviewer — AI repair / statistics |
| Reviewer name | PENDING |
| Affiliation / independence declaration | PENDING |
| Conflicts of interest | PENDING |
| Outcome access at review | PENDING (PILOT SEEN must be declared — the 2026-11-18 pilot outcome is already public; #1164 has no pre-sign-off data) |
| Verdict | PENDING (APPROVE / APPROVE WITH CONDITIONS / REJECT / FREEZE CHAIN NOT DEMONSTRABLE) |
| Conditions | PENDING |
| Date | PENDING |
| Signature or cryptographic attestation | PENDING |

### C3 — #1165

| Field | Value |
| :--- | :--- |
| Campaign | #1165 — corpus-wide Nextflow/Snakemake semantic equivalence |
| Artifacts reviewed (version) | PR-01 1.0.4, PR-02 1.0.4, PR-03 1.0.4, PR-04 1.0.4, PR-05 1.1.0, PR-06 1.0.4 (current at amendment 11; the reviewer records the versions actually reviewed) |
| Manifest hash reviewed | PENDING |
| Reviewer role | Independent scientific reviewer — workflow engines |
| Reviewer name | PENDING |
| Affiliation / independence declaration | PENDING |
| Conflicts of interest | PENDING |
| Outcome access at review | PENDING (NONE / PILOT SEEN / PRE-SIGN-OFF DATA SEEN / CONFIRMATORY SEEN) |
| Verdict | PENDING (APPROVE / APPROVE WITH CONDITIONS / REJECT / FREEZE CHAIN NOT DEMONSTRABLE) |
| Conditions | PENDING |
| Date | PENDING |
| Signature or cryptographic attestation | PENDING |

### C4 — #1166

| Field | Value |
| :--- | :--- |
| Campaign | #1166 — cross-architecture reproduction measurement |
| Artifacts reviewed (version) | PR-01 1.0.4, PR-02 1.0.4, PR-03 1.0.4, PR-04 1.0.4, PR-05 1.1.0, PR-06 1.0.4 (current at amendment 11; the reviewer records the versions actually reviewed) |
| Manifest hash reviewed | PENDING |
| Reviewer role | Independent scientific reviewer — reproducibility / systems |
| Reviewer name | PENDING |
| Affiliation / independence declaration | PENDING |
| Conflicts of interest | PENDING |
| Outcome access at review | PENDING (PRE-SIGN-OFF DATA SEEN must be declared — the governed campaign ran 2026-11-28) |
| Verdict | PENDING (APPROVE / APPROVE WITH CONDITIONS / REJECT / FREEZE CHAIN NOT DEMONSTRABLE) |
| Conditions | PENDING |
| Date | PENDING |
| Signature or cryptographic attestation | PENDING |

## 5. How a sign-off is recorded

1. The reviewer writes the completed record into a new version of this
   artifact **and** a dated entry in `AMENDMENTS.md` naming the manifest
   hash that was reviewed.
2. The sign-off entry includes the reviewer's name, role, independence
   declaration, outcome-access declaration, disposition over any
   pre-sign-off data (§2.3), verdict, conditions, date, and an attestation
   token (a detached signature over the manifest hash, or a named written
   attestation in the review thread, recorded by URL).
3. A sign-off is valid only for the manifest hash it names. Any change to
   PR-01 through PR-04 or PR-06 invalidates it until the reviewer confirms
   that the change does not affect the signed scope. The manifest is
   recomputed with the signed PR-05 and the amendment entry records both
   the pre-signature hash reviewed and the recomputed manifest (§2.1).
4. A conditional approval lists each condition as a numbered item and
   names the artifact section that must change. Conditions are closed and
   recorded by an amendment that changes no analysis content; the campaign
   stays blocked until then, and any condition that changes analysis
   content voids promotion of pre-sign-off data (§2.3).
5. The act of signing with an `APPROVE` disposition — or with
   `APPROVE WITH CONDITIONS` once every condition is closed and recorded
   — is what creates a confirmatory result; a row completed with `REJECT`,
   `FREEZE CHAIN NOT DEMONSTRABLE`, or open conditions creates none. A
   reviewer cannot have seen this campaign's confirmatory outcome before
   signing it. A later reviewer of a campaign whose data were already
   promoted declares `CONFIRMATORY SEEN`. A reviewer who has seen this
   campaign's pre-sign-off data declares `PRE-SIGN-OFF DATA SEEN` and
   adjudicates it under §2.3; that is not a `POST-OUTCOME REVIEW` and does
   not by itself make the data exploratory.

## 6. Gate effect

Completion of all four rows is necessary but not sufficient for Task 4.0:
the pre-run manifests, case seals, and analysis-script hashes must also
exist. Pre-sign-off status (§2) does not shorten this list, satisfy any
row, or count toward Task 4.0 or Gate 3. Until the rows and seals exist,
Task 4.0 remains **Planned / blocked** and ledger Gate 3 remains
unpassed. Artifact creation is not gate passage.
