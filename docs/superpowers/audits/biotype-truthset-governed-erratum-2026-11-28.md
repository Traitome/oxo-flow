# BioType governed pre-sign-off truth-set campaign — post-review erratum (2026-11-28)

> **Scope.** This erratum accompanies
> [`biotype-truth-set-governed-2026-11-27.md`](biotype-truth-set-governed-2026-11-27.md)
> after an independent review of PR #1443 found **1 HIGH, 4 MEDIUM, and 5
> LOW** issues. Every finding is fixed below or in the amended
> pre-registration (AMENDMENTS entry 4, artifact version 1.0.3). **No
> primary endpoint, threshold, direction, denominator, or FA result
> changed.** The FA primary analysis remains 0/83 with an exact 95% upper
> bound of 4.3471% ≤ `τ_FA = 0.05`; the POSITIVE category remains subject
> to PR-05 C1 adjudication of the control-provenance deviation described in
> §1. Under PR-05 §2 (amendment 11) the campaign is a **governed
> pre-sign-off measurement**, not a confirmatory result and not
> publication-ready.

## 1. HIGH — nine controls were pre-existing gallery examples (fixed by disclosure)

**Finding.** The sealed corpus's `g108`–`g116` controls are
`source: repository-example`: pre-existing `examples/gallery/*.oxoflow`
files that the auto-discovering gates `cli_validate_all_gallery_workflows`
(`tests/cli_integration.rs:3552`) and `cli_lint_all_gallery_workflows`
(`tests/cli_integration.rs:5184`) validate/lint on every CI run. Their
accept outcome was therefore **known before the freeze**, so they are not
"authored after the amendment with no outcome observed". Only **24** of the
33 controls were authored post-freeze.

**What this does and does not affect.**

- The **83 expected-reject cases** are the FA primary analysis; all were
  authored after amendment 2 (corpus commit `20ad630b`, after the
  amendment commit `89c4127d`). Their outcomes were not observed before
  the run. The FA result (0/83, U = 4.3471%) is unaffected.
- The control floor is **ambiguous**: the frozen text counts 33 sealed
  controls (33 ≥ 30), but if the floor was intended to apply only to
  held-out authored controls, the count is 24 < 30. **PR-05 must
  adjudicate this.** The pre-registered analysis set and the category are
  not silently rewritten here.

**Fixes.**

1. Amendment 4 (commit `ef42c659`) corrects PR-01 §C1, PR-02 §C1,
   PR-03 §C1, and PR-06 §C1 to state the true composition (83
   expected-reject + 24 authored controls + 9 pre-existing gallery
   controls) and to require PR-05 adjudication of the ≥30 floor. PR-04's
   C1 "Independent author" sentence is corrected by amendment 5 (commit
   `00677a9b`): it now names the authored portion of the governed corpus
   and discloses the 9 pre-existing gallery controls as exposure, pointing
   to this erratum §1. The manifest was recomputed for amendment 5.
2. The corpus report's title, status block, §1, §5.1, §6.2, and §11 now
   distinguish the authored held-out subset from the 9 exposed controls.
3. A separately sealed **post-hoc supplement** (`§5` below) adds new
   authored acceptance controls, including the missing `TYPE-005`
   accept-side contrast. It is explicitly descriptive, was authored after
   the primary outcome, is not part of the pre-registered governed set,
   and **never enters the FA denominator**.
4. README, ROADMAP, vision, the pilot banner, and the preregistration
   status ledger now use the corrected composition.

## 2. MEDIUM — `N_min = 73` is not the minimum (fixed)

The zero-event exact 95% upper bound is `1 − 0.025^(1/N)`:

| N | Upper bound | ≤ 0.05? |
| ---: | ---: | :--- |
| 71 | 0.0506294 | no |
| 72 | 0.0499441 | **yes** |
| 73 | 0.0492771 | yes |

**72**, not 73, is the smallest expected-reject count that satisfies the
bound. The frozen value **73 is retained** as a one-case conservative
margin; the incorrect "smallest" rationale in AMENDMENTS entry 2,
PR-06 §C1, PR-01 §C1, and the report is corrected. The manifest was
recomputed for v1.0.3 (`1d306904…4b4ec9`, superseded by amendment 5's
`e3863f53…f97eb8e` after the PR-04 caveat).

## 3. MEDIUM — sealed toolchain fields recorded the ambient shell (fixed)

The pre-run manifest's `toolchain.rustc/cargo` recorded **1.97.1**, the
ambient shell's default rustup toolchain. The binary was built by
`cargo build -p oxo-flow-cli` inside the repository, which honors
`rust-toolchain.toml` and therefore used **1.98.0** (`rustc 1.98.0
(88d9e12ae 2026-08-18)`; `cargo 1.98.0 (797e8a9bc 2026-08-05)`). The report
already stated 1.98.0 and is correct. The sealed manifest is not rewritten
post-run; its **binary SHA-256**
(`463d15e618e28d4e694bc3aab472fe53a11188266e8f3f61a344e3099a820b0a`) is
the binding execution identity. The post-hoc supplement manifest
(`prerun/C1-governed-supplement-2026-11-28.json`) captures the
repository-pinned 1.98.0 toolchain explicitly. A fresh debug rebuild after
the run has a different SHA-256; byte-identical rebuilds are **not**
claimed (Rust debug builds can embed build metadata). The evidence chain
is the recorded source commit + the sealed case set + the as-run binary
hash in the results JSON, not a rebuild.

## 4. MEDIUM — case bodies were not content-sealed (fixed)

PR-03 §C1 had said "case bodies and expected verdicts … sealed by SHA-256".
In fact the seal covers the **expected verdicts** (canonical `cases` array
hash `b5db3653…f3e1`) and the prior gate only checked that each case path
**existed** at the stamped commit — a post-stamp body edit would not fail.
Fixed:

- `make truthset-stamp` now compares the working-tree case blob with the
  stamped commit's blob for **every** case in all three results files, and
  fails closed on any content difference.
- The gate verifies each governed results file's `manifest_sha256` against
  the actual sealed manifest, and verifies the decision file's
  `results_sha256` against the results file.
- Negative test performed: appending a comment to
  `s01_bam_readgroup_accept.oxoflow` made the gate fail with the stamped
  and worktree blob hashes; the file was then restored.
- PR-03 §C1 wording corrected (amendment 4).

## 5. MEDIUM — `TYPE-005` had no accept-side contrast (fixed by supplement)

The primary corpus's `TYPE-005` cases (`g064`–`g066`) are all
expected-reject; no primary control exercised a satisfied read-group
contract. A post-hoc supplement was authored, sealed
(`prerun/C1-governed-supplement-2026-11-28.json`, SHA-256
`757891a4…2fc3`, commit `0204c1e4`), and run through the same real
surface:

| Supplement control | Cell | Expected | Actual |
| :--- | :--- | :--- | :--- |
| `s01_bam_readgroup_accept` | BAM `@RG` ID,SM,PL satisfied | accept | accept |
| `s02_bam_readgroup_lb_accept` | BAM `@RG` ID,SM,PL,LB satisfied | accept | accept |
| `s03_vcf_explicit_none_accept` | VCF `compression=none` both sides | accept | accept |
| `s04_bed_bgzip_tbi_accept` | BED bgzip + tbi both sides | accept | accept |
| `s05_gtf_grcm39_accept` | GTF GRCm39 both sides | accept | accept |
| `s06_fasta_bgzip_fai_accept` | FASTA bgzip + fai + GRCh38 | accept | accept |
| `s07_h5ad_attribute_accept` | H5AD `assay=scrna` both sides | accept | accept |

Result: **7/7 accepted, codes 7/7**, 0 non-verdicts, elapsed 0.7 s
(`data/biotype-truthset-governed-supplement-results.json`). The supplement
is descriptive only; it does not enter the FA denominator and does not
retroactively satisfy the pre-registered ≥30 held-out control floor.

## 6. LOW — budget enforcement wording (fixed)

The sealed primary runner enforced the **60 s per-case** subprocess
timeout but not the 7200 s total as a hard timer. The observed primary
campaign wall time was ~20 s, so the ceiling was never approached. The
frozen budget is now stated as a declared ceiling for the primary and as
enforced for the supplement; the supplement runner enforces **both**
deadlines and records `elapsed_secs` (0.7 s) in its result file.

## 7. LOW — stale present-tense statements (fixed)

- `biotype-truth-set-2026-11-20.md`: banner corrected to "sealed 116-case
  corpus" with the authored/exposed control split.
- `typed-pipeline-evidence-2026-11-20.md`: a governed-follow-up banner is
  added, and the three "`τ_FA` remains UNDECIDED" statements are marked as
  historical/at-freeze statements.
- `preregistration/README.md`: version 1.0.3, the #1163 row records the
  erratum and supplement, and the stale "No campaign was run … no empirical
  result" sentence is corrected to a freeze-time statement.
- `industrial-scientific-audit-ledger.md` WF-04: the long historical
  narrative is banner-marked as historical; the status cell carries the
  governed pre-sign-off measurement (POSITIVE (pre-sign-off); not
  confirmatory; PR-05 C1 sign-off pending).

## 8. LOW — `g049`/`g052` header claim superseded (fixed by note, no body edit)

The sealed case headers for `g049`/`g052` say "Distinct … from the pilot's
cases". Report §8.1 records that this header claim is **superseded** by the
post-run pilot-disjointness audit (they re-assert the pilot `t04` VCF index
cell under a strict reading). The sealed case bodies were **not edited**;
the manifest and case hashes stay intact.

## 9. LOW — analysis-script override flags removed (fixed)

The sealed v1 analysis script accepted optional `--tau`/`--n-min`
overrides, which could recompute the decision with non-frozen constants
while the script hash stayed unchanged. The hardened script removes both
flags: `TAU_FA = 0.05` and `N_MIN = 73` are the only constants it can use.
The change is recorded here (v1 hash `c5c0311b…9fbb`; hardened hash
`0c431b21…2000`); re-running the hardened script on the committed results
reproduces `biotype-truthset-governed-decision.json` **byte-identically**
(verified with `diff`).

## 10. LOW — documentation boundaries added (fixed)

README, ROADMAP, and vision now state that the governed result is
**static `validate`-surface validation only, single arm**, and point to
report §11; it is not execution evidence and not biological truth.

## 11. What is unchanged

- **Primary FA analysis**: 0/83 false accepts, exact 95% upper bound
  **4.3471%** ≤ `τ_FA = 0.05`; category **POSITIVE** (PR-06 §C1), subject
  to PR-05 adjudication of the control-provenance deviation.
- Threshold `τ_FA = 0.05`, direction (lower is better), primary
  denominator (expected-reject cases), and the sealed 83-case reject set.
- The pilot remains exploratory/exposure and is never pooled.
- Independent reanalysis (PR-01 §0 rule 5) and PR-05 sign-off remain
  **PENDING**; Gate 3 remains unpassed.

## 12. Commit and hash record

| Artifact | Value |
| :--- | :--- |
| Amendment 4 commit | `ef42c6599a90861962484c96ce6ec9ede4bc94b6` |
| Amendment 5 commit (PR-04 caveat) | `00677a9be84c2ddf4ff0ba832d159f55e75af9f3` |
| v1.0.3 `MANIFEST.sha256` (amendment 4) | `1d3069043f53bdf13386b248392ba63ef35b296e46b65b764fac251d2e4b4ec9` |
| v1.0.3 `MANIFEST.sha256` (amendment 5; superseded by the merged 1.0.4 manifest) | `e3863f53fe0245b191a8215279623a8756f0b472e3a096e7f91ea3042f97eb8e` |
| v1.0.5 `MANIFEST.sha256` (current; PR-05-only amendment 11 — the merged 1.0.4 was `7e398477…`) | `7f27300563fcf2baaa82e07a9c68b67b3b757f44493917d6e9b6568b5c11d108` |
| Supplement corpus + runner commit | `742a684b81be06a2a3c723f9b7dbfb7afbae1769` |
| Supplement seal commit | `0204c1e44ea167b4598be8a27d6b67be13fea207` |
| Supplement manifest SHA-256 | `757891a479948bb2c6933ee61ce3df96a233b8240bf92a4862ad25309e552fc3` |
| Supplement case-manifest SHA-256 | `9b41a04d332850e483ec57a39d5e78125552754090bf2e516aa1e0576a2075a9` |
| Supplement runner SHA-256 | `47bb15017cf8859e60e6e9517837ae6a2de77ab9416b954cde5b432fa1074423` |
| Primary analysis script (v1, sealed) | `c5c0311b4f509f93eb4a8f1e40a1ff97bd9536065271d85d5e7263ea04739fbb` |
| Primary analysis script (v2, hardened) | `0c431b21f7ca088f720c5fce176ac4bed7ccb9fddef9f88d894d09245ade2000` |
| Primary runner (sealed, unchanged) | `e7a8e5fde464faebeedfff220ca75f730c2ddf752f52725027cebbbbffffd0ea` |
| Mainline squash-merge stamp repair (2026-11-28) | governed results/decision and supplement stamps repointed from `6fbf38c1`/`0204c1e4` to the mainline squash commit `b771feb222ba58a5845e860bdd84034e9f096719`; the squash commit contains the identical sealed case tree, and the measured data, `engine_commit`, `binary_sha256`, and manifest binding are unchanged. See `stamp_note` in the three JSON artifacts. |
| Binary SHA-256 (binding identity) | `463d15e618e28d4e694bc3aab472fe53a11188266e8f3f61a344e3099a820b0a` |

## 13. PR-05 adjudication checklist

1. **Control floor**: does the pre-registered "≥30 expected-accept
   controls" require held-out authored controls (24, short) or the sealed
   corpus as a whole (33, met)? The 9 pre-existing gallery controls are
   exposure.
2. **Pilot-disjointness interpretation** (report §8.1): coverage-cell
   reading (POSITIVE; N=79–83 sensitivity still POSITIVE) versus a stricter
   dimension-only reading (UNDERPOWERED).
3. **Independent reanalysis** of the 0/83 primary endpoint from the raw
   per-case JSON and the sealed/hardened analysis script.
4. **Confirm toolchain identity**: binary SHA-256 above; repository-pinned
   rustc/cargo 1.98.0 (the sealed manifest's 1.97.1 fields record the
   ambient shell and are corrected here).
