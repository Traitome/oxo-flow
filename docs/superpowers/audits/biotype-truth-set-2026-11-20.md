# BioType truth-set validation — broadened run, with its exclusions (#1163)

> **Status**: broadened truth set. This supersedes the 18-case first set in
> [`biotype-truth-set-2026-11-19.md`](biotype-truth-set-2026-11-19.md) as the
> current measured state, and replaces the *claim* "broad truth-set validation"
> with a *number*: a **42-case** set run through the real `oxo-flow validate
> --json` surface, yielding **25 true-accept, 0 false-accept, 17 true-reject, 0
> false-reject**, with a diagnostic-code cross-check of **42/42**. It is a
> *breadth* improvement over N=18 but is still **not** the full governed
> campaign: the pre-registration threshold `τ_FA` remains UNDECIDED, so no
> governed POSITIVE/NEGATIVE decision is claimed.
>
> **One defect surfaced and it is a DOC defect, not a checker defect.** Case
> `b18` was authored from the CRAM specification to expect a **reject** and it
> rejects — but the emitted behaviour contradicted the shipped reference text
> (`workflow-format.md` claimed "BAM and CRAM are BGZF by specification"). The
> checker is correct (only BAM implies BGZF), so the finding is a corrected doc
> row, not a code change; details in §7.

## 1. What was measured

The BioType checker (`.oxoflow` `[rules.types]` / `[types.*]` contracts) was
exercised against a 42-case truth set authored **independently of the checker**
and run through the real CLI validation surface, `oxo-flow validate <case>
--json`. For every case the *expected* verdict (accept / reject, and the
TYPE-* code set when reject) and a one-line rationale were recorded in a header
comment **before running**; the verdict and code set are then read mechanically
from the CLI JSON (`"valid"`, and the sorted set of `TYPE-NNN` codes), never
eyeballed.

Three case sources:

- **Fixture-modelled edges (6)** — `t01`–`t06`, hand-authored adversarial ports
  modelled on the format edges exercised by the Nextflow/Snakemake import
  fixtures. Only `t01` (STAR→subread/featureCounts) and `t06` (trimmomatic
  FASTQ-gz passthrough) track an edge literally present in
  `tests/fixtures/import/nextflow/rnaseq.ok.nf`; the rest are hand-authored
  format-analogue edges.
- **Adversarial boundary probes (29)** — `a06`–`a14` (9) hand-authored at the
  edges of individual TYPE codes, plus the **`b01`–`b20` breadth wave (20)**,
  authored from the published contract (the format list in the schema, the
  index/build/compression/sort semantics in `workflow-format.md`, and the
  TYPE-001–010 catalogue) rather than from the checker source.
- **Real repository examples (7)** — `examples/gallery/{13_simple_variant_calling,
  06_rnaseq_quantification,09_single_cell_rnaseq,07_wgs_germline,
  08_multiomics_integration,14_paired_experiment_control,16_16s_qiime2_amplicon}.
 oxoflow`, files **not written for the checker** (all untyped, so all expected
  accept); they prove the checker does not disturb the shipped example corpus.

The EXPECT table in the harness is the single mechanical source of truth, so the
matrix is computed, not transcribed.

## 2. Per-arm identity (ledger §8)

| Field | Value |
| :--- | :--- |
| hostname | `shixiangwangdeiMac.local` |
| `uname -srm` | `Darwin 25.6.0 arm64` |
| arch | `arm64` (Apple silicon) |
| rustc | `1.98.0 (88d9e12ae 2026-08-18)` |
| oxo-flow | `0.32.0`, built from the branch worktree |
| commit (branch HEAD at run) | `1dd70d4f94e3e95a1c66b3ba1179932a8b7e61bb` |
| branch | `research/1163-biotype-truth-set` |
| binary | `target/debug/oxo-flow` (debug build) |
| run timestamp (UTC) | `2026-11-19T18:18:11Z` |

Single arm; no cross-architecture contrast is claimed (unlike #1166). The
checker is platform-independent pure Rust, so a single arm is the correct design
for a *logic* truth set.

## 3. Exact commands

```bash
# build the CLI the harness drives
cargo build -p oxo-flow-cli

# run the whole truth set, print the 2x2 matrix AND write the machine-readable
# per-case result file (deliverable 4)
sh scripts/biotype-truth-set.sh target/debug/oxo-flow \
  docs/superpowers/audits/data/biotype-truth-set-results.json
```

Single-case form (what the harness runs per case):

```bash
oxo-flow validate tests/truthset/biotype/cases/b01_bcf_to_vcf_consumer.oxoflow --json
```

The harness `scripts/biotype-truth-set.sh` carries the EXPECT table
(`path <TAB> expected <TAB> codes <TAB> note`), drives every case, tallies the
2x2 cells and the codes cross-check, and exits 0 always — it is a
*measurement*, not a gate. When a second argument is given it also emits the
machine-readable result file (`{generated_at, commit, binary, cases:[…],
confusion_matrix, codes_correct, codes_fail, cases_total}`).

**Gate-scope disclosure.** `make docs` runs only
`RUSTDOCFLAGS="-D warnings" cargo doc --workspace --no-deps`; it does **not**
render the MkDocs site. This report, the ledger row, and the README bet are
plain Markdown and are therefore **not render-checked** by `make docs` — they
are kept consistent by hand and by the drift tripwires, not by a rendering gate.

## 4. Acceptance criterion — pre-registered, operational, not the governed threshold

**This report's criterion is an OPERATIONAL sanity criterion, NOT the campaign's
governed decision threshold.** The campaign's publication decision is governed
by the frozen pre-registration artifacts; PR-06 §C1 states, verbatim:

> **Threshold.** `τ_FA` is **UNDECIDED** (owner: campaign lead + independent
> reviewer; deadline: before data collection). It is the maximum acceptable
> false-accept proportion on expected-reject cases.

with a **CI-based** decision rule (POSITIVE iff the exact 95% upper bound
`U ≤ τ_FA`; NEGATIVE iff `L > τ_FA`; otherwise INCONCLUSIVE). Because `τ_FA`
was never fixed before data collection, this report **cannot and does NOT claim
a governed POSITIVE/NEGATIVE/INCONCLUSIVE decision**.

**Pre-registered operational criterion (frozen before this run, same criterion
as the 18-case report so it cannot be fitted to the result).**

- **Hard — ZERO false-accepts.** A false-accept means a typed consumer edge with
  a stated, checkable requirement was silently accepted with **no diagnostic**.
  Any non-zero false-accept ⇒ the set FAILED this criterion.
- **Soft — false-reject ≤ 1 case.** More than one is treated as a defect
  cluster.

**Result: 0 false-accepts, 0 false-rejects — the operational criterion PASSES.**

**Scope caveat filed up front.** Even a perfect 42-case matrix does not by
itself clear the full WF-04 exit criterion (see §9 for the excluded
constructs); the criterion governs *correctness of this set*, not roadmap
closure.

### 4.1 Exact 95% confidence intervals (PR-06 §C1 requires the CI)

Clopper-Pearson exact two-sided 95% intervals (same construction as R
`binom.test`), level 0.95 (alpha = 0.05, two-sided). With zero events, the
one-sided upper bounds are reported as the two-sided interval's upper limit:

| Endpoint | Count | Point estimate | Exact 95% CI (Clopper-Pearson) |
| :--- | :--- | :--- | :--- |
| False-accept rate `FA/(FA+TR)` | 0/17 | 0.0% | **[0.0%, 19.5%]** |
| False-reject rate `FR/(FR+TA)` | 0/25 | 0.0% | **[0.0%, 13.7%]** |

Computation (false-accept, x = 0, n = 17): upper = `Beta.ppf(0.975, 1, 17)`
= 0.1950 → **[0.0%, 19.5%]**. False-reject (x = 0, n = 25): upper =
`Beta.ppf(0.975, 1, 25)` = 0.1372 → **[0.0%, 13.7%]**. The upper bounds are
still wide (N = 17 expected-reject cases), which is itself the point PR-06 §0.1
step 5 makes: a zero count bounds the true rate only loosely, and even a
0-false-accept result with U ≈ 19.5% cannot decide a governed verdict without a
fixed `τ_FA`.

## 5. Confusion matrix

```
  2x2 confusion matrix (rows = expected, cols = actual) — VERDICT scoring:
                     accept   reject
    EXPECT-ACCEPT    25       0
    EXPECT-REJECT    0        17

  true-accept=25  false-accept=0  true-reject=17  false-reject=0
```

42/42 cases (100%) match their pre-registered verdict.

**Diagnostic-code cross-check (expected-code set == actual-code set, asserted):**
`codes-correct=42  codes-fail=0  (of 42)`. The ACTUAL code set is the union of
the `TYPE-NNN` codes seen on `validate --json` and on `lint --json`, compared
for **set-equality** against the EXPECT table (the lint surface is load-bearing
because the advisory codes `TYPE-008`/`009`/`010` are filtered out of
`validate --json`). No case is verdict-pass/codes-fail — every expected code set
matches the actual set exactly.

**Verdict vs the operational criterion: PASS (0 false-accepts, 0 false-rejects).**
This is an operational pass on *this* set, not a governed campaign verdict
(§4), and the breadth boundary in §9 still applies.

## 6. Per-case summary (42 cases)

Verdicts are read from the CLI JSON; `codes` is the asserted code set. Full
per-case rows (verdict, expected-codes, actual-codes, note) are in the
machine-readable result file
[`data/biotype-truth-set-results.json`](data/biotype-truth-set-results.json).

### 6.1 Fixture-modelled edges and early adversarial probes (t01–t06, a06–a14)

| case | expected | actual | codes | correct? |
| :--- | :---: | :---: | :--- | :---: |
| `t01_star_bam_to_featurecounts` | accept | accept | — | yes |
| `t02_bwa_sam_to_sorted_bam_consumer` | reject | reject | TYPE-001,TYPE-002 | yes |
| `t03_bcftools_oz_to_indexed_consumer` | accept | accept | — | yes |
| `t04_gzip_vcf_to_tabix_consumer` | reject | reject | TYPE-003,TYPE-006 | yes |
| `t05_cram_hg19_to_grch38_consumer` | reject | reject | TYPE-004 | yes |
| `t06_fastq_gz_passthrough` | accept | accept | — | yes |
| `a06_queryname_bam_to_coordinate_consumer` | reject | reject | TYPE-002 | yes |
| `a07_bam_no_readgroup_to_bqsr` | reject | reject | TYPE-005 | yes |
| `a08_multichannel_default_mismatch` | accept | accept | TYPE-010 | yes |
| `a09_multichannel_namekey_default` | reject | reject | TYPE-002 | yes |
| `a10_build_alias_hg38_grch38` | accept | accept | — | yes |
| `a11_multichannel_namekey_mismatch` | accept | accept | TYPE-010 | yes |
| `a12_bam_implicit_bgzip` | accept | accept | — | yes |
| `a13_somatic_to_germline_consumer` | reject | reject | TYPE-007 | yes |
| `a14_typed_consumer_untyped_producer` | accept | accept | TYPE-008 | yes |

### 6.2 Breadth wave b01–b20 (authored from the published contract)

| case | expected | actual | codes | correct? |
| :--- | :---: | :---: | :--- | :---: |
| `b01_bcf_to_vcf_consumer` | reject | reject | TYPE-001 | yes |
| `b02_cram_to_bam_consumer` | reject | reject | TYPE-001 | yes |
| `b03_fasta_fai_consumer` | reject | reject | TYPE-003 | yes |
| `b04_gtf_accept` | accept | accept | — | yes |
| `b05_bed_build_match` | accept | accept | — | yes |
| `b06_bed_to_gtf_consumer` | reject | reject | TYPE-001 | yes |
| `b07_bam_bai_to_csi_consumer` | reject | reject | TYPE-003 | yes |
| `b08_build_alias_mm39_grcm39` | accept | accept | — | yes |
| `b09_build_mismatch_grch37_grch38` | reject | reject | TYPE-004 | yes |
| `b10_untyped_pure_accept` | accept | accept | — | yes |
| `b11_untyped_bed_bam_accept` | accept | accept | — | yes |
| `b12_multichannel_paired_accept` | accept | accept | — | yes |
| `b13_germline_to_somatic_consumer` | reject | reject | TYPE-007 | yes |
| `b14_coordinate_to_queryname_consumer` | reject | reject | TYPE-002 | yes |
| `b15_bgzip_vcf_both_sides_accept` | accept | accept | — | yes |
| `b16_named_types_build_mismatch` | reject | reject | TYPE-004 | yes |
| `b17_genuinely_unpaired_advisory` | accept | accept | TYPE-010 | yes |
| `b18_cram_implied_bgzip` | reject | reject | TYPE-006 | yes |
| `b19_build_incomplete_advisory` | accept | accept | TYPE-009 | yes |
| `b20_unsorted_consumer_accept` | accept | accept | — | yes |

Newly covered by the breadth wave (constructs the 18-case set did not drive):
the **BED**, **FASTA**, **GTF**, and **BCF** formats; the named `[types.<name>]`
table resolution path (`b16`, lowercase-`format` enum); the `index=bai`→`csi`
index-kind boundary (`b07`); the `mm39`↔`GRCm39` build alias (`b08`); the
untyped/opt-out path on an error-prone BED+BAM edge (`b10`, `b11`); a genuine
multi-channel **paired** accept control (`b12`) distinct from the advisory
`TYPE-010` shapes (`b17`); the advisory `TYPE-009` build-incomplete edge
(`b19`); and the consumer `sort_order=unsorted` no-constraint path (`b20`).

### 6.3 Real repository examples (7)

| case | expected | actual | correct? |
| :--- | :---: | :---: | :---: |
| `examples/gallery/13_simple_variant_calling` | accept | accept | yes |
| `examples/gallery/06_rnaseq_quantification` | accept | accept | yes |
| `examples/gallery/09_single_cell_rnaseq` | accept | accept | yes |
| `examples/gallery/07_wgs_germline` | accept | accept | yes |
| `examples/gallery/08_multiomics_integration` | accept | accept | yes |
| `examples/gallery/14_paired_experiment_control` | accept | accept | yes |
| `examples/gallery/16_16s_qiime2_amplicon` | accept | accept | yes |

## 7. The one defect surfaced — a DOCUMENTATION defect (b18)

`b18_cram_implied_bgzip` was authored from the **CRAM specification**: a CRAM
container carries block-level gzip/bzip2/LZMA framing with no BGZF `BC` extra
field, so CRAM is **not** BGZF and a plain CRAM cannot satisfy a consumer's
`compression=bgzip` requirement. The case was therefore pre-registered to
**reject** with `TYPE-006`. It does reject with `TYPE-006` — but that result
contradicted the shipped reference text, which claimed:

> BAM and CRAM are BGZF by specification, so a producer declaring a plain
> `bam`/`cram` already satisfies a consumer's `compression=bgzip` requirement …

The **checker is correct**: `BioTypeSpec::implied_compression`
(`crates/oxo-flow-core/src/types/mod.rs`) implies BGZF for **BAM only** (CRAM →
`None`), and this is deliberate and unit-tested
(`crates/oxo-flow-core/src/types/negative_tests.rs`,
`cram_does_not_imply_bgzip`). The defect was in the **doc**, which overstated the
CRAM case. Per the brief's deliverable 5, the honest disposition is: **no
checker defect to fix, no new issue to file** — the doc row was corrected in
this change to name **BAM only** and to state explicitly that CRAM is not BGZF
(`docs/guide/src/reference/workflow-format.md`, the `compression` field row).

This is reported as a finding, not absorbed as noise: an independently authored
case is what caught a wrong claim in the reference text.

> Note: the 18-case report's §7.3 recorded the *former* `a12` false-reject with
> the rationale "BAM/CRAM ⇒ BGZF". That rationale was correct for BAM (the case
> that mattered) but its CRAM clause was the same overstatement now corrected
> here; the `a12` fix (#1255) is unaffected because `implied_compression` is
> BAM-only by design.

## 8. Verdict

- **Correctness of this 42-case set vs the pre-registered operational
  criterion: PASS** (0 false-accepts, 0 false-rejects; codes 42/42). Not a
  governed verdict: `τ_FA` is UNDECIDED in the frozen pre-registration
  (PR-06 §C1), so no POSITIVE/NEGATIVE/INCONCLUSIVE decision is claimed.
- **Breadth:** 2.3× the first set (42 vs 18 cases) and it now drives BED, FASTA,
  GTF, BCF, the named-table path, the index-kind boundary, and the advisory
  TYPE-009 path — but it is still not the full corpus (§9).
- **Defects surfaced:** one, and it is a **documentation** defect (§7),
  corrected in this change. No checker defect was exposed by the breadth wave,
  so no checker fix and no new issue were required.
- **Roadmap:** WF-04's "broad truth-set validation" exit criterion moves from
  UNMET (N=18) toward met (N=42, 0/0, exclusions enumerated) but stays gated on
  the governed `τ_FA`, so the label is not closed outright. See the ledger
  update and §9.

## 9. Honesty boundary — exclusions and what this does NOT measure

Reported with its exclusions, as the exit gate requires:

- **Excluded formats.** The `Generic` short-circuit format and the `gff3`,
  `h5ad`, `rds`, `parquet`, and `tsv` format enum variants are **not driven** by
  any case. Formats driven: BAM, SAM, CRAM, VCF, BCF, BED, FASTA, GTF, FASTQ.
- **Excluded diagnostics.** Every TYPE-001–010 code is exercised except none —
  all of TYPE-001 (b01/b02/b06), 002 (t02/a06/b14), 003 (t04/b03/b07), 004
  (t05/b09/b16), 005 (a07), 006 (t04/b18), 007 (a13/b13), 008 (a14), 009
  (b19), and 010 (a08/a11/b17) have at least one case. `TYPE-UNKNOWN`
  resolution is only touched indirectly.
- **Excluded constructs.** No case uses `transform.*` scatter/combine generated
  rules, `[[include]]`-contributed types, or `[ai]`; the truth set is
  single-document, hand-authored DAGs.
- **~~Still no `[types.*]` usage in the repo.~~** **Closed (2026-11-20, #1272).**
  A real, complete, runnable gallery pipeline now exists with a
  `[types.*]` / `[rules.types]` contract on every crossing edge:
  `examples/gallery/17_typed_variant_calling.oxoflow`. It was accepted by the
  real admission surface and shown load-bearing (a one-sided edit fails
  `TYPE-001`); see [`typed-pipeline-evidence-2026-11-20.md`](typed-pipeline-evidence-2026-11-20.md).
  The 42-case run *below* still rests on hand-authored cases for breadth; that
  addendum closes only this "no real typed pipeline" clause, not the breadth
  boundary.
- **Single arm, single toolchain** (macOS arm64, rustc 1.98.0). No
  cross-platform or cross-version contrast intended or performed.
- **Not the governed campaign.** `τ_FA` remains UNDECIDED (PR-06 §C1), so even
  this 42-case, 0/0 result cannot be scored POSITIVE/NEGATIVE/INCONCLUSIVE; the
  exact 95% upper bound on the false-accept rate is still ≈19.5% at N=17.
- **The examples were validated, not executed** — no real data was staged and no
  bioinformatics tool was run.

## 10. Raw-data lineage

- Cases: `tests/truthset/biotype/cases/{t01..t06,a06..a14,b01..b20}*.oxoflow`
  (each with its pre-registered EXPECT in a header comment).
- Harness: `scripts/biotype-truth-set.sh` (EXPECT table + matrix printer +
  machine-readable JSON emitter).
- Machine-readable result file:
  `docs/superpowers/audits/data/biotype-truth-set-results.json`
  (`{generated_at, commit, binary, cases:[…], confusion_matrix, codes_correct,
  codes_fail, cases_total}`).
- Real examples: `examples/gallery/*.oxoflow` (7 files listed in §6.3).
- Full matrix reproduced by re-running the harness at commit `1dd70d4f`.

## 11. References

- `docs/superpowers/audits/biotype-truth-set-2026-11-19.md` — the 18-case first
  set and the three defects it found (two false-accepts, one false-reject) plus
  their #1254/#1255 fixes.
- `docs/superpowers/preregistration/PR-06-publication-decision-rule.md`
  (v1.0.1, FROZEN 2026-11-18) §C1 — the campaign decision rule and the `τ_FA`
  UNDECIDED threshold.
- `docs/superpowers/preregistration/PR-01-outcomes-and-statistical-models.md`
  §C1 and §5 — primary/secondary endpoints, exact-CI model, `τ_FA` row.
- `docs/guide/src/reference/workflow-format.md` — the `compression` field row
  corrected by this change.
