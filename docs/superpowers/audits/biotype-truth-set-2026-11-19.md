# BioType truth-set validation — first measured confusion matrix (#1163)

> **Status**: first measured truth set. This replaces the *claim* "broad
> truth-set validation" with a *number*: an 18-case set run through the real
> `oxo-flow validate --json` surface, yielding **8 true-accept, 2 false-accept,
> 7 true-reject, 1 false-reject**. It is **not** a broad campaign and it does
> **not** clear the WF-04 exit criterion. Two false-accepts and one false-reject
> are reported as defects (below), not absorbed as noise.

## 1. What was measured

The BioType checker (`.oxoflow` `[rules.types]` / `[types.*]` contracts) was
exercised against an 18-case truth set authored **independently of the checker**
and run through the real CLI validation surface, `oxo-flow validate <case>
--json`. For every case the *expected* verdict (accept / reject, the TYPE-* code
when reject) and a one-line rationale were **recorded in a header comment before
running**; the verdict is then read mechanically from the CLI JSON (`"valid"`,
and the sorted set of `TYPE-NNN` codes), never eyeballed.

Three case sources:

- **Transplanted real edges (6)** — `t01`–`t06`, ports of the format edges in
  the Nextflow/Snakemake import fixtures (`tests/fixtures/import/**`), each
  argued from the upstream tool contract (STAR→featureCounts, bwa-mem2→MarkDuplicates,
  `bcftools -Oz`→tabix, gzip-VCF→bgzip+tabix, CRAM hg19→GRCh38, trimmomatic→salmon).
- **Adversarial boundary probes (9)** — `a06`–`a14`, hand-authored at the edges
  of individual TYPE codes (sort order, reference build, read groups, channel
  matching, compression-invariant formats, attribute contracts, the advisory
  TYPE-008 path), each expected verdict argued from the format/diagnostic
  contract rather than from current checker behavior.
- **Real repository examples (3)** — `examples/gallery/{13_simple_variant_calling,
  06_rnaseq_quantification,09_single_cell_rnaseq}.oxoflow`, files **not written
  for the checker** (all untyped, so all expected accept); they prove the
  checker does not disturb the shipped example corpus.

Authoring/EXPECT freeze happened in the prior working session; the EXPECT table
is the single mechanical source of truth in the harness (Section 3), so the
matrix is computed, not transcribed.

## 2. Per-arm identity (ledger §8)

| Field | Value |
| :--- | :--- |
| hostname | `shixiangwangdeiMac.local` |
| `uname -srm` | `Darwin 25.6.0 arm64` |
| arch | `arm64` (Apple silicon) |
| rustc | `1.98.0 (88d9e12ae 2026-08-18)` |
| oxo-flow | built from the worktree at `b7be10ba` |
| commit (worktree HEAD) | `b7be10ba844df04b51299264184c207cca95286c` |
| branch | `research/1163-biotype-truth-set` |
| binary | `target/debug/oxo-flow` (debug build) |
| identity capture (UTC) | `2026-11-19T01:16:43Z` |

Single arm; no cross-architecture contrast is claimed (unlike #1166). The
checker is platform-independent pure Rust, so a single arm is the correct design
for a *logic* truth set.

## 3. Exact commands

```bash
# build the CLI the harness drives
cargo build -p oxo-flow-cli

# run the whole truth set + print the 2x2 matrix (repeatable)
bash scripts/biotype-truth-set.sh
```

Single-case form (what the harness runs per case):

```bash
oxo-flow validate tests/truthset/biotype/cases/a12_bam_implicit_bgzip.oxoflow --json
```

The harness `scripts/biotype-truth-set.sh` carries the EXPECT table
(`path <TAB> expected <TAB> codes <TAB> note`), drives every case, tallies the
2x2 cells, and exits 0 always — it is a *measurement*, not a gate.

## 4. Pre-registered acceptance threshold

**Frozen before the matrix was computed.** Pre-registration timestamp:
`2026-11-19T01:13:08Z`.

The threshold is stated first so it cannot be fitted to the result:

- **Hard criterion — ZERO false-accepts.** A false-accept means a typed consumer
  edge with a stated, checkable requirement (e.g. BAM must be coordinate-sorted)
  was silently accepted with **no diagnostic**. For a contract checker a silent
  skip is a correctness defect: the user declared a guarantee and received no
  signal that it is unverified. Any non-zero false-accept ⇒ campaign verdict
  **FAIL**.
- **Soft criterion — false-reject ≤ 1 case of 18.** A false-reject (over-strict
  rejection of a valid edge) degrades usability but is fail-safe. More than one
  is treated as a defect cluster.
- **Scope caveat filed up front.** A single 18-case set cannot by itself clear
  "broad truth-set validation"; even a perfect matrix would leave the WF-04
  exit criterion (breadth) **unmet**. The threshold governs *correctness of this
  set*, not roadmap closure.

## 5. Per-case table

Expected verdict and rationale were recorded before running; ACTUAL is read from
the CLI JSON. `codes` = sorted unique `TYPE-NNN` seen in the output.

| case | expected | actual | codes | correct? | rationale (argued from the format/rule contract) |
| :--- | :---: | :---: | :--- | :---: | :--- |
| `t01_star_bam_to_featurecounts` | accept | accept | — | yes | STAR `SortedByCoordinate` + `bai` satisfies featureCounts' coordinate-sorted BAM. |
| `t02_bwa_sam_to_sorted_bam_consumer` | reject | reject | TYPE-001,TYPE-002 | yes | bwa-mem2 emits SAM (not BAM); consumer needs coordinate BAM — both the format and the sort requirement are violated. |
| `t03_bcftools_oz_to_indexed_consumer` | accept | accept | — | yes | `bcftools -Oz` yields bgzip VCF + `.tbi`; the `compression=bgzip,index=tbi` requirement is met. |
| `t04_gzip_vcf_to_tabix_consumer` | reject | reject | TYPE-003,TYPE-006 | yes | plain-gzip VCF is not BGZF (no random access) and has no `.tbi`; both the compression and index requirements are violated. |
| `t05_cram_hg19_to_grch38_consumer` | reject | reject | TYPE-004 | yes | CRAM reference build `hg19` (GRCh37) ≠ consumer's `GRCh38`; a build conflict. |
| `t06_fastq_gz_passthrough` | accept | accept | — | yes | trimmomatic→salmon both sides FASTQ[gzip]; format and compression unchanged. |
| `a06_queryname_bam_to_coordinate_consumer` | reject | reject | TYPE-002 | yes | queryname-sorted BAM is not coordinate-sorted. |
| `a07_bam_no_readgroup_to_bqsr` | reject | reject | TYPE-005 | yes | a `[types.rg_required]` BAM with no `@RG` violates a required read-group contract. |
| `a08_multichannel_default_mismatch` | reject | **accept** | — | **NO** | producer types `a.bam`=BAM and `b.vcf`=VCF (path-keyed); consumer reads `a.bam` under key `default` requiring BAM[coordinate]. The BAM is unsorted → TYPE-002 expected. **Silent skip = false-accept.** |
| `a09_multichannel_namekey_default` | reject | reject | TYPE-002 | yes | control for `a08` with the consumer keyed by the same path (`"a.bam"`); the SAME check fires. Isolates the defect to channel *pairing*, not the type logic. |
| `a10_build_alias_hg38_grch38` | accept | accept | — | yes | `hg38` is an alias of `GRCh38`; the builds agree. |
| `a11_multichannel_namekey_mismatch` | reject | **accept** | — | **NO** | producer names channels `aligned`=BAM,`calls`=VCF; consumer keys the literal path `a.bam` under `reads` requiring BAM[coordinate]. Different channel names → no pair → **silent skip = false-accept.** |
| `a12_bam_implicit_bgzip` | accept | **reject** | TYPE-006 | **NO** | a conformant BAM **is** BGZF by spec; requiring `compression=bgzip` on a BAM is redundant-but-true. Rejecting it is **over-strict (false-reject).** |
| `a13_somatic_to_germline_consumer` | reject | reject | TYPE-007 | yes | somatic VCF (`variant_type=somatic`) fed to a germline joint-genotyping consumer (`variant_type=germline`); attribute contract violated. |
| `a14_typed_consumer_untyped_producer` | accept | accept | — | yes | TYPE-008 is declared advisory; a typed consumer reading an untyped producer must NOT fail validation. |
| `examples/gallery/13_simple_variant_calling` | accept | accept | — | yes | real repo example (untyped); the checker must not disturb it. |
| `examples/gallery/06_rnaseq_quantification` | accept | accept | — | yes | real repo example (untyped). |
| `examples/gallery/09_single_cell_rnaseq` | accept | accept | — | yes | real repo example (untyped). |

## 6. Confusion matrix

```
                   actual
                accept   reject
  expected
    accept        8        1      (false-reject = 1)
    reject        2        7      (false-accept = 2)
```

`true-accept=8  false-accept=2  true-reject=7  false-reject=1`

15/18 cases (83%) match their pre-registered verdict; 3 do not.

**Campaign verdict vs the pre-registered threshold: FAIL (hard criterion).**
The hard criterion was zero false-accepts; the set produced **2**, so this set
does not pass. This is a measured failure of the checker, which is exactly what
an independent truth set exists to find.

## 7. Enumerated false cells (defect reports)

### 7.1 `a08_multichannel_default_mismatch` — FALSE-ACCEPT (TYPE-002 missing)

Minimal repro (`validate` returns `valid: true`, `errors: []`, `warnings: []`):

```toml
[[rules]]
name = "make"
output = ["a.bam", "b.vcf"]
[rules.types]
outputs = { "a.bam" = "BAM", "b.vcf" = "VCF" }

[[rules]]
name = "use"
input = ["a.bam"]
[rules.types]
inputs = { default = "BAM[sort_order=coordinate]" }
```

The producer typed `a.bam` as (unsorted) `BAM`; the consumer requires it
coordinate-sorted. Root cause: channel pairing (`crates/oxo-flow-core/src/types/checker.rs`
lines 353–372). `out_ch="a.bam"` vs `in_ch="default"`: not identical, the
`consumer.inputs.len()==1` rule needs `out_ch=="default"`, the
`producer.outputs.len()==1`/shared-path rule needs exactly one output each side
(here two), the cross-reference rule needs `out_ch`∈consumer's raw inputs
(`a.bam` — the raw input path is `a.bam`, but `out_ch` is the *channel key*
`a.bam` and `producer_rule.output` is `a.bam`/`b.vcf`, so this branch is about
`in_ch`… net: no branch matches) → **no pair forms → no diagnostic → silent
accept.** There is no "unpaired typed channel ⇒ advisory" fallback, so a typed
edge that produces nothing is indistinguishable from an untyped edge.

### 7.2 `a11_multichannel_namekey_mismatch` — FALSE-ACCEPT (TYPE-002 missing)

Same root cause, second surface. Producer channels named `aligned`/`calls`;
consumer channel named `reads` on the literal path `a.bam`. `out_ch="aligned"`
vs `in_ch="reads"`: not identical; neither is `default`; both sides are
multi-typed so the single/single fallback is off; cross-reference needs the
channel keys to equal the opposite rule's raw paths, which they do not. → no
pair → silent accept. `a09` is the passing control (same producer, consumer key
`"a.bam"`): identical logic, correct TYPE-002 — proving the type comparison is
right and only *channel resolution* is at fault.

**Fix direction (NOT applied here).** The brief permits only a trivial
one-liner; the correct fix is not trivial and risks changing pairing semantics
for legitimate multi-output workflows, so it is deferred. Candidate: when a
typed edge yields *zero* matched channel pairs (and neither side is untyped),
emit an advisory `TYPE-UNKNOWN`/`TYPE-008`-class "could not pair typed channels"
diagnostic instead of silence — closing the silent-skip class without
over-rejecting. Tracked as a #1163 follow-up.

### 7.3 `a12_bam_implicit_bgzip` — FALSE-REJECT (TYPE-006 over-strict)

Minimal repro (`validate` exits 1):

```
TYPE-006: upstream rule 'align' (default -> default): block compression
mismatch: consumer requires bgzip, but producer provides none
```

Producer declares plain `BAM`; consumer requires `BAM[compression=bgzip]`. Since
every conformant BAM is BGZF, the requirement is always satisfied for a valid
BAM — yet the checker compares the *declared* compression tags and rejects when
the producer is silent. Root cause: `is_subtype_of` TYPE-006
(`crates/oxo-flow-core/src/types/checker.rs` ~lines 220–244) gates on the
consumer's expectation being non-`None` but does not treat the format's own
invariant (`BAM`/`CRAM` ⇒ BGZF) as satisfying `compression=bgzip`. Fail-safe
direction (over-reject, blocks a valid workflow) but still a usability defect:
the natural way to write "this BAM edge" is punished for not restating an
invariant. **Fix direction (NOT applied):** derive implied compression from the
format before comparison — a format-invariant satisfaction, not a special case.

## 8. Verdict

- **Correctness of this set: FAIL** (2 false-accepts; hard criterion was 0).
- **Fail-safe direction:** the false-reject (1) is the safe direction; the two
  false-accepts are the material finding — they are the exact failure mode WF-04
  names ("BioType validation can still false-accept").
- **The checker's *type logic* is sound:** every case where channels pair
  (`a09` control, `a06`–`a07`, `a10`, `a13`, all `t0*`) matches its expectation,
  including alias/build normalization (`a10`) and the advisory TYPE-008 policy
  (`a14`). The defects localize to **channel resolution** (silent skip) and one
  **format-invariant compression** comparison.
- **Roadmap:** WF-04 stays **Planned / blocked**-adjacent as an *exit criterion*
  — a first measured set now exists, but it is small and it failed its own hard
  criterion, so "broad truth-set validation" remains unfinished. See the ledger
  update and Section 9.

## 9. Honesty boundary — what this does NOT measure

- **Not broad.** 18 cases; 6 transplanted, 9 hand-authored adversarial, 3 real
  examples. This is a *first* truth set, not a corpus. No statistical coverage
  claim; no false-positive *rate* is estimable from N=18.
- **No `[types.*]` usage in the repo.** A grep confirms no shipped `.oxoflow`
  in `examples/`/`tests/`/`docs/` declares `[rules.types]`/`[types.*]`, so the
  typed-correctness evidence rests entirely on transplanted + hand-authored
  cases, not on real typed pipelines.
- **Single arm, single toolchain** (macOS arm64, rustc 1.98.0). No
  cross-platform or cross-version contrast intended or performed.
- **One type-check entry point only.** Only `validate` was used as the surface;
  `run`/`dry-run` preflight share `validate_workflow_types` but were not driven
  here. Advisory-vs-fatal policy was checked for TYPE-008 (`a14`, accept) via
  `validate`; TYPE-009 was **not** exercised by a case.
- **Formats covered:** BAM, SAM, CRAM, VCF, FASTQ (and gvcf alias by code
  reading only). BED, FASTA, GTF, BCF and the `Generic` short-circuit path were
  **not** driven.
- **No fix applied.** Both false-accepts and the false-reject are reported only;
  the brief allows a trivial one-liner and neither defect qualifies.
- **The examples were validated, not executed** — `missing_inputs` were reported
  (4 fastq each for the two rnaseq examples) but no real data was staged.

## 10. Raw-data lineage

- Cases: `tests/truthset/biotype/cases/{t01..t06,a06..a14}*.oxoflow` (each with
  its pre-registered EXPECT in a header comment).
- Harness: `scripts/biotype-truth-set.sh` (EXPECT table + matrix printer).
- Real examples: `examples/gallery/{13_simple_variant_calling,
  06_rnaseq_quantification,09_single_cell_rnaseq}.oxoflow`.
- Checker source read for root-cause: `crates/oxo-flow-core/src/types/checker.rs`
  (channel pairing ~353–372; `is_subtype_of` TYPE-006 ~220–244),
  `crates/oxo-flow-core/src/format.rs` (~1146–1170 bridge),
  `crates/oxo-flow-cli/src/commands/quality.rs` (`validate` surface).
- Raw CLI JSON for the three false cells captured inline in Section 7; full
  matrix reproduced by re-running the harness at commit `b7be10ba`.
