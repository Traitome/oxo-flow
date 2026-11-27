# Amendment Log

**Original freeze version:** 1.0.0

**Current version:** 1.0.4 (amendments 1–10; amendment 6 is the #1166 parameter freeze; amendments 7–10 are #1166 harness/status/tooling)

**Freeze date:** 2026-11-18

**Manifest file:** [`MANIFEST.sha256`](MANIFEST.sha256) — current SHA-256
`7e3984771249522314d360a617132609c741daae96b8d11cd1f2e8615121bbfe`; the
superseded v1.0.0, v1.0.1, v1.0.2, and pre-amendment-5 v1.0.3 manifest
hashes are retained in the history table in §2.1.

**Freeze commit:** `62d9161b2992` (the commit that introduced this
directory). The amendment-1 commit is recorded in
`docs/superpowers/audits/industrial-scientific-audit-ledger.md` §7 wave 7.

## 1. Rule

Every change to PR-01 through PR-06 after the freeze commit requires an
entry here, dated and signed by the approver. An entry must record the
outcome-exposure state. A post-outcome change makes the affected endpoint
exploratory (PR-06 §1 rules 1–4). An entry that appends a reviewer
signature edits PR-05, so it changes PR-05's hash and requires the manifest
to be recomputed and committed; the entry names the pre-signature manifest
hash that was actually signed.

## 2. Entries

| # | Date | Artifact / section | Change | Reason | Outcome exposure | Effect | Approver |
| :--- | :--- | :--- | :--- | :--- | :--- | :--- | :--- |
| 1 | 2026-11-18 | PR-06 §0.1 and §C1–§C4; PR-01 §C3; PR-02 §C3; PR-03 §C3; PR-04 §2.3 table; README §2.3; AMENDMENTS §1 | v1.0.0 → v1.0.1: (D1) added the ordered evaluation algorithm and inclusive/exclusive interval boundary rules so exactly one of POSITIVE/NEGATIVE/INCONCLUSIVE/INVALID/STOPPED applies, and defined #1166's class-level NEGATIVE precedence plus `τ_ARCH` as the campaign-level threshold (class stability is 100% by definition, not a free parameter); (D2) corrected the false claim that the Nextflow version was unrecorded — `benches/comparative/README.md` records Nextflow 25.10.4 and Snakemake 9.19.0; (D3) added the missing `bwa-mem2` stub (nine stub tools, matching `fault_injection.py`); (D4) fixed the broken internal references (PR-01 → PR-02 §0.3, PR-01 → PR-03 §0, PR-02 → PR-04 §2.2); (D5) a case with zero declared outputs is INVALID for #1165 equivalence and cannot count as vacuously equivalent; (nit) corrected the signature/manifest recompute wording in README/AMENDMENTS and noted that `make eval-lint` needs a built engine binary | Independent review of PR #1225 (D1–D5 and two nits) | None — no confirmatory outcome was examined | Confirmatory; pre-data correction, no endpoint or direction changed | Campaign maintainer (pre-data correction requested by independent review) |
| 2 | 2026-11-27 | PR-01 §C1 and §5; PR-02 §C1; PR-03 §C1; PR-04 §C1; PR-06 §C1; AMENDMENTS §2, §2.1, §3 | v1.0.1 → v1.0.2, all frozen before any held-out case was authored or run: (a) #1163 `τ_FA` frozen at **0.05** (maximum acceptable false-accept proportion on expected-reject cases) with the exact two-sided Clopper–Pearson 95% interval and the PR-06 §0.1 lower-is-better mapping; (b) `N_min = 73` expected-reject cases plus at least 30 expected-accept acceptance controls — 73 was frozen with a one-case conservative margin (the true minimum is 72: `U = 1 − 0.025^(1/72) = 0.0499441`; 0/71 = 0.0506294); corrected by entry 4; (c) non-verdict limit frozen at **0** (any non-verdict ⇒ INVALID); (d) C1 pilot firewall: the 2026-11-20/22 N=54 operational set (32 TA / 0 FA / 22 TR / 0 FR) was measured while `τ_FA` was UNDECIDED, so it is **exploratory/exposure**, cannot be the confirmatory basis, cannot be pooled with the governed corpus, and its contract assertions may not be re-asserted by confirmatory cases; (e) corpus expansion protocol, independence rule (expected verdicts derived from the published contract only — schema, `workflow-format.md`, TYPE-* catalogue, published tool contracts — never the checker source or a prior run), and exclusion rules (pilot-disjointness, duplicate assertions, checker-derived verdicts); (f) single-arm measurement host, 60 s/case and 7200 s total static-only budget, and the pre-run-manifest seal location `preregistration/prerun/C1-governed-2026-11-27.json` + `.sha256`, whose `cases` array is the runner's single EXPECT source | PR-06 §C1 left `τ_FA` UNDECIDED; the issue #1396 acceptance requires a governed decision, which is impossible until the threshold is fixed before held-out data; the N=54 set cannot be the confirmatory basis (PR-06 §1 rule 2) | Pilot/exposure seen: the N=54 operational truth-set outcomes (2026-11-20/22) were observed while `τ_FA` was UNDECIDED. No expected-reject case and no authored acceptance control of the confirmatory corpus had been authored or run when this entry was committed; the 9 pre-existing gallery controls later included were already public and CI-validated, as disclosed in entry 4. | Confirmatory for the authored held-out subset only (83 expected-reject + 24 authored controls); 9 pre-existing gallery controls are exposure, and the pilot is exploratory/exposure and never pooled. No primary endpoint, direction, or denominator changed. | Campaign lead (maintainer), with independent review still PENDING (PR-05 C1) |
| 3 | 2026-11-27 | README §3 status ledger only | Status-only update after the governed run: `#1163 confirmatory data` changed from "None collected" to the governed result (sealed 116-case corpus: 83 expected-reject + 24 authored controls + 9 pre-existing gallery controls; 33 TA / 0 FA / 83 TR / 0 FR; exact 95% upper bound 4.3471% ≤ `τ_FA = 0.05`; POSITIVE; report `docs/superpowers/audits/biotype-truth-set-governed-2026-11-27.md`), and the Task 4.0 row now records #1163's completed pre-run manifest, case seal, and analysis-script hash. PR-01 through PR-06 text is unchanged; the manifest was recomputed and is unchanged (`7aeb3ebb5f445d21141a4a0c64c191c66b13a2e689403b45e15ed9d9b07afb18`). | Record the governed campaign result truthfully in the status ledger. | Confirmatory outcome seen: yes (run 2026-11-27T16:50:33Z, results commit `365e563a`). | Reporting-only; no rule, endpoint, threshold, direction, or denominator changed. | Campaign lead (maintainer) |
| 4 | 2026-11-28 | PR-06 §C1; PR-01 §C1; PR-02 §C1; PR-03 §C1; AMENDMENTS entry 2 (correction note); preregistration README §3; campaign report erratum | Post-review erratum (no endpoint change): (a) **control provenance** — 9 of the 33 expected-accept controls are pre-existing `examples/gallery/*.oxoflow` files validated by the gallery gates in `tests/cli_integration.rs` on every CI run before the freeze, so their accept outcomes were known; only 24 controls were authored post-freeze. The pre-registered "≥30 expected-accept controls" floor is met by the 33 sealed controls but not by the held-out authored subset (24); **PR-05 must adjudicate**. The 83 expected-reject cases — the FA primary analysis — were all authored post-freeze and are unaffected. A separately sealed post-hoc supplement of new authored acceptance controls, including the missing `TYPE-005` accept-side contrast, is added and reported descriptively; it is not part of the pre-registered confirmatory set and never enters the FA denominator. (b) **`N_min` rationale** — 72, not 73, is the smallest expected-reject N with a zero-event exact upper bound ≤ 0.05 (0/72 = 0.0499441; 0/71 = 0.0506294); the frozen value 73 is retained as a one-case conservative margin. (c) **toolchain** — the sealed manifest recorded the ambient shell toolchain (rustc/cargo 1.97.1), while the binary was built by `cargo build` in the repository, which honors `rust-toolchain.toml` (1.98.0); the binary SHA-256 is the binding identity and the erratum records the correction. (d) **case-body custody** — PR-03's "case bodies … sealed by SHA-256" wording is corrected: expected verdicts are sealed by the pre-run manifest's canonical `cases` array hash, and case bodies are pinned by the stamped commit and now checked byte-for-byte by `make truthset-stamp`; the gate also verifies the decision file's `results_sha256`. (e) **budget** — the sealed runner enforced the 60 s per-case timeout but not the 7200 s total as a hard timer; the observed campaign wall time was ~20 s, the distinction is recorded, and the supplement runner enforces both and records elapsed time. (f) **analysis-script hardening** — the optional `--tau`/`--n-min` overrides are removed from the sealed analysis script (new hash recorded; re-running it on the committed results reproduces the decision byte-identically). (g) stale present-tense text in the pilot, ledger, preregistration README, and typed-pipeline evidence is corrected or banner-marked. | Independent review of PR #1443 (1 HIGH, 4 MEDIUM, 5 LOW) | Confirmatory outcome seen (run 2026-11-27T16:50:33Z) | Reporting, custody, and coverage clarifications only. No primary endpoint, threshold, direction, denominator, or FA decision changed. The FA primary analysis remains 0/83; the POSITIVE category remains subject to PR-05 adjudication of the control-provenance deviation. | Campaign lead (maintainer), independent delta review PENDING |
| 5 | 2026-11-28 | PR-04 §C1; AMENDMENTS §2.1; MANIFEST.sha256 | PR-04 1.0.2 → 1.0.3: the C1 "Independent author" sentence now names the **authored portion** of the governed corpus (83 expected-reject + 24 authored controls) and discloses the 9 pre-existing gallery controls as **exposure**, cross-referencing the campaign erratum §1. Manifest recomputed (`1d306904…4b4ec9` → `e3863f53…f97eb8e`). | Delta review of PR #1443, item 4 (LOW documentation consistency): PR-04 §C1's "held-out governed corpus" lacked the 9-gallery exposure caveat. | Confirmatory outcome seen (run 2026-11-27T16:50:33Z). | Reporting/custody wording only; no rule, endpoint, threshold, direction, or denominator changed. | Campaign lead (maintainer), independent delta review PENDING |
| 6 | 2026-11-28 | PR-01 §5; PR-02 §C4; PR-03 §C4; PR-04 §C4; PR-06 §C4 and header; preregistration README §3; AMENDMENTS §2/§3 | v1.0.3 → v1.0.4: closed the #1166 pre-data values — `τ_ARCH = 0.80` and `n_min = 3`; primary endpoint fixed as the architecture-stable artifact proportion over a frozen 2-workflow / 24-artifact corpus in 8 classes of 3; exact Clopper-Pearson 95% CI (all-identical lower bound `0.025^(1/24) = 0.8574`); environment lock = one shared multi-arch manifest-list digest `sha256:5e2bdb1b…4fa5b`, identical in-container tool versions, same engine commit with a clean workflow tree; native x86_64 (`tx-ubuntu`) vs native aarch64 (Apple Silicon Docker Desktop linux/arm64 VM, no qemu); negative control `uname -m` excluded from the primary endpoint and required to differ; missingness limit 0% and the remaining INVALID conditions; corpus and analysis-script hashes sealed in the pre-run manifest | PR-05/README freeze protocol requires every UNDECIDED/TBD value to be closed by a pre-data amendment before the campaign starts | None — no confirmatory #1166 outcome was examined. The 2026-11-19 OS-confounded field observation and the 2026-11-20 native-amd64-vs-qemu-emulated-arm64 container observation are report-local operational measurements; they are not this frozen corpus, and no cross-arm run of this corpus existed at freeze | Confirmatory; pre-data amendment. The primary endpoint, direction, denominator, and threshold are fixed before data collection. PR-05 C4 independent reviewer sign-off remains PENDING and is disclosed in the campaign report; the governed category is not described as a publication-ready confirmatory result until sign-off | Campaign lead (repository owner-directed research session); infrastructure-owner role filled in the same session; independent reviewer sign-off PENDING |
| 7 | 2026-11-28 | Pre-run harness (not a PR-01..PR-06 artifact); `docs/superpowers/audits/data/crossarch-1397/preregistration.json` | Pre-data harness revision: `CROSSARCH_STAGE=0` pre-staged image path with a fail-closed architecture check; the orchestrator pre-stages both images without `docker rmi` | Docker Desktop 4.94 crashed on `docker rmi` of the digest-only reference during the first staging attempt; no workflow artifact was produced (identity-only attempt preserved at `docs/superpowers/audits/data/crossarch-1397/attempt-1-docker-stopped/`) | None — no confirmatory outcome was examined; no workflow artifact existed | Confirmatory; pre-data harness correction. No threshold, endpoint, corpus, denominator, or environment identity changed; the same manifest-list digest remains the lock. the pre-merge #1166 branch manifest was unchanged (`b07721b846cbd74808cae156cebdbb93f4e6c140cf289aad217ea6cc06e07609`); the merged 1.0.4 manifest is `7e3984771249522314d360a617132609c741daae96b8d11cd1f2e8615121bbfe` because no PR-01..PR-06 content changed | Campaign lead (repository owner-directed research session) |
| 8 | 2026-11-28 | Pre-run harness (not a PR-01..PR-06 artifact); `docs/superpowers/audits/data/crossarch-1397/preregistration.json` | Pre-analysis / pre-final-run harness revision 2: per-workflow evidence stems now come from the workflow's declared `[workflow] name`, matching the frozen corpus keys; the first complete two-arm run's (attempt-2) evidence is preserved under `docs/superpowers/audits/data/crossarch-1397/attempt-2-stem-mismatch/` | Attempt-2 used the `.oxoflow` file basename as the evidence stem, so the analysis could not pair the two arms; outputs and checkpoints were intact, and no cross-arm comparison was made or used | Attempt-2 checkpoint/output evidence exists but was NOT analyzed as the campaign result; the final analyzable run is at 17:01:00Z | Confirmatory; pre-analysis / pre-final-run harness correction. No threshold, endpoint, corpus, class, denominator, or environment identity changed. the pre-merge #1166 branch manifest was unchanged (`b07721b846cbd74808cae156cebdbb93f4e6c140cf289aad217ea6cc06e07609`); the merged 1.0.4 manifest is `7e3984771249522314d360a617132609c741daae96b8d11cd1f2e8615121bbfe` because no PR-01..PR-06 content changed | Campaign lead (repository owner-directed research session) |
| 9 | 2026-11-28 | Preregistration README §3 status ledger; campaign report `docs/superpowers/audits/cross-arch-provenance-governed-2026-11-27.md` | Post-outcome status update: one governed #1166 campaign collected data under amendment 6; result 24/24 identical (100%, exact 95% CI [85.75%, 100%]), 8/8 classes stable, category POSITIVE, negative control differed; PR-05 C4 sign-off remains PENDING | The honest status ledger must state what exists after the campaign | Confirmatory outcome seen — the #1166 campaign result | Status only; no PR-01..PR-06 content changed. The pre-merge #1166 branch manifest was unchanged (`b07721b846cbd74808cae156cebdbb93f4e6c140cf289aad217ea6cc06e07609`); the merged 1.0.4 manifest is `7e3984771249522314d360a617132609c741daae96b8d11cd1f2e8615121bbfe`. The result is reported as a governed decision, not a publication-ready confirmatory result, until PR-05 sign-off | Campaign lead (repository owner-directed research session) |
| 10 | 2026-11-28 | Final-run manifest `docs/superpowers/audits/data/crossarch-1397/preregistration.json`; analysis/arm/orchestrator/test scripts; campaign report; implementation plan, audit ledger, and the two prior #1166 audits | Post-outcome review-hardening: correct the manifest lineage (freeze `0eaca24b` at `dd5dcd44`, final-run `770ae8f2` at `56b109c1`, post-run `3082b228`), add sealed corpus/workflow/PR-06/MANIFEST hash checks, host-arch↔container-arch, `wc` version, image-src/staged-tag, and normalized env-lock digest checks with regression tests; add post-run host/container supplements; update stale v1.0.1/UNDECIDED/missing-data text; document remote/local cleanup | Independent review of PR #1444 found provenance-record inaccuracies and unmechanized validity checks | Confirmatory outcome seen — the #1166 result; the hardening does not change thresholds, corpus, or observed hashes, and the analysis re-derives the same POSITIVE result | Status/validity-tooling only; no PR-01..PR-06 content changed. The pre-merge #1166 branch manifest was unchanged (`b07721b846cbd74808cae156cebdbb93f4e6c140cf289aad217ea6cc06e07609`); the merged 1.0.4 manifest is `7e3984771249522314d360a617132609c741daae96b8d11cd1f2e8615121bbfe`. The executed corpus/workflow/script hashes are preserved; the MANIFEST/PR-06 entries were updated to the merged 1.0.4 hashes; post-run hardened script hashes are in `post_run_hardening` | Campaign lead (repository owner-directed research session) |

### 2.1 Hash history

The v1.0.0 hashes below remain the pre-outcome record of the original
freeze; they are superseded for current verification by the recomputed
v1.0.4 manifest. The v1.0.0 content is recoverable from commit
`62d9161b2992`; the v1.0.1 content is recoverable from its amendment-1
commit `0a18b07afb54`.

| Version | Date | `MANIFEST.sha256` | Note |
| :--- | :--- | :--- | :--- |
| 1.0.0 | 2026-11-18 | `df1cc1b084cf7f9312ffbdcf2e8e6afdc194a05c8551ac7b8cfca77e62c40450` | Original freeze commit `62d9161b2992`; superseded by amendment 1 |
| 1.0.1 | 2026-11-18 | `d4fa650cf09afd42c7b23718e36674a0a54fdce8d326f2b757df3c89cde691b8` | Amendment 1; superseded by amendment 2 |
| 1.0.2 | 2026-11-27 | `7aeb3ebb5f445d21141a4a0c64c191c66b13a2e689403b45e15ed9d9b07afb18` | Amendment 2 (`τ_FA` freeze); superseded by amendment 4 |
| 1.0.3 | 2026-11-28 | `1d3069043f53bdf13386b248392ba63ef35b296e46b65b764fac251d2e4b4ec9` | Amendment 4 (post-review erratum); superseded by amendment 5 |
| 1.0.3 | 2026-11-28 | `e3863f53fe0245b191a8215279623a8756f0b472e3a096e7f91ea3042f97eb8e` | Amendment 5 (PR-04 held-out caveat); superseded by the merged 1.0.4 |
| 1.0.4 | 2026-11-28 | `7e3984771249522314d360a617132609c741daae96b8d11cd1f2e8615121bbfe` | Merged #1163 1.0.3 (amendments 1–5) with #1166 amendment 6; current |

Original v1.0.0 artifact hashes:

| Artifact | v1.0.0 SHA-256 |
| :--- | :--- |
| PR-01 | `11050e1a647fdb1d9a10e4cc2156f8d1f8d252c0bb7b708caa6d7ac11f788178` |
| PR-02 | `56283ea01fb32fe59482c3cfe848680a8e582141df859141c3f0593fdf084f5e` |
| PR-03 | `501cb5719c181d844098863a98317e278ce79a2df0b051c8e384a337caee6377` |
| PR-04 | `503315297fd90494417ea7c2a086b9ce750103899931ba1f7e7416b9384bb73d` |
| PR-05 | `47d9d329e923286168568bd3f29819331da41135bd08de941a24503fb737d866` |
| PR-06 | `97a999624e7b45c71c77a79d3e3e24a17416fb6c76e3b76f9cbb5eec0b14d58e` |

v1.0.1 artifact hashes (amendment 1 manifest
`d4fa650cf09afd42c7b23718e36674a0a54fdce8d326f2b757df3c89cde691b8`):

| Artifact | v1.0.1 SHA-256 |
| :--- | :--- |
| PR-01 | `c7436213f7fa99f78703ca451f6766c52a8d8736247d90b0b7f89d5136b32e28` |
| PR-02 | `454a399a3576ae5a5ee6745d5f29bc572bb701f7a112007a7308ec8d1d2943da` |
| PR-03 | `f8335d8ef396f61e354186164c9e8d0281861fb52127d19c65ff30651d466b4a` |
| PR-04 | `cd3d25a2af3ac68af41fba0ea0509802bde1833f2d61c7a4b333578c6d1c75b7` |
| PR-05 | `9a65c632fcb8388fdc06b303578dad61ac413e603c8a2cbaa0ad0e6b08b1b5c5` |
| PR-06 | `ea88c0871804e30332372e3d835c86d8ae764a70e325fb872b76e9ce34b1354e` |

v1.0.2 artifact hashes (amendment 2 manifest
`7aeb3ebb5f445d21141a4a0c64c191c66b13a2e689403b45e15ed9d9b07afb18`):

| Artifact | v1.0.2 SHA-256 |
| :--- | :--- |
| PR-01 | `bb29396c52e5e63ff71103e62b2538b0666180f15316e0e9c0d02217582bcf13` |
| PR-02 | `b496f355e99bce7af4027cabe2c002627dc03229bf0390fe8ccc544a60e94bc7` |
| PR-03 | `fa31d4f49db20fa452920e4edd43bc6ad1a19482190c4b59c591db9eab1ce0a0` |
| PR-04 | `72263dc5faab2fa795b09153677bfd12452ef5f5b740880bc88e19806ff296c8` |
| PR-05 | `9a65c632fcb8388fdc06b303578dad61ac413e603c8a2cbaa0ad0e6b08b1b5c5` |
| PR-06 | `5b18570fd79bf027bdc7df2d36577d4cac71201c44ee500cbcac34567b95b4d4` |

v1.0.3 artifact hashes (amendment-5 manifest
`e3863f53fe0245b191a8215279623a8756f0b472e3a096e7f91ea3042f97eb8e`; the
amendment-4 v1.0.3 manifest
`1d3069043f53bdf13386b248392ba63ef35b296e46b65b764fac251d2e4b4ec9` had
PR-04 at `72263dc5…96c8`):

| Artifact | v1.0.3 SHA-256 |
| :--- | :--- |
| PR-01 | `97d67ec37efab4f8c112a355d02d457bbae1323b594eb5aa4624f8b3f39c69dc` |
| PR-02 | `bc592383742aa54dc5e6c9fdf99d29a82822a4f7eae1fea39821f28900112c3c` |
| PR-03 | `297d58bc9f60ec55796678c10f068ea65fadaee435bdec559ad3c8a9b2e61763` |
| PR-04 | `013a4acb5a3d3a276e08ea36d964f1f6cdeb5c3b13350883a66c06cf7a9a899c` |
| PR-05 | `9a65c632fcb8388fdc06b303578dad61ac413e603c8a2cbaa0ad0e6b08b1b5c5` |
| PR-06 | `25040bb1f0c83e506b8b4cfc6dc16b73f7a608c6f960992ecde79b41782e20b6` |


The #1166 campaign branch used a pre-merge 1.0.2 manifest
`b07721b846cbd74808cae156cebdbb93f4e6c140cf289aad217ea6cc06e07609`
(entries 6–10); it is not a mainline version and is superseded by 1.0.4.


v1.0.4 artifact hashes (merged manifest
`7e3984771249522314d360a617132609c741daae96b8d11cd1f2e8615121bbfe`;
#1163 1.0.3 plus #1166 amendment 6):

| Artifact | v1.0.4 SHA-256 |
| :--- | :--- |
| PR-01 | `0c346ee08b8623a73831ae12f5367dd1a8f08920044890c55c386118e78b5f30` |
| PR-02 | `aaf35ede5f634e07a071cd2f834f65c5a7710240388258530cb65fe818fda984` |
| PR-03 | `8869a366aebf0a3afa816e82ed74bce493f4c90f54c48a9d8c7fd76a6d206611` |
| PR-04 | `736fd94b4400ceb7bf3dee4601f32d6d93e6d43cf8c54077955f3f5ecdbd299e` |
| PR-05 | `9a65c632fcb8388fdc06b303578dad61ac413e603c8a2cbaa0ad0e6b08b1b5c5` (unchanged) |
| PR-06 | `94a29848281d55d4970de5c3929189a68389df5b05a3b414e2fd2d093993c5b8` |

## 3. Open values tracker (must be closed before data collection)

Every value below is intentionally `UNDECIDED` or `TBD` in the frozen
text. Closing one requires a pre-data amendment (a new entry in §2) that
records the value, its evidence, the date, and the approver. A campaign
cannot start with any of its rows still open.

### #1163

Closed by amendment 2 on 2026-11-27, before any held-out case was authored
or run:

| Closed value | Artifact | Frozen value |
| :--- | :--- | :--- |
| `τ_FA` (false-accept threshold) | PR-01 §5, PR-06 §C1 | `0.05` (exact Clopper–Pearson 95% interval; lower is better) |
| Truth-set size and per-code case counts | PR-02 §C1 | `N_min = 73` expected-reject cases + ≥30 expected-accept controls; full coverage rule in PR-02 §C1 |
| Non-verdict invalidity limit | PR-02 §C1 | 0 non-verdicts (any non-verdict ⇒ INVALID) |
| Measurement host | PR-03 §C1 | Single arm, campaign lead's macOS arm64 workstation, identity in the pre-run manifest |
| Wall-clock budget per case and total | PR-03 §C1 | 60 s/case, 7200 s total, static-only (no network) |

Still open:

| Open value | Artifact | Owner | Deadline |
| :--- | :--- | :--- | :--- |
| Independent reviewer sign-off | PR-05 C1 | Independent reviewer | Before a publication claim; PENDING |

### #1164

| Open value | Artifact | Owner | Deadline |
| :--- | :--- | :--- | :--- |
| `τ_R` (full-loop verified-repair threshold) | PR-01 §5, PR-06 §C2 | Campaign lead + independent reviewer | Before data collection |
| Exact model identifier (pilot recorded only "GLM via school proxy") | PR-03 §C2 | Campaign lead | Before data collection |
| Held-out confirmatory case set (hash-sealed) | PR-01 §C2, PR-03 §C2 | Campaign lead + custodian | Before data collection |
| Repeat count R and repeat policy | PR-01 §C2, PR-03 §C2 | Campaign lead | Before data collection |
| Measurement host(s) | PR-03 §C2 | Campaign lead + infrastructure owner | Before data collection |
| Total dollar and wall-clock budget | PR-03 §C2 | Campaign lead | Before data collection |
| Independent reviewer sign-off | PR-05 C2 | Independent reviewer | Before data collection |

### #1165

| Open value | Artifact | Owner | Deadline |
| :--- | :--- | :--- | :--- |
| `τ_EQ` (strict equivalence threshold) | PR-01 §5, PR-06 §C3 | Campaign lead + independent reviewer | Before data collection |
| Corpus composition, size, and per-origin-engine minimum | PR-02 §C3, PR-03 §C3 | Campaign lead | Before data collection |
| Repeats per arm R and run-order randomization seed | PR-02 §C3, PR-03 §C3 | Campaign lead | Before data collection |
| Per-case Nextflow/Snakemake build and container-digest pin (repository reference pins are Nextflow 25.10.4 and Snakemake 9.19.0 in `benches/comparative/README.md`) | PR-03 §C3 | Campaign lead | Before data collection |
| Zero-declared-output cases excluded before the run (each exclusion recorded) | PR-02 §C3, PR-06 §C3 | Campaign lead | Before data collection |
| Equivalence normalizer specification and hash | PR-01 §C3, PR-03 §C3 | Campaign lead | Before data collection |
| Measurement host | PR-03 §C3 | Campaign lead + infrastructure owner | Before data collection |
| Per-arm and total budget | PR-03 §C3 | Campaign lead | Before data collection |
| Independent reviewer sign-off | PR-05 C3 | Independent reviewer | Before data collection |

### #1166

| Open value | Artifact | Status |
| :--- | :--- | :--- |
| `n_min` and `τ_ARCH` per artifact class | PR-01 §5, PR-06 §C4 | **CLOSED 2026-11-28 (amendment 6):** `τ_ARCH = 0.80`, `n_min = 3` |
| aarch64 host identity | PR-03 §C4 | **CLOSED 2026-11-28 (amendment 6):** local Apple Silicon macOS host, Docker Desktop native linux/arm64 VM (no qemu) |
| Frozen workflow set and artifact count | PR-02 §C4, PR-03 §C4 | **CLOSED 2026-11-28 (amendment 6):** 2 workflows, 24 primary artifacts, 8 classes of 3; corpus manifest `scripts/cross-arch-provenance-governed-corpus.json` |
| Missingness invalidity limit | PR-02 §C4 | **CLOSED 2026-11-28 (amendment 6):** 0% |
| Compute-hour and storage budget | PR-03 §C4 | **CLOSED 2026-11-28 (amendment 6):** 2 hosts × 2 workflows, minutes of CPU and < 1 GiB storage; no cloud/paid calls |
| Independent reviewer sign-off | PR-05 C4 | **PENDING** — external publication-package gate. Per PR-05, no result may be described as pre-registered or confirmatory until this row is completed. The #1397 campaign report discloses this limitation and labels its category a governed decision under the frozen rule, not a publication-ready confirmatory result. It is not a campaign-validity condition in PR-02 or PR-06 §C4. |

**Protocol note (2026-11-28).** Amendment 6 closes the six
campaign-parameter values above. The independent reviewer sign-off is an
external publication gate, not a parameter this session is authorized to
self-sign; it remains visibly PENDING rather than fabricated. The campaign
is therefore run and reported as a governed decision under the frozen rule
with that limitation disclosed, and the preregistration README's honest
status ledger says exactly that.

## 4. Amendment entry template

Copy this block into §2 when closing a value or changing a frozen rule:

```text
| <n> | <YYYY-MM-DD> | <PR-id §section> | <old → new> | <reason> | <none / confirmatory outcome seen — describe> | <confirmatory / exploratory / invalid> | <name, role> |
```
