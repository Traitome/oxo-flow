# PR-04 — Leakage Audit and Held-Out-Intent Custody

| Freeze record | Value |
| :--- | :--- |
| Artifact | PR-04 (Task 4.0 required artifact 4 of 6) |
| Version | 1.0.2 |
| Freeze status | FROZEN |
| Freeze date | 2026-11-18 (original freeze; amendment 1 same date; amendment 2 on 2026-11-28) |
| Amendment | 1 (2026-11-18): review fixes D1–D5 and clarifications — see [`AMENDMENTS.md`](AMENDMENTS.md) entry 1. 2 (2026-11-28): #1166 workflow, host, and artifact-class seal frozen — see entry 2 |
| Content binding | SHA-256 recorded in [`MANIFEST.sha256`](MANIFEST.sha256) |
| Campaigns covered | #1163, #1164, #1165, #1166 |
| Change control | Post-freeze edits only through [`AMENDMENTS.md`](AMENDMENTS.md) |

**Frozen-before-data statement.** This audit and custody protocol were
frozen before confirmatory data collection for #1163, #1165, and #1166.
#1164's pilot outcome was already observed; §C2 records that exposure and
its consequence rather than claiming a pre-outcome custody state.

## 1. What counts as leakage

A leak is any path by which knowledge of a held-out case, its expected
outcome, or the analysis choice reaches the system being measured before
the confirmatory outcome is recorded.

| ID | Vector | Why it invalidates the campaign |
| :--- | :--- | :--- |
| L1 | **Benchmark-to-knowledge leak** — case text, expected verdict, or expected outputs embedded in `crates/oxo-flow-ai/src/knowledge/*.jsonl`, a skill, or a system prompt | The AI arm can retrieve the answer instead of generating it; a gate pass no longer measures the engine |
| L2 | **Prompt leak** — held-out case content pasted into a generation or recovery prompt, few-shot example, or task contract | Same, through the prompt channel |
| L3 | **Selection leak** — the case set is chosen or pruned after observing how the checker/importer/engine behaves | The measured set is conditional on the system under test; the rate is not an estimate of the population the set claims to represent |
| L4 | **Analysis leak** — the primary analysis script, threshold, or exclusion rule is written after outcomes are seen | The result is fitted to the data; pre-registration is void for that endpoint |
| L5 | **Training exposure** — the model executing the campaign was trained on the public case, patch, or expected output | Cannot be eliminated by access control; must be declared and mitigated with a held-out set |
| L6 | **Post-freeze edit leak** — frozen artifacts or a manifest are edited without an amendment record | The frozen text no longer proves the promised pre-outcome state |
| L7 | **Mid-campaign system change** — the binary, importer, container digest, or prompt changes between cases | Cases are measured against different systems; the aggregate is uninterpretable |

## 2. Custody protocol

### 2.1 Roles

| Role | Duty |
| :--- | :--- |
| Case author (independent) | Authors cases and expected outcomes without running the system under test to derive them |
| Custodian | Holds the sealed case bodies, hashes, and the reserve list; releases them only to the runner and the analyst |
| Runner | Executes the frozen manifest; may not edit cases or prompts |
| Analyst | Computes the frozen primary endpoint; sees outcomes only after the analysis script hash is sealed |
| Independent reviewer | Audits this protocol and signs PR-05; not the author of the system under test |

A person may hold more than one role only where this document says so and
the conflict is declared. The case author and the independent reviewer
must not be the author of the engine change under test. Names and
independence declarations are recorded in PR-05; they are **PENDING** at
freeze and no sign-off is claimed.

### 2.2 Sealing sequence (before the first confirmatory run)

1. Author the case list, expected outcomes, and coverage map.
2. Compute SHA-256 for every case body and for the canonical case list
   (key-sorted JSON, UTF-8, trailing newline).
3. Commit the **hashes and case metadata**, not necessarily the held-out
   bodies, and record the custody location for the bodies.
4. Seal the pre-run manifest (PR-03 §0), the analysis script, the prompt
   hashes, and the expected output manifests.
5. Record the seal in `AMENDMENTS.md` as a pre-data entry (date, owner,
   hash list). The seal is valid only if no confirmatory outcome has been
   examined.
6. Release the case bodies to the runner. The runner executes only the
   frozen protocol.

### 2.3 Leakage checks

Run before the campaign and after each batch, with the exact commands
recorded:

| Check | Command or method | Expected result |
| :--- | :--- | :--- |
| Gold-set provenance and negative-name leakage | `make eval-lint` (`python3 eval/scripts/check_gold.py --no-fetch-community`); requires a built engine binary at `target/debug/oxo-flow` or `--oxo-flow <path>` — run `cargo build -p oxo-flow-cli` first | Exit 0; no findings |
| Gold evidence integrity (where the campaign touches gold rows) | `python3 eval/scripts/verify_gold.py` | Exit 0; hash consistency holds |
| Case-text leak into knowledge | Grep the knowledge JSONL and every prompt/skill source for each sealed case ID and a distinctive body fingerprint; record the command and the hit count | Zero hits |
| Case-text leak into prompts | Grep generation/recovery prompt sources for sealed fingerprints; hash the prompt sources and compare to the manifest | Zero hits; hashes match |
| Analysis-script integrity | Recompute the sealed script hash immediately before analysis | Matches the pre-run manifest |
| System-identity stability | Recompute the binary SHA-256, engine commit, container digests, and prompt hashes mid-campaign and at the end | Match the pre-run manifest |
| Manifest integrity | `shasum -a 256 -c MANIFEST.sha256` in this directory | All six PR artifacts OK |

The existing `make eval-lint` gate covers the gold set's provenance and
negative-name leakage. It does not scan the frontier intent set or a new
truth set; that coverage must come from the campaign-specific grep and
hash check above, whose script is committed and sealed before data
collection.

### 2.4 Breach handling

1. **Before any confirmatory outcome is seen**: replace the affected case
   from the sealed reserve list, re-seal, and document the replacement.
2. **After an outcome is seen**: the affected endpoint is labeled
   **exploratory** in the report and in PR-06's decision record. It may
   not be presented as the pre-registered confirmatory result.
3. A system-identity change (L7) mid-campaign stops the campaign under
   PR-02 §0.3 and restarts it with the frozen system; results from the
   changed system are reported separately.
4. Any breach is reported in the campaign report even if the result is
   otherwise positive.

## C1 — #1163 truth-set custody

- **Threat profile.** The checker is deterministic; there is no model in
  the loop, so L1/L2/L5 are low risk for this campaign. The dominant risks
  are **L3 selection** and **L4 analysis**: choosing cases by running the
  checker and keeping the ones that behave, or choosing the threshold after
  seeing the false-accept count.
- **Custody rule.** Truth-set cases are authored and expected verdicts
  recorded before the checker is run on them. The case bodies may be
  public (no model learns from them during the campaign); the expected
  verdict file is sealed by hash. Cases must not be pruned or reworded to
  make the checker pass; any pre-run correction is recorded as an
  exclusion/replacement with the original hash.
- **Independent author.** The expected verdicts must be authored or
  independently reviewed by someone who is not the author of the
  `TYPE-*` change under test. The identity is PENDING in PR-05.
- **Audit at freeze.** No truth set has been authored yet, so the
  pre-run leakage audit has no held-out material to scan. The obligation
  attaches to the pre-run seal.

## C2 — #1164 repair-campaign custody

- **Threat profile.** This campaign has a model in the loop, so L1, L2,
  and L5 are live. The pilot used ten cases that are **public repository
  content**: their seeded workflows, expected codes, and reference repairs
  are in `eval/frontier/fault_injection.py`, and the pilot outcome is in
  `eval/frontier/PILOT_FINDINGS.md`. A model may have seen this material
  in training or retrieval. That exposure cannot be undone by this
  protocol.
- **Consequence.** The confirmatory campaign must use a case set that was
  not published before the run. The case bodies, expected diagnostics, and
  reference repairs are held by the custodian; only hashes and metadata
  are committed before the run. After the run, the set is published with
  the raw logs so the result is reproducible.
- **Prompt custody.** The engine recovery prompt is in-repo and fixed by
  the binary commit. Its hash is recorded. No case-specific hint,
  expected-code string, or reference patch may be added to the prompt or
  to `[ai]` config.
- **Pilot disclosure.** The pilot's 3/10 applied and 3/10 verified result
  is recorded as already-observed evidence. It may not be presented as a
  confirmatory outcome, and it may not be used to choose the threshold
  retroactively (PR-06 §C2).
- **Audit at freeze.** The leakage audit records the pilot's public
  exposure as a declared limitation, not a clean state.

## C3 — #1165 corpus custody

- **Threat profile.** Import and execution are deterministic; the risks
  are **L3** (selecting corpus cases after seeing which import cleanly)
  and **L7** (patching the importer mid-campaign after a failure).
- **Custody rule.** The corpus, the source-engine versions, the
  `.oxoflow` counterparts, and the expected declared-output manifests are
  sealed before the first run. Cases are drawn from the documented
  supported subset by a rule fixed in advance, not by "whatever imports
  successfully after a fix". Workflows outside the subset are recorded in
  the unsupported-construct table; they are not silently dropped.
- **No mid-campaign fixes.** A discovered importer defect is recorded as a
  measured failure under the frozen binary. A fix lands after the campaign
  and is evaluated as a new version; the campaign result stands for the
  frozen version.
- **Counterpart review.** The independently reviewed `.oxoflow`
  counterpart is authored before the run; manual corrections are listed.
  A counterpart that had to encode a semantic decision is excluded from
  the equivalence denominator (PR-02 §C3), not counted as a win.
- **Audit at freeze.** No corpus has been frozen; the existing comparative
  echo chain is a seed, not a held-out corpus.

## C4 — #1166 reproduction custody

- **Threat profile.** No model; the risks are **L3** (choosing the
  workflow or artifact classes after seeing which checksums match) and
  **L7** (changing the container, lock, or binary between arms).
- **Custody rule.** The workflow, declared output list, input/reference
  hashes, environment lock, and container digest are sealed before the
  first run. Artifact classes are enumerated before the run; a class may
  not be added or removed after seeing the comparison.
- **Independence of arms.** Both arms use the same inputs and references
  by content hash and the same container digest. A run with a different
  digest is recorded as a tooling-attribution case and cannot support the
  primary claim.
- **Audit at freeze.** The two-workflow corpus, the aarch64 host identity, the
  environment lock (one multi-arch manifest-list digest), and the eight
  artifact classes are fixed in the pre-run manifest (frozen 2026-11-28;
  amendment 2). The sealing obligation now attaches to that committed
  manifest; the corpus, class map, and analysis script hashes are recorded
  there before the first confirmatory run.

## 3. Disclosure requirements

Every campaign report must include, regardless of result:

1. The role separation actually used, with names or PENDING markers.
2. The leakage checks run, their commands, and their outcomes.
3. Every known exposure that cannot be eliminated (for example #1164's
   public pilot cases and model training exposure).
4. Any breach and its handling.
5. The seal hashes, and confirmation that the executed manifest matches
   the frozen one.

A report that omits this section is not publication-ready, even if its
primary endpoint is positive.
