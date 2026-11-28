# Independent scientific audit of the 36 frontier intents (#1395)

> Supplied for the governed campaign as an independent per-intent audit.
> It is **post-hoc** relative to the frozen pre-registration (which did
> not include an intent-validity gate). The per-intent verdict list is
> authoritative; it contains 18 SOUND (15 specified + 3 vague), even
> though the supplied summary header said 17. Reproduced verbatim
> otherwise, with the count line corrected to the list.

Verdict counts: 18 SOUND (incl. 3 vague), 10 AMBIGUOUS, 6 GATE-UNFAIR, 1 FLAWED, 1 DUPLICATE.

## Systemic finding
The benchmark's "success" = three deterministic gates + a raw lowercased substring fidelity check (`run_eval.py:508-529`). The gates certify engine-acceptable structure; they do not judge tool choice, flags, method, or biological correctness. `lint` is not run with `--strict`, so all warnings pass; SCI checks never fail `dry-run`. Any claim that a gate-passing workflow is "scientifically correct" is unsupported. Strongest counterexample: `wgbs-bismark` (below).

## Per-intent verdicts (id — verdict — reason)
1. qc-fastp-multiqc — SOUND — paired-end QC + MultiQC; tools match.
2. qc-fastqc — SOUND — FastQC assay-agnostic; cohort fan-out.
3. trim-cutadapt — AMBIGUOUS — small-RNA adapter sequence and length/quality thresholds unspecified.
4. qc-fastq-screen — GATE-UNFAIR — executable `fastq_screen` vs required hyphenated package `fastq-screen` (latent).
5. qc-nanoq — SOUND — ONT QC/filter pair correct.
6. rnaseq-star — SOUND — standard STAR→featureCounts→MultiQC; strandedness advisory only.
7. rnaseq-hisat2 — SOUND — standard HISAT2/StringTie; guided/de-novo unspecified.
8. rnaseq-salmon — SOUND — decoy-aware Salmon specified.
9. rnaseq-de-deseq2 — GATE-UNFAIR — text asks MultiQC but `requires` omits it; DESeq2 design/contrast unspecified.
10. scrna-starsolo — AMBIGUOUS — "cell-level QC with fastqc-style reports" conflates read-level FastQC with per-cell metrics; R1/R2 and chemistry/whitelist unspecified.
11. germline-variant — SOUND — correct bcftools chain.
12. germline-gatk — AMBIGUOUS — "best practice" omits BQSR and GenomicsDBImport; requires `picard` without `gatk|picard` alternate.
13. somatic-tumor-normal — AMBIGUOUS — Strelka2 indels without Manta `--indelCandidates` reduces indel sensitivity; intent promises indels.
14. somatic-mutect2 — SOUND — correct Mutect2 chain.
15. cnv-cnvkit — GATE-UNFAIR — text names MarkDuplicates but `requires` omits picard/gatk.
16. variant-annotate — AMBIGUOUS — no filter criteria/order; summary tool unnamed.
17. chipseq-macs2 — SOUND — standard MACS2 chain; control/genome-size unspecified.
18. chipseq-broad — DUPLICATE — identical tool chain/requires as chipseq-macs2; `--broad`/control invisible to gates/fidelity.
19. atacseq — SOUND — correct ATAC chain; mito/blacklist not stated.
20. wgbs-bismark — FLAWED — bwa-mem is not bisulfite-aware; Bismark uses Bowtie2/HISAT2. A gate-passing 2026-11-28 workflow aligned with bwa-mem and fed the BAM to bismark_methylation_extractor (demonstrated). Intent also names bwa-mem but does not require `bwa`.
21. amplicon-dada2 — AMBIGUOUS — primers/region/truncation/taxonomy DB unspecified.
22. metagenomics-kraken2 — SOUND — standard Kraken2→Bracken→Krona; DB/read length/confidence unspecified.
23. metag-assembly — AMBIGUOUS — host genome and co-assembly vs per-sample unspecified.
24. assembly-spades — SOUND — correct isolate assembly.
25. assembly-flye-polish — SOUND — correct ONT chain.
26. hic-cooler — GATE-UNFAIR — text names pairix/read-pair parsing but `requires` omits pairix/pairtools.
27. spatial-visium — SOUND — canonical Visium→Seurat→RCTD; failures are schema-dialect, not intent.
28. single-cell-multiome — SOUND — canonical Multiome→Signac→Seurat WNN; failures are schema-dialect, not intent.
29. metagenome-humann-pathways — AMBIGUOUS — HUMAnN3's taxonomic input is MetaPhlAn, not Kraken2; reads as one pipeline but is two branches.
30. longread-structural-variants — AMBIGUOUS — ONT vs PacBio; minimap2 preset and Clair3 model differ by platform.
31. gatk-cohort-joint-genotyping — GATE-UNFAIR — requires samtools/bcftools not named in text; all-GATK implementation can fail fidelity.
32. cutandtag-epigenomics — AMBIGUOUS — spike-in normalization, SEACR threshold, MACS3 genome size unspecified.
33. rnaseq-star-salmon-quant — GATE-UNFAIR — requires package `subread` while executable/every other intent uses `featurecounts`; correct workflow naming only featureCounts fails fidelity. STAR 2-pass and Salmon are parallel branches.
34. qc-vague — SOUND — deliberately vague, separate cell.
35. rnaseq-vague — SOUND — deliberately vague, separate cell.
36. variant-vague — SOUND — deliberately vague, separate cell.

## Persistent failures
`single-cell-multiome`, `spatial-visium`: genuine model/arm schema-dialect bug, not intent defects. Generated `[rules.environment] container = "..."`; `container` is not a key in `EnvironmentSpec`/schema (valid: `environment = { docker = "..." }` or `singularity`). Triggered by proprietary 10x tools with no Bioconda package; the agent correctly decided a container is needed but wrote the wrong TOML key. E017 feedback repeats across seeds without convergence; message says only "not a .oxoflow key. Check the spelling", not "did you mean docker/singularity". In the 2026-11-26 qwen run these intents passed gates in some arms, so the failure is model/arm-specific. Do not exclude on scientific grounds; annotate as proprietary-tool/container-dialect cells. Product fix: E017 suggestion for `container` → docker/singularity; add environment-syntax and spacexr/RCTD guidance to the generation knowledge base.

## Vague cells double-count (reporting-contract bug)
README says vague cells are "reported on their own row and never compared against specified tiers", but `run_eval.py:378-386` counts them in their base tier AND the vague row, and `statistical_report.py:329-379` includes them in the McNemar comparisons (no vague filter). This matters for #1395 headline pass@1 and paired p-values. The 2026-11-26 audit already notes it parenthetically.

## Exclusion/annotation summary
- Exclude from any endpoint interpreted as "scientifically valid workflows": `wgbs-bismark` (FLAWED, demonstrated).
- Annotate gate-unfair (keep in gate-pass endpoint, do not read success as task compliance): rnaseq-de-deseq2, cnv-cnvkit, hic-cooler, gatk-cohort-joint-genotyping, rnaseq-star-salmon-quant; plus latent alias risks qc-fastq-screen, germline-gatk, somatic-tumor-normal, cutandtag-epigenomics, metagenomics-kraken2, longread-structural-variants.
- Annotate ambiguous: trim-cutadapt, scrna-starsolo, somatic-tumor-normal, variant-annotate, amplicon-dada2, metag-assembly, metagenome-humann-pathways, longread-structural-variants, cutandtag-epigenomics, germline-gatk.
- Duplicate/overlap: chipseq-broad near-duplicate of chipseq-macs2; note partial overlaps.
- Keep and annotate proprietary-tool status: spatial-visium, single-cell-multiome.
- Vague cells: keep separate; fix tier/statistical double-count in a follow-up.

## Bottom line
Most prompts name the right tools for the assay. The main validity problems are (a) one genuinely flawed method (`wgbs-bismark`), (b) several `requires`/text mismatches that let non-compliant workflows pass or compliant ones fail, and (c) the benchmark's framing (gates = syntax/structure; fidelity = substring). The governed data show the gates are highly correlated, so "all three gates" is effectively one syntax gate plus a tool-name check.
