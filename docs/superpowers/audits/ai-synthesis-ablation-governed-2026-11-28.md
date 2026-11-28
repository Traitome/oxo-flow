# AI synthesis three-arm ablation — governed pre-registered execution (2026-11-28)

**Status: pre-registered execution on a development intent set.**
The 36 intents are **not held out** — they were used by the prior
non-governed run (2026-11-26). This report therefore executes a frozen
protocol on the same development set; it is not a held-out confirmatory
campaign and makes no publication, peer-review, or generalization claim.
No independent reviewer sign-off exists; this is not the governed
#1163–#1166 Gate-3 process.

- **Issue:** [#1395](https://github.com/Traitome/oxo-flow/issues/1395)
- **Pre-registration:** [`eval/frontier/GOVERNED_CAMPAIGN_PRE_REGISTRATION_2026-11-28.md`](../../../eval/frontier/GOVERNED_CAMPAIGN_PRE_REGISTRATION_2026-11-28.md),
  commit `471f38cc`
- **Harness commit:** `f4b6c979` (`eval/frontier/run_eval.py` SHA-256
  `289e6ec956353ab0d1fc313343039f1e4f828a3a833156893a327f2ec16be8df`)
- **Analysis script:** `eval/frontier/statistical_report.py` SHA-256
  `e0471ee640cfc45d0d4074fdf600e8b193543d22d5f3ebe5693568933835594a`
  (the harness-commit version; exact McNemar + Holm primary, Edwards +
  Holm sensitivity — see §2.1)
- **Campaign run:** `eval/frontier/results/governed-2026-11-28/` (gitignored)

## 1. Executive summary

The campaign completed all **324/324** cells (36 intents × 3 arms × 3
seeds). Intention-to-treat (ITT) over the frozen all-36 population:

| Arm | Success / cells | pass@1 | 95% bootstrap CI |
| :--- | :---: | :---: | :---: |
| A `unconstrained` | 0 / 108 | **0.0%** | [0.0%, 0.0%] |
| B `syntax-only` | 92 / 108 | **85.2%** | [78.7%, 91.7%] |
| C `full-contract` | 49 / 108 | **45.4%** | [36.1%, 54.6%] |

Under the pre-registered primary inference (exact McNemar + Holm;
Edwards + Holm sensitivity), **B > A** (exact p = 4.04e-28; Edwards
p = 2.41e-21), **B > C** (exact p = 1.60e-11; Edwards p = 9.03e-10),
and **C > A** (exact p = 3.55e-15; Edwards p = 7.07e-12). The ITT
direction is therefore the same as the prior non-governed run: A worst,
B above C.

**The B > C ITT result is not a contract-feedback effect.** All 59 C
failures are zero-token provider transport failures in one contiguous
window (**2026-11-28T00:04:48Z → 00:12:39Z**; every failure ~7.1 s,
`AI request failed after 3 attempts: error sending request`, endpoint
redacted). C's last success started at 23:59:46Z; every C cell after it
failed before any generation. C's 49 cells that actually reached the
model all produced a gate-passing workflow. The harness records only
`error: "template exited 1"`, which hides the provider cause; the AI
session archive carries it.

**Gate success is not loop completion.** The 49 C gate successes are
**39 Session-Success + 10 Session-Failed**: 9 exhausted the
correction-round budget (`exceeded max rounds (10) without valid
output`) after D004/D003 reference-path or binary-not-found warnings,
and 1 ended in a provider transport error after producing the workflow.
All 10 carry the CLI warning "Generated pipeline reached its
correction-round budget". The post-hoc session-level matched comparison
is **B 48/48 vs C 39/48** (exact p = 0.00390625; Edwards p = 0.00766),
so the gate-level p = 1.0 is not a loop-completion null.

In the post-hoc **per-protocol matched** analysis (48 cells where both B
and C actually completed), the gate-level counts are B 47/48 (97.9%) and
C 48/48 (100%); the sole gate-level discordant cell is
`somatic-tumor-normal` seed 3, where B failed E010 on a hard-coded
absolute path and C passed. The session-level counts are B 48/48 and
C 39/48 (exact p = 0.00390625). On those cells C spends ~1.87× the
input and ~2.14× the output tokens in total (per-cell medians ~1.86× /
~2.49×) and ~1.8× the wall time of B.

**Conclusion:** the campaign cannot estimate the contract-feedback
effect from the ITT contrast because >half of the C arm was lost to a
provider outage. The ITT B > C result must not be reported as evidence
that contract feedback is harmful; the prior non-governed B > C result
had a different failure mechanism (short outputs / `template exited 1`
content failures) and is **not replicated** here. On the cells that
completed, the feedback loop is active (44/49 C successes carry
`TYPE-`/`SCI-`/`D00` markers), costs roughly 1.9× input / 2.1× output
tokens and 1.8× wall time, and exhausts the correction-round budget on
9/49 gate successes because advisory D004/D003 warnings are fed back as
hard failures — a product finding that the gate-based comparison alone
would have hidden. The follow-up targets are listed in §4.6.

The post-hoc validity-restricted population (15 SOUND specified intents)
gives B 36/45 (80.0%) vs C 25/45 (55.6%), exact p = 0.00342 (Edwards
p = 0.00555) — significant on ITT, but the same transport outage drives
C's losses, so it is not a clean scientific-validity contrast.
The specified-only (33 intents, vague excluded) population gives B
86/99 (86.9%) vs C 49/99 (49.5%), exact p = 7.84e-10 (Edwards
p = 1.89e-08). The vague-only population gives B 6/9 vs C 0/9 (C's
cells are all transport failures), exact p = 0.03125 but Holm
p = 0.09375 (not significant; Edwards Holm p = 0.1237).

The independent intent audit (§3.5) finds 15 specified SOUND + 3 vague
SOUND, 10 AMBIGUOUS, 6 GATE-UNFAIR, 1 FLAWED (`wgbs-bismark`), and 1
DUPLICATE (`chipseq-broad`). The `wgbs-bismark` counterexample proves
that **gate success is not scientific correctness**: Arm B produced one
gate-passing workflow that aligns bisulfite reads with bwa-mem and runs
`bismark_methylation_extractor` on the BWA BAM (invalid), and another
gate-passing workflow that uses the correct Bismark/Bowtie2 backend.

## 2. Campaign manifest

| Field | Frozen value |
| :--- | :--- |
| Date | 2026-11-28 (Asia/Shanghai); campaign UTC window 2026-11-27T16:49:49Z → 2026-11-28T00:12:47Z |
| Issue | #1395 |
| Pre-registration commit | `471f38cc` |
| Harness commit | `f4b6c9793bf2f7c5932facef549efef2997baab6` |
| Intent fixture | `eval/frontier/intents.json`, SHA-256 `76136532e308464bed29827e001b6d894840c5d2e7bdd8925f24361bde75bd51` |
| Binary | `target/debug/oxo-flow` (built from base `e7170492`), SHA-256 `989740b6362e41e1c6aeb5925d555c0be69a9e52b335f982039fd13214c76b8e` |
| Provider | operator `~/.oxo-flow/ai_config.json`: `provider = openai`, OpenAI-compatible endpoint (host/URL/key not committed) |
| Model | alias `DeepSeek`; endpoint-reported `model: "DeepSeek"` (backend version opaque) |
| Arms | A `unconstrained`, B `syntax-only`, C `full-contract` |
| Arm C toggle | `--ai-contract-feedback` (argv recorded per run) |
| Seeds | 1, 2, 3 (`R = 3`) |
| Cells | 36 intents × 3 arms × 3 seeds = **324** (108 per arm); completed 324/324 |
| Temperature | Arm A 0.0; Arms B/C provider default (engine `[ai] temperature` unset) |
| Budget | `OXO_FLOW_AI_MAX_TOKENS=16384`, `OXO_FLOW_AI_TIMEOUT_SECS=600`; harness `--timeout 900 --sleep 1` |
| Run order | serial arm-major (model → arm → intent → seed), no randomization |
| Primary analysis | intention-to-treat over all attempted cells; exact McNemar + Holm primary, Edwards + Holm sensitivity |

Exact campaign command:

```bash
export OXO_FLOW_AI_MAX_TOKENS=16384
export OXO_FLOW_AI_TIMEOUT_SECS=600
python3 eval/frontier/run_eval.py \
  --creds-from config \
  --arm-a-protocol auto \
  --variants unconstrained,syntax-only,full-contract \
  --seeds 3 \
  --binary "$PWD/target/debug/oxo-flow" \
  --timeout 900 --sleep 1 \
  --campaign-note "governed pre-registered execution #1395; intents not held out; R=3; pre-registration commit 471f38cc" \
  --out eval/frontier/results/governed-2026-11-28
```

A 1-intent × 3-arm protocol pilot (`qc-fastp-multiqc`) ran after the
pre-registration commit into a separate directory and is excluded from
the primary dataset. It verified that Arm A receives content, Arms B/C
execute and archive sessions, and Arm C's argv carries
`--ai-contract-feedback`. The pilot was not used in any number below.

### 2.1 Analysis-plan note: exact McNemar primary

The pre-registration (§7 of
[`GOVERNED_CAMPAIGN_PRE_REGISTRATION_2026-11-28.md`](../../../eval/frontier/GOVERNED_CAMPAIGN_PRE_REGISTRATION_2026-11-28.md))
and the frozen analysis instrument at harness commit `f4b6c979` both
name the **exact two-sided McNemar test with Holm correction** as the
primary. An intermediate revision (`c7f0990b`, made after data
collection ended at 2026-11-28T00:12:47Z) relabeled the
continuity-corrected Edwards chi-square as the "frozen primary"; the
independent review identified that relabel as a post-data analysis
change. It is withdrawn here: this report presents **exact McNemar +
Holm as the pre-registered primary** and **Edwards + Holm as a
sensitivity**. Both are shown for every contrast. The choice changes no
p-value and no significance decision: where the exact primary is
significant the Edwards sensitivity is too, and the vague-only contrast
is not significant after Holm under either. The pre-registration note
itself is not rewritten; this note records the correction.

## 3. Design (frozen before the first primary cell)

- **Arm A `unconstrained`:** one direct LLM call with the unchanged bare
  7-rule system prompt, OpenAI chat-completions protocol,
  `max_tokens = 8192`, `temperature = 0.0`.
- **Arm B `syntax-only`:** `oxo-flow template --ai` (PipelineGenAgent,
  tools, syntax/structural feedback) with `--profile compact`,
  `--ai-attempts 1`, the binary's default retry budget, and **no**
  `--ai-contract-feedback`.
- **Arm C `full-contract`:** the identical Arm B argv plus
  `--ai-contract-feedback`; this is the only difference. Each engine run
  writes `argv.json` as direct evidence.
- **Judging:** only the three deterministic gates (`validate --json`,
  `dry-run --json`, `lint --json`, exit 0) plus the harness's
  deterministic named-tool substring fidelity check. Success = all three
  gates pass AND fidelity passes. No LLM judge.
- **Exclusions:** primary analysis is intention-to-treat over all
  attempted cells (a provider transport/timeout failure is a failure).
  A per-protocol sensitivity excludes only mechanically classified
  infrastructure failures: `generated = false`, zero input and output
  tokens, and a transport/timeout/HTTP-status error. Content failures
  (truncation, no TOML, gate failure) are never excluded.
- **Primary test:** exact two-sided McNemar on matched `(intent, seed)`
  cells for Arm B vs Arm C, Holm-corrected over {B vs A, C vs B, C vs A}.
  Secondary: marginal 95% percentile bootstrap CIs, per-tier rates, a
  seed-clustered intent-level sensitivity (`all` / `any` / `majority`),
  and the per-protocol sensitivity.

## 3.5 Intent validity audit (post-hoc, independent)

A separate per-intent scientific audit was supplied for this campaign
([full audit table](data/ai-synthesis-ablation-governed-2026-11-28-intent-audit.md)).
It is **post-hoc** relative to the frozen pre-registration: the frozen
protocol did not include an intent-validity gate, and this audit does
**not** change the frozen primary analysis. Its purpose is to separate
"the benchmark produced a gate-passing workflow" from "the intent is
scientifically well-posed and fairly judged".

### 3.5.1 Systemic finding

The benchmark's success is the three deterministic gates plus a raw
lowercased substring fidelity check. The gates certify engine-acceptable
structure; `lint` runs without `--strict` (all warnings pass), SCI
preflight never fails `dry-run`, and fidelity only checks that a tool
name appears somewhere in the artifact. None of these judge tool choice,
flags, method, or biological correctness. The strongest in-campaign
counterexample is `wgbs-bismark` (§3.5.6).

### 3.5.2 Per-intent verdicts

The per-intent list is authoritative (its header count says 17 SOUND
while the list contains 18 — 15 specified SOUND + 3 vague SOUND). The 36
verdicts sum to 18 SOUND (15 specified + 3 vague), 10 AMBIGUOUS,
6 GATE-UNFAIR, 1 FLAWED, 1 DUPLICATE.

| # | Intent | Verdict | Audit finding and impact |
| :---: | :--- | :--- | :--- |
| 1 | qc-fastp-multiqc | SOUND | Paired-end QC + MultiQC; tools match. |
| 2 | qc-fastqc | SOUND | Assay-agnostic FastQC; cohort fan-out. |
| 3 | trim-cutadapt | AMBIGUOUS | Small-RNA adapter sequence and length/quality thresholds unspecified. |
| 4 | qc-fastq-screen | GATE-UNFAIR | Executable `fastq_screen` vs required hyphenated package `fastq-screen` (latent false negative). |
| 5 | qc-nanoq | SOUND | ONT QC/filter pair correct. |
| 6 | rnaseq-star | SOUND | Standard STAR → featureCounts → MultiQC; strandedness advisory only. |
| 7 | rnaseq-hisat2 | SOUND | Standard HISAT2/StringTie; guided vs de-novo unspecified. |
| 8 | rnaseq-salmon | SOUND | Decoy-aware Salmon specified. |
| 9 | rnaseq-de-deseq2 | GATE-UNFAIR | Text asks MultiQC but `requires` omits it; DESeq2 design/contrast unspecified. |
| 10 | scrna-starsolo | AMBIGUOUS | "Cell-level QC with fastqc-style reports" conflates read-level FastQC with per-cell metrics; R1/R2, chemistry, whitelist unspecified. |
| 11 | germline-variant | SOUND | Correct bcftools chain. |
| 12 | germline-gatk | AMBIGUOUS | "Best practice" omits BQSR and GenomicsDBImport; requires `picard` without a `gatk|picard` alternate. |
| 13 | somatic-tumor-normal | AMBIGUOUS | Strelka2 indels without Manta `--indelCandidates` reduces indel sensitivity; the intent promises indels. |
| 14 | somatic-mutect2 | SOUND | Correct Mutect2 chain. |
| 15 | cnv-cnvkit | GATE-UNFAIR | Text names MarkDuplicates but `requires` omits picard/gatk. |
| 16 | variant-annotate | AMBIGUOUS | No filter criteria/order; summary tool unnamed. |
| 17 | chipseq-macs2 | SOUND | Standard MACS2 chain; control/genome size unspecified. |
| 18 | chipseq-broad | DUPLICATE | Identical tool chain/requires as chipseq-macs2; `--broad`/control invisible to gates/fidelity. |
| 19 | atacseq | SOUND | Correct ATAC chain; mito/blacklist not stated. |
| 20 | wgbs-bismark | FLAWED | bwa-mem is not bisulfite-aware; Bismark uses Bowtie2/HISAT2. A gate-passing 2026-11-28 workflow aligned with bwa-mem and fed the BAM to `bismark_methylation_extractor` (§3.5.6). Intent also names bwa-mem but does not require `bwa`. |
| 21 | amplicon-dada2 | AMBIGUOUS | Primers/region/truncation/taxonomy DB unspecified. |
| 22 | metagenomics-kraken2 | SOUND | Standard Kraken2 → Bracken → Krona; DB/read length/confidence unspecified. |
| 23 | metag-assembly | AMBIGUOUS | Host genome and co-assembly vs per-sample unspecified. |
| 24 | assembly-spades | SOUND | Correct isolate assembly. |
| 25 | assembly-flye-polish | SOUND | Correct ONT chain. |
| 26 | hic-cooler | GATE-UNFAIR | Text names pairix/read-pair parsing but `requires` omits pairix/pairtools. |
| 27 | spatial-visium | SOUND | Canonical Visium → Seurat → RCTD; failures are schema-dialect, not intent. |
| 28 | single-cell-multiome | SOUND | Canonical Multiome → Signac → Seurat WNN; failures are schema-dialect, not intent. |
| 29 | metagenome-humann-pathways | AMBIGUOUS | HUMAnN3's taxonomic input is MetaPhlAn, not Kraken2; reads as one pipeline but is two branches. |
| 30 | longread-structural-variants | AMBIGUOUS | ONT vs PacBio; minimap2 preset and Clair3 model differ by platform. |
| 31 | gatk-cohort-joint-genotyping | GATE-UNFAIR | Requires samtools/bcftools not named in the text; an all-GATK implementation can fail fidelity. |
| 32 | cutandtag-epigenomics | AMBIGUOUS | Spike-in normalization, SEACR threshold, MACS3 genome size unspecified. |
| 33 | rnaseq-star-salmon-quant | GATE-UNFAIR | Requires package `subread` while the executable/every other intent uses `featureCounts`; a correct workflow naming only featureCounts fails fidelity. STAR 2-pass and Salmon are parallel branches. |
| 34 | qc-vague | SOUND (vague) | Deliberately vague; separate reporting cell. |
| 35 | rnaseq-vague | SOUND (vague) | Deliberately vague; separate reporting cell. |
| 36 | variant-vague | SOUND (vague) | Deliberately vague; separate reporting cell. |

The audit's annotation summary keeps GATE-UNFAIR and AMBIGUOUS intents in
the gate-pass endpoint with an explicit annotation (they are not removed);
`wgbs-bismark` is excluded from any endpoint interpreted as
"scientifically valid workflows" but **kept and annotated** in the frozen
gate-pass primary.

The GATE-UNFAIR finding is not hypothetical: Arm A's
`gatk-cohort-joint-genotyping` seeds 2 and 3 were fidelity-scored as
missing `samtools`/`bcftools` — tools the intent text does not name — as
the audit predicted. Both cells also failed the gates, so the success
verdict was unchanged, but the scoring contract is demonstrably unfair
for that intent.

### 3.5.3 Populations reported

| Population | Intents | Cells/arm (R=3) | Status |
| :--- | :---: | :---: | :--- |
| Frozen primary (pre-registered) | 36 | 108 | The pre-registration included all 36; vague cells are counted in their base tiers and in the paired tests (the code's behavior). |
| Specified-only (vague excluded) | 33 | 99 | **Post-hoc**, the reporting contract the README claims; corrected tier and paired tables. |
| Validity-restricted (SOUND specified) | 15 | 45 | **Post-hoc**, the audit's scientific-validity population; includes `spatial-visium`/`single-cell-multiome`. |
| Vague-only | 3 | 9 | **Post-hoc**, descriptive; never compared against specified tiers in this report. |

The frozen and corrected reporting conventions are both reported for
every headline; the report never shows only the more favorable one.

**Annotated / defective intents, reported separately** (C's cells are
all transport failures, so none of these rows is a feedback result):

| Intent | Audit verdict | A | B | C | Note |
| :--- | :--- | :---: | :---: | :---: | :--- |
| wgbs-bismark | FLAWED | 0/3 | 2/3 | 0/3 | B's two gate-passing cells include the scientifically invalid bwa-mem → `bismark_methylation_extractor` workflow (§3.5.6). |
| gatk-cohort-joint-genotyping | GATE-UNFAIR | 0/3 | 3/3 | 0/3 | Fidelity requires samtools/bcftools not named in the intent text. |
| rnaseq-star-salmon-quant | GATE-UNFAIR | 0/3 | 2/3 | 0/3 | Fidelity requires the package name `subread`, not the `featureCounts` executable. |
| chipseq-broad | DUPLICATE | 0/3 | 3/3 | 0/3 | Near-duplicate of `chipseq-macs2`; `--broad` is invisible to the gates. |

### 3.5.4 Vague-cell double-count (reporting-contract bug)

The README says vague cells are "reported on their own row and are never
compared against the specified tiers", but the `aggregate_report` tier
aggregation in `run_eval.py` counts them in their base tier **and** the
vague row, and `analyze_arms` in `statistical_report.py` includes them in
the McNemar comparisons (no vague filter). The frozen pre-registration
inherited the code's all-36 inclusion. This report therefore gives the
frozen all-36 numbers and the post-hoc specified-only (33) numbers side
by side; the double-count is disclosed, not silently corrected. Fixing
the harness is follow-up work, not a post-hoc edit of this campaign.

### 3.5.5 Persistent failures: proprietary-tool/container-dialect

`single-cell-multiome` and `spatial-visium` are **SOUND** intents. In
this campaign both failed in Arm B because the model emitted
`[rules.<n>.environment] container = "..."`, which is not a schema key.
The exact rejection (all three seeds) is:

```text
parse error in workflow.oxoflow: config error: E017: unknown key
'container' in [rules] #0.environment — not a .oxoflow key. Check the
spelling (see the workflow format reference for the valid sections).
```

The valid keys are `docker` / `singularity` (or a conda specification).
The E017 feedback repeated across seeds without convergence and offered
no "did you mean docker/singularity" suggestion. These are model/arm
schema-dialect failures, not intent defects: the prompt is scientifically
coherent and `requires` matches. The 2026-11-26 Qwen run passed
`single-cell-multiome` in Arm B and `spatial-visium` in Arm C, so the
failure is model/arm-specific. In the 2026-11-28 governed campaign the
C-arm cells for both intents were lost to the provider transport outage
(no artifact), so the container-dialect evidence here is B-specific; C
had no opportunity to reproduce or repair it. They are **not excluded**
from any scientific-validity population; they are annotated as
proprietary-tool/container-dialect cells. Product findings: add an E017
`container` → `docker`/`singularity` suggestion, and environment-syntax /
Space Ranger / RCTD knowledge to the generation knowledge base.

### 3.5.6 `wgbs-bismark`: gate success is not scientific correctness

Arm B produced two gate-passing, fidelity-passing `wgbs-bismark`
workflows in this campaign with opposite scientific validity:

- `DeepSeek__syntax-only__wgbs-bismark__s1` (success): builds a **BWA**
  index (`bwa index -a bwtsw`), aligns with `bwa mem`, then runs
  `bismark_methylation_extractor` on the BWA BAM. BWA is not
  bisulfite-aware, so this pipeline cannot produce valid methylation
  calls. Workflow SHA-256 is in the sanitized manifest.
- `DeepSeek__syntax-only__wgbs-bismark__s2` (success): builds the
  Bismark genome with `bismark_genome_preparation --bowtie2` and uses
  the Bismark/Bowtie2 aligner. This is the methodologically correct
  branch.

Both count as benchmark successes because the three gates and the
substring fidelity check cannot distinguish them. This is the concrete
evidence for the limitation "gate success ≠ scientific correctness" and
for the audit's FLAWED verdict on the intent (which itself names
bwa-mem). The intent stays in the frozen gate-pass primary, annotated;
it is excluded from any endpoint interpreted as scientifically valid.

## 4. Results

### 4.1 Overall (frozen all-36 population, ITT)

Success = all three deterministic gates pass AND the named-tool fidelity
check passes. "Gate pass" means all three gates; fidelity counts include
non-generated cells whose partial artifact happened to contain every
required tool name.

| Arm | Generated | Gate pass | Fidelity | Success | pass@1 | 95% bootstrap CI | Tokens in | Tokens out | Total tokens | Est. cost | Wall mean / max |
| :--- | :---: | :---: | :---: | :---: | :---: | :---: | ---: | ---: | ---: | ---: | :---: |
| A `unconstrained` | 81 | 0 | 79 | **0** | **0.0%** | [0.0%, 0.0%] | 23,322 | 355,391 | 378,713 | $0.3975 | 32.5 s / 106.2 s |
| B `syntax-only` | 99 | 92 | 102 | **92** | **85.2%** | [78.7%, 91.7%] | 6,156,764 | 1,313,563 | 7,470,327 | $3.1688 | 120.9 s / 554.8 s |
| C `full-contract` | 49 | 49 | 58 | **49** | **45.4%** | [36.1%, 54.6%] | 5,312,429 | 1,181,293 | 6,493,722 | $2.7869 | 89.4 s / 456.7 s |

- Arm A's 81 generated workflows all fail at parse time (E017 wrong
  dialect); 27 cells produced no workflow (14 provider connection drops,
  13 truncations at the frozen 8,192-token Arm A cap).
- Arm B's 7 generated-but-unsuccessful cells are 6
  proprietary-tool/container-dialect cells plus one E010 hard-coded
  absolute-path cell (§4.5).
- Arm C's 59 non-successes are all provider transport failures; C has
  **zero** generated-but-unsuccessful cells.
- Cost is the harness's static price-table estimate, not a bill.

### 4.2 Tiers

**Frozen all-36 (vague cells counted in their base tier, the code's
behavior):**

| Arm | easy | medium | hard |
| :--- | :---: | :---: | :---: |
| A | 0/15 | 0/42 | 0/51 |
| B | 13/15 | 38/42 | 41/51 |
| C | 12/15 | 19/42 | 18/51 |

**Post-hoc specified-only (33 intents; vague excluded):**

| Arm | easy | medium | hard |
| :--- | :---: | :---: | :---: |
| A | 0/12 | 0/36 | 0/51 |
| B | 12/12 | 33/36 | 41/51 |
| C | 12/12 | 19/36 | 18/51 |

**Post-hoc validity-restricted (15 SOUND specified intents):**

| Arm | easy | medium | hard |
| :--- | :---: | :---: | :---: |
| A | 0/9 | 0/18 | 0/18 |
| B | 9/9 | 17/18 | 10/18 |
| C | 9/9 | 13/18 | 3/18 |

**Vague-only (3 intents), descriptive:**

| Arm | easy | medium |
| :--- | :---: | :---: |
| A | 0/3 | 0/6 |
| B | 1/3 | 5/6 |
| C | 0/3 | 0/6 |

C's vague cells are all transport failures; the vague 0/9 is not a
model/feedback result.

### 4.3 Paired statistics (exact McNemar + Holm primary; Edwards + Holm sensitivity)

`b` = second-arm-only pass, `c` = first-arm-only pass. For B vs C,
`b` is C-only and `c` is B-only. Holm p-values below 1e-6 are stored as
0.0 by the instrument's 6-decimal rounding; they are reported here as
`< 1e-6`.

| Population | N/arm | Contrast | b / c | Exact p (primary) | Holm exact p | Edwards p (sensitivity) | Holm Edwards p |
| :--- | :---: | :--- | :---: | ---: | ---: | ---: | ---: |
| Frozen all 36 | 108 | B vs A | 92 / 0 | 4.04e-28 | < 1e-6 | 2.41e-21 | < 1e-6 |
| Frozen all 36 | 108 | C vs B | 2 / 45 | **1.60e-11** | < 1e-6 | **9.03e-10** | < 1e-6 |
| Frozen all 36 | 108 | C vs A | 49 / 0 | 3.55e-15 | < 1e-6 | 7.07e-12 | < 1e-6 |
| Specified 33 | 99 | B vs A | 86 / 0 | 2.58e-26 | < 1e-6 | 5.00e-20 | < 1e-6 |
| Specified 33 | 99 | C vs B | 2 / 39 | 7.84e-10 | < 1e-6 | 1.89e-08 | < 1e-6 |
| Specified 33 | 99 | C vs A | 49 / 0 | 3.55e-15 | < 1e-6 | 7.07e-12 | < 1e-6 |
| SOUND 15 | 45 | B vs A | 36 / 0 | 2.91e-11 | < 1e-6 | 5.45e-09 | < 1e-6 |
| SOUND 15 | 45 | C vs B | 1 / 12 | **0.003418** | 0.003418 | **0.005546** | 0.005546 |
| SOUND 15 | 45 | C vs A | 25 / 0 | 5.96e-08 | < 1e-6 | 1.59e-06 | 3.00e-06 |
| Vague 3 | 9 | B vs A | 6 / 0 | 0.03125 | 0.09375 | 0.04123 | 0.12368 |
| Vague 3 | 9 | C vs B | 0 / 6 | 0.03125 | 0.09375 | 0.04123 | 0.12368 |
| Vague 3 | 9 | C vs A | 0 / 0 | 1.0 | 1.0 | 1.0 | 1.0 |

The frozen all-36 and specified-33 populations show B above C under both
tests. The SOUND-15 population is the same direction but is dominated by
the same transport outage (§4.5); it is not a clean scientific-validity
contrast.

**Per-protocol matched comparison (48 cells where both B and C
completed; single post-hoc tests, not part of the 3-contrast family):**

| Measure | Gate-level outcome | Session-level outcome |
| :--- | :---: | :---: |
| B success | 47/48 | 48/48 |
| C success | 48/48 | 39/48 |
| Discordant (C-only / B-only) | 1 / 0 | 0 / 9 |
| Exact p | 1.0 | **0.00390625** |
| Edwards p | 1.0 | 0.00766 |

The gate-level comparison is null; the session-level comparison is not.
The one gate-level discordant cell is `somatic-tumor-normal` seed 3,
where B failed E010 on a hard-coded absolute path and C passed. The
nine session-level B-only cells are C gate successes whose generation
loop failed (nine max-round exhaustions and the `chipseq-macs2` seed 1
transport ending); see §4.5.

### 4.4 Intent-level sensitivity (frozen all-36; exact test, N = 36 intents)

| Rule | B vs A (b/c, exact p) | B vs C (b/c, exact p) | C vs A (b/c, exact p) |
| :--- | :---: | :---: | :---: |
| all seeds pass | 26/0, 2.98e-08 | 2/12, 0.01294 | 16/0, 3.05e-05 |
| any seed passes | 34/0, 1.16e-10 | 0/17, 1.53e-05 | 17/0, 1.53e-05 |
| majority pass | 32/0, 4.66e-10 | 0/16, 3.05e-05 | 16/0, 3.05e-05 |

The intent-level view is the same direction as the cell-level ITT and is
subject to the same transport confound: C's `any`/`majority` zeros are
driven by late intents lost to the outage.

### 4.5 Failure classification, the C outage, and the feedback loop

| Arm | Success | Generated, gate/fidelity fail | No artifact: transport | No artifact: other |
| :--- | :---: | :---: | :---: | :---: |
| A | 0 | 81 (74 E017 + 7 other TOML parse errors) | 14 (`Remote end closed connection`) | 13 (no TOML; 8,192-token cap) |
| B | 92 | 7 (6 container dialect + 1 E010 absolute path) | 9 | 0 |
| C | 49 | 0 | **59** | 0 |

**The 59 ITT failures are a provider outage.** Every C failure is:

```text
provider error: AI request failed after 3 attempts:
error sending request for url (<provider-endpoint>)
```

with **0 input tokens, 0 output tokens, 0 tool calls, 2 session
messages, and a ~7.1 s wall time**. The CLI log shows the three attempts
and the 2 s / 5 s backoff. The harness's `rounds` field is null for every
run (it is not recorded); round/tool-call evidence comes from the
session archive (`messages`, `tool_calls`) and the CLI log. The harness
records only `error: "template exited 1"`; the provider cause lives in
the AI session archive and CLI log. This is the "CLI error not
classified by the harness" mechanism.

The failures are one contiguous tail:

| Fact | Value |
| :--- | :--- |
| Last C success started | 2026-11-27T23:59:46Z |
| First C transport failure started | 2026-11-28T00:04:48Z |
| Last C transport failure started | 2026-11-28T00:12:39Z |
| C transport failures | 59 / 108 (54.6%) |
| Duration per failure | ~7.1 s, 0 tokens |
| C cells that reached the model | 49, all gate-passing |

B had 9 transport failures scattered over ~2.5 h (18:53–21:23Z), not a
contiguous tail. The C block ran last (the serial arm-major order), so
the outage at 00:04:48Z removed the C arm's remaining 59 cells.

**Gate success is not loop completion.** Session outcomes show that 10 of
C's 49 gate successes ended with a failed AI session:

| Arm | Gate successes | Session Success | Session Failed |
| :--- | :---: | :---: | :---: |
| B | 92 | 92 | 0 |
| C | 49 | 39 | **10** |

The 10 C cells are:

| Intent | Seed | Session terminal state | CLI budget warning |
| :--- | :---: | :--- | :---: |
| rnaseq-salmon | 2 | `exceeded max rounds (10) without valid output`; D004 reference-path warnings | yes |
| scrna-starsolo | 3 | `exceeded max rounds (10) without valid output`; D003/D004 warnings | yes |
| germline-variant | 2 | `exceeded max rounds (10) without valid output`; D004 reference-path warning | yes |
| germline-variant | 3 | `exceeded max rounds (10) without valid output`; D004 reference-path warning | yes |
| germline-gatk | 3 | `exceeded max rounds (10) without valid output`; D004 reference-path warning | yes |
| somatic-tumor-normal | 1 | `exceeded max rounds (10) without valid output`; D004 reference-path warning | yes |
| somatic-tumor-normal | 2 | `exceeded max rounds (10) without valid output`; D004 reference-path warning | yes |
| somatic-tumor-normal | 3 | `exceeded max rounds (10) without valid output`; D004 reference-path warning | yes |
| cnv-cnvkit | 1 | `exceeded max rounds (10) without valid output`; D004 reference-path warnings | yes |
| chipseq-macs2 | 1 | provider transport error after producing the workflow | yes |

The nine max-round cells are the product finding: **advisory D003/D004
warnings (binary not found / reference path not found) were fed back as
hard contract-feedback failures**, the model could not make the
generation sandbox satisfy them, and the loop exhausted its
correction-round budget even though the final artifact passed the three
static gates. The harness's gate endpoint records these as successes;
the session archive records the loop failure. The post-hoc
session-level matched comparison (48 cells) is **B 48/48 vs C 39/48**
(exact p = 0.00390625; Edwards p = 0.00766), so the gate-level p = 1.0
is not a loop-completion null.

**The feedback loop was active in the C cells that ran.** 44 of C's 49
gate successes contain `TYPE-`/`SCI-`/`D00` markers in the session
archive (199 marker occurrences). The loop did not cause the 59 ITT
failures (those cells never received a model response), but it did cause
9 of the 49 gate successes to exhaust the round budget.

**Matched-cell cost** (the 48 cells where both arms completed):

| Arm | Successes | Input tokens median / total | Output tokens median / total | Wall median / max |
| :--- | :---: | ---: | ---: | :---: |
| B | 47/48 | 52,917 / 2,754,753 | 8,606 / 536,363 | 92.9 s / 554.8 s |
| C | 48/48 | 98,446 / 5,143,932 | 21,408 / 1,147,447 | 168.9 s / 447.0 s |

C uses ~1.87× the input and ~2.14× the output tokens in total
(per-cell medians ~1.86× / ~2.49×) and ~1.8× the wall time on the same
cells. This is the measurable cost of the contract-feedback loop; the
gate-level success difference on these cells is one discordant cell
(p = 1.0), while the session-level difference is nine cells
(p = 0.0039).

### 4.6 Direction, prior evidence, mechanism, and product conclusion

**Prior evidence and directional hypothesis.** Arm C (`full-contract`)
is the product target and the a-priori best arm. The prior non-governed
run contradicted that expectation: B 28/36 > C 18/36, exact McNemar
p = 0.0129 (one seed, Qwen alias via an internal Anthropic-compatible
proxy, default temperature). Its C failures were dominated by very short
outputs followed by `template exited 1` — a content/generation failure
mode.

**This campaign's direction.** The ITT direction matches the prior run
(B > C, A worst; exact p = 1.60e-11, Edwards p = 9.03e-10), but the
mechanism does **not**: all 59 ITT C failures are the provider transport
outage described in §4.5. Separately, 9 of C's 49 gate successes are
max-round exhaustions caused by D003/D004 feedback. The per-protocol
gate-level comparison is null (B 47/48 vs C 48/48, p = 1.0); the
session-level comparison is not (B 48/48 vs C 39/48, exact
p = 0.00390625).

**Mechanism hypotheses, checked:**

- *Max-round / retry exhaustion:* **supported for 9 of C's 49 gate
  successes** — the sessions hit `exceeded max rounds (10) without valid
  output` after D003/D004 reference-path / binary-not-found warnings.
  It is **not** the cause of the 59 ITT failures: those cells have
  0 tool calls and 2 messages; the provider request failed before any
  model turn.
- *Feedback non-convergence:* **supported for the D003/D004 loop** in the
  9 cells, because advisory warnings were fed back as hard contract
  failures. The E017 `container` loop is real but belongs to Arm B's
  `spatial-visium`/`single-cell-multiome` cells (6 cells); C's
  corresponding cells were transport-failed and produced no artifact.
- *Over-constraint / gate interaction:* the three gates pass all 49 C
  gate successes, but 9 sessions exhaust; **gate success hides loop
  exhaustion**. This is a gate-blindness finding, not evidence that the
  static gates are wrong.
- *Proprietary-tool knowledge gap:* observed in B (container dialect,
  §3.5.5); C had no opportunity to show it because those cells were
  lost to the outage.
- *CLI error misclassification:* confirmed. `template exited 1` hides
  the provider transport cause; the session archive is the only place
  the real error appears.

**Product conclusion.** This campaign does **not** show that contract
feedback is harmful (the ITT contrast is transport-confounded), and it
does **not** show that it helps (the per-protocol gate-level contrast is
null and the session-level contrast is worse). It also does not
replicate the prior non-governed negative result's mechanism. The
measured product facts are: (a) the feedback loop is active in completed
C runs (44/49 gate successes carry `TYPE-`/`SCI-`/`D00` markers);
(b) it costs roughly 1.87× input / 2.14× output tokens in total
(per-cell medians ~1.86× / ~2.49×) and 1.8× the wall time on matched
cells; (c) the gate-level matched contrast is null (B 47/48 vs C 48/48)
while the session-level contrast is C 39/48 vs B 48/48 (exact
p = 0.0039); (d) **9/49 C gate successes exhaust the correction-round
budget because advisory D003/D004 warnings are treated as hard
failures** — the primary product finding; (e) the harness does not
classify provider transport errors separately from generation failures
(the CLI stderr and session archive carry the cause, but `results.json`
records only `template exited 1`); (f) the serial arm-major order let a
provider outage wipe out an entire arm's tail.

**Optimization targets (follow-up work, not changes made here):**

1. Contract feedback must not treat advisory D003/D004 warnings as hard
   failures; distinguish "cannot verify in the generation sandbox" from
   "invalid workflow", and bound no-progress rounds.
2. Report the AI-session outcome (Success/Failed/max-round) beside the
   gate outcome; add a session-level completion endpoint or sensitivity
   to the harness.
3. Surface the CLI/session provider transport cause in the harness's
   `error` field instead of the generic `template exited 1`, and add
   campaign-level retry/backoff with an attempt ledger.
4. Interleave or randomize arm/seed order so a provider outage cannot
   correlate with one arm.
5. Add the E017 `container` → `docker`/`singularity` suggestion and
   environment-syntax guidance to the generation feedback.
6. Add proprietary-tool knowledge (Space Ranger, Cell Ranger ARC,
   Signac/Seurat, RCTD/Spacexr) and R-package invocation patterns.
7. Study the contract-feedback cost envelope (~1.9× input / ~2.1× output
   tokens, 1.8× wall) and consider enabling it only for model
   tiers/ceilings where it pays.
8. Intent-set v2, gate/fidelity corrections, and the vague double-count
   fix.

### 4.7 Per-intent B/C differences

Only these specified intents differ between B and C (all other specified
intents have identical per-arm pass counts; the per-cell manifest has
the full matrix):

| Intent | B (of 3) | C (of 3) | Cause / note |
| :--- | :---: | :---: | :--- |
| amplicon-dada2 | 2 | 0 | transport outage |
| assembly-flye-polish | 3 | 0 | transport outage |
| assembly-spades | 3 | 0 | transport outage |
| atacseq | 3 | 0 | transport outage |
| chipseq-broad | 3 | 0 | transport outage |
| chipseq-macs2 | 3 | 1 | transport outage (2 cells) |
| cutandtag-epigenomics | 3 | 0 | transport outage |
| gatk-cohort-joint-genotyping | 3 | 0 | transport outage |
| germline-variant | 2 | 3 | C-only win on one B transport cell |
| hic-cooler | 3 | 0 | transport outage |
| longread-structural-variants | 3 | 0 | transport outage |
| metag-assembly | 3 | 0 | transport outage |
| metagenome-humann-pathways | 3 | 0 | transport outage |
| metagenomics-kraken2 | 1 | 0 | transport outage |
| rnaseq-star-salmon-quant | 2 | 0 | transport outage |
| somatic-tumor-normal | 2 | 3 | C-only win on B's E010 absolute-path cell |
| wgbs-bismark | 2 | 0 | transport outage |

The three vague intents also differ: `qc-vague` B 1/3 vs C 0/3,
`rnaseq-vague` B 3/3 vs C 0/3, and `variant-vague` B 2/3 vs C 0/3
(B 6/9 vs C 0/9); C's cells are all transport failures. See §4.2.

Every C loss is a transport outage; the two C-only wins are one B
transport failure and one B E010 hard-coded-path failure. No C loss is a
contract-feedback or gate failure. The session-level losses (the 9
max-round gate successes) are a separate measure: they count as gate
successes but not as loop completions (§4.5).

## 5. Limitations

- **Intent validity is imperfect (post-hoc audit).** Of 36 intents, the
  independent audit finds 15 specified SOUND + 3 vague SOUND, 10
  AMBIGUOUS, 6 GATE-UNFAIR, 1 FLAWED (`wgbs-bismark`), and 1 DUPLICATE
  (`chipseq-broad` ≈ `chipseq-macs2`). The frozen primary includes all
  36; the post-hoc validity-restricted analysis uses only the 15 SOUND
  specified intents. The audit and its per-intent table are in §3.5.
- **Gate success ≠ scientific correctness.** Success is the three
  deterministic gates plus a raw substring fidelity check. `lint` is not
  `--strict`, SCI preflight never fails `dry-run`, and fidelity only
  checks that a tool name appears. The in-campaign counterexample is
  `wgbs-bismark` (§3.5.6): a gate-passing BWA→`bismark_methylation_extractor`
  workflow that cannot produce valid methylation calls. Gate-passing
  workflows must not be described as scientifically validated.
- **Provider outage confound (C arm).** All 59 C failures are provider
  transport failures in one contiguous window
  (2026-11-28T00:04:48Z → 00:12:39Z), zero tokens, ~7.1 s each; C's 49
  cells that reached the model all produced a gate-passing workflow.
  The ITT B > C result is therefore **not** a contract-feedback effect.
  The post-hoc gate-level per-protocol matched analysis (48 cells, the
  17 early intents selected by outage timing) is null (B 47/48 vs
  C 48/48, p = 1.0) and is not representative. The campaign cannot
  estimate the contract-feedback effect on the ITT contrast; a rerun with
  interleaved arms or a stable provider is required. Arm A's 14
  `Remote end closed connection` cells are the same provider-instability
  class.
- **Session outcome is not the gate endpoint.** Ten of C's 49 gate
  successes ended with a failed AI session: 9 exhausted the
  correction-round budget after advisory D003/D004 reference-path /
  binary-not-found warnings were fed back as hard failures, and 1 ended
  in a provider transport error after producing the workflow. The
  session-level matched comparison is B 48/48 vs C 39/48 (exact
  p = 0.00390625; Edwards p = 0.00766), so the gate-level matched null
  is not a loop-completion null. The 9 max-round cells are a product
  finding (advisory warnings should not be hard feedback), not an intent
  defect. Session outcome is a post-hoc endpoint; the pre-registered
  primary remains the gate endpoint.
- **Vague-cell double-count.** The README claims vague cells are reported
  separately, but the code counts them in their base tier and in the
  McNemar comparisons. The frozen primary inherits that reporting
  convention; the specified-only (33-intent) analysis is the corrected
  post-hoc view. Both are reported.
- **Proprietary-tool/container-dialect failures.** `single-cell-multiome`
  and `spatial-visium` are SOUND intents; their failures are E017
  `container` key errors that repeated without convergence and without a
  docker/singularity suggestion. They are not excluded on scientific
  grounds; they are annotated as model/arm schema-dialect cells.
- **Not held out.** The 36 intents are the development set used by the
  prior non-governed run. The numbers are a pre-registered execution,
  not an out-of-sample estimate.
- **One provider, one model alias, one host.** The alias `DeepSeek` does
  not reveal its backend version; the endpoint reported only the alias.
  Provider routing, rate limiting, and hardware are single-host
  observations.
- **Temperature asymmetry.** Arm A is pinned to 0.0; Arms B/C inherit
  the provider default because the engine `[ai] temperature` is unset.
  The arms may sample differently; reruns are not guaranteed
  reproducible.
- **Thinking-style backend.** Reasoning tokens count against the output
  budget. Arm A's frozen 8192-token cap truncated some generations; that
  is a budget effect of the bare baseline, visible in the raw
  `output_tokens` and `no TOML found in response` records.
- **Serial arm-major order.** Arms ran in fixed blocks (all Arm A cells,
  then B, then C). Time-varying provider behavior can confound arms; no
  run-order randomization was performed.
- **Static judging.** The gates and named-tool fidelity check measure
  structural validity and tool presence, not biological correctness.
  A gate-passing workflow can still be scientifically wrong (see the
  `wgbs-bismark` counterexample).
- **Cost is an estimate.** The harness price table assigns the alias the
  `"*"` entry (0.28 / 1.10 USD per Mtok). It is not a billing receipt.
- **No independent sign-off** and no human adjudication of failures. The
  intent audit is a supplied independent review of the fixtures, not a
  PR-05 sign-off of the campaign. The per-protocol sensitivity is
  mechanical.
- **No POSITIVE/NEGATIVE publication decision** is claimed; the governed
  held-out campaign remains outstanding.

## 6. Documentation impact

- **README research-program bet 2** gains the measured three-arm result
  (ITT B 92/108, C 49/108, A 0/108), the gate-level per-protocol null,
  the session-level C 39/48 vs B 48/48 finding (9 max-round exhaustions
  from advisory D003/D004 hard feedback), the provider transport
  confound, and the explicit "not held out / no publication claim"
  caveat.
- **ROADMAP Stage 12.4** moves the AI-synthesis row from
  "Simulation only (operational pilot)" to "Operational field evidence
  (pre-registered execution on the development intent set; held-out
  campaign outstanding)" and records the intent-audit verdicts, the
  container-dialect finding, the C transport outage, and the
  session-level max-round finding.
- **Vision** updates the "Deterministic AI evaluations" bullet with the
  measured result, the gate-success ≠ scientific-correctness caveat, and
  the session-level gate-vs-loop distinction, and the "Controlled
  benchmark campaigns" bullet no longer says that none has run: the
  synthesis ablation has run on the development set, while the
  held-out-intent, multi-model, and human-factors campaigns remain
  outstanding.
- **AGENTS.md** replaces the "the governed pre-registered campaign
  remains outstanding" sentence with the 2026-11-28 execution, its
  numbers, its transport confound, its session-level max-round finding,
  and the still-outstanding held-out campaign.
- **`eval/frontier/README.md`** restores the exact-primary correction
  note and gains a pointer to the governed report, the
  transport-classification limitation, and the session-outcome caveat.
- No peer-review, biological-correctness, or held-out-generalization
  claim is added anywhere. Every doc states that the ITT B > C is
  transport-confounded, that the gate-level per-protocol contrast is
  null while the session-level contrast is not, and that the campaign
  cannot estimate the contract-feedback effect from the ITT contrast.

## 7. Evidence and reproduction

- Raw per-cell artifacts (generated workflow, gate JSON/stderr,
  `cli.log`, `session.json`, `argv.json`, `result.json`) remain in the
  gitignored `eval/frontier/results/governed-2026-11-28/`.
- Sanitized machine-readable manifest:
  [`data/ai-synthesis-ablation-governed-2026-11-28-results.json`](data/ai-synthesis-ablation-governed-2026-11-28-results.json)
  — absolute paths stripped, binary normalized to `target/debug/oxo-flow`,
  no endpoint/credential material; each run carries the generated
  workflow's SHA-256 and its AI-session identity
  (`session_started_at`, `session_ended_at`, `session_outcome`,
  URL-sanitized `session_error`, `session_messages`,
  `session_tool_calls`). The 10 session-failed gate successes and the 59
  transport failures are therefore auditable from the committed manifest
  without the raw directory.
- Committed analysis evidence:
  [`data/ai-synthesis-ablation-governed-2026-11-28-stats.json`](data/ai-synthesis-ablation-governed-2026-11-28-stats.json),
  [`...-stats.md`](data/ai-synthesis-ablation-governed-2026-11-28-stats.md),
  [`...-posthoc.json`](data/ai-synthesis-ablation-governed-2026-11-28-posthoc.json),
  [`...-mechanism.json`](data/ai-synthesis-ablation-governed-2026-11-28-mechanism.json),
  and the independent
  [`...-intent-audit.md`](data/ai-synthesis-ablation-governed-2026-11-28-intent-audit.md).
- The two post-hoc generators are committed and deterministic (no
  provider calls). Re-run the frozen command in §2 with the operator's
  own configured provider, then:

```bash
python3 eval/frontier/statistical_report.py \
  --results-dir eval/frontier/results/governed-2026-11-28 \
  --out-json /tmp/governed-stats.json --out-md /tmp/governed-stats.md

python3 eval/frontier/posthoc_analysis.py \
  --results-dir eval/frontier/results/governed-2026-11-28 \
  --out-json /tmp/governed-posthoc.json

python3 eval/frontier/mechanism_analysis.py \
  --results-dir eval/frontier/results/governed-2026-11-28 \
  --out-json /tmp/governed-mechanism.json
```

- The manifest was regenerated with `session_outcome`/`session_error`
  fields after the independent review; the committed `...-results.json`
  is the audited version. No paid provider calls run in CI;
  `make eval-tests` covers the harness and these analysis utilities with
  mocks and local fixtures.
