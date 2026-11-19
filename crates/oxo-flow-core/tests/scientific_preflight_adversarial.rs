//! Adversarial fixture campaign for the curated `SCI-*` scientific-preflight
//! rules (issue #1244, ROADMAP Stage 12.2).
//!
//! Every rule in `src/scientific_preflight.rs` is exercised from a SINGLE
//! shared fixture table ([`fixtures`]). The table is never allowed to drift
//! from the source: [`fixture_table_covers_every_source_rule_exactly_once`]
//! parses the source file at compile time (`include_str!`) and fails if the
//! table does not enumerate exactly the distinct `code: "SCI-…"` ids the
//! engine can emit.
//!
//! Per rule the table carries up to three workflows:
//!
//! - **positive** — a workflow that violates the rule; the exact `code`
//!   MUST appear in the returned warnings (required for every rule);
//! - **corrected** — the fixed configuration; the `code` MUST stay silent;
//! - **mention-only** — a workflow that merely MENTIONS the trigger token
//!   (as a file name / path) while otherwise being correct; the `code`
//!   MUST stay silent (the #441 / #263 / #530 false-positive class).
//!
//! A rule with no meaningful corrected/mention-only fixture carries a
//! `skip` reason instead and lands in `NOT_ADVERSARIALLY_EXERCISED`, which
//! the coverage test collects and prints.

use std::collections::BTreeSet;

use oxo_flow_core::WorkflowConfig;
use oxo_flow_core::scientific_preflight::analyze_scientific_constraints;

/// One row of the shared fixture table.
struct Fx {
    code: &'static str,
    positive: String,
    corrected: Option<String>,
    mention_only: Option<String>,
    /// Set when the rule is not adversarially exercisable, with a one-line
    /// reason; such rules are reported as NOT_ADVERSARIALLY_EXERCISED.
    skip: Option<&'static str>,
}

/// A fully exercised rule: positive + corrected + mention-only.
fn fx3(code: &'static str, positive: String, corrected: String, mention_only: String) -> Fx {
    Fx {
        code,
        positive,
        corrected: Some(corrected),
        mention_only: Some(mention_only),
        skip: None,
    }
}

/// A rule that only has a positive fixture; `reason` binds it to the
/// NOT_ADVERSARIALLY_EXERCISED list.
fn fx_skip(code: &'static str, positive: String, reason: &'static str) -> Fx {
    Fx {
        code,
        positive,
        corrected: None,
        mention_only: None,
        skip: Some(reason),
    }
}

// ---- TOML builders -------------------------------------------------------

/// Single rule, benign textual output.
fn wf(shell: &str) -> String {
    format!(
        "[workflow]\nname = \"t\"\nversion = \"1.0\"\n\n[[rules]]\nname = \"r1\"\noutput = [\"out.txt\"]\nshell = \"{shell}\"\n"
    )
}

/// Single rule with an explicit input and output.
fn wf_io(shell: &str, input: &str, output: &str) -> String {
    format!(
        "[workflow]\nname = \"t\"\nversion = \"1.0\"\n\n[[rules]]\nname = \"r1\"\ninput = [\"{input}\"]\noutput = [\"{output}\"]\nshell = \"{shell}\"\n"
    )
}

/// Single rule with an explicit output only.
fn wf_o(shell: &str, output: &str) -> String {
    format!(
        "[workflow]\nname = \"t\"\nversion = \"1.0\"\n\n[[rules]]\nname = \"r1\"\noutput = [\"{output}\"]\nshell = \"{shell}\"\n"
    )
}

/// Single rule with declared resources (threads / memory).
fn wf_res(shell: &str, output: &str, threads: u32, memory: &str) -> String {
    format!(
        "[workflow]\nname = \"t\"\nversion = \"1.0\"\n\n[[rules]]\nname = \"r1\"\noutput = [\"{output}\"]\nshell = \"{shell}\"\n\n[rules.resources]\nmemory = \"{memory}\"\nthreads = {threads}\n"
    )
}

/// Rule whose shell is benign but whose name + input path MENTION the
/// trigger token (file-name mention class).
fn wf_mention(name: &str, input: &str) -> String {
    format!(
        "[workflow]\nname = \"t\"\nversion = \"1.0\"\n\n[[rules]]\nname = \"{name}\"\ninput = [\"{input}\"]\noutput = [\"out.bam\"]\nshell = \"samtools sort -o out.bam {input}\"\n"
    )
}

/// Rule that only MENTIONS `token` in its name and an input path while
/// running a benign shell — the mention-only false-positive class.
fn wf_mention_tok(token: &str) -> String {
    format!(
        "[workflow]\nname = \"t\"\nversion = \"1.0\"\n\n[[rules]]\nname = \"{token}_notes\"\ninput = [\"{token}_plan.md\"]\noutput = [\"out.txt\"]\nshell = \"echo done > out.txt\"\n"
    )
}

/// One rule plus a `[[sample_groups]]` cohort of the given sample count.
fn wf_cohort(shell: &str, samples: usize) -> String {
    let list = (0..samples)
        .map(|i| format!("\"s{i}\""))
        .collect::<Vec<_>>()
        .join(", ");
    format!(
        "[workflow]\nname = \"t\"\nversion = \"1.0\"\n\n[[sample_groups]]\nname = \"cohort\"\nsamples = [{list}]\n\n[[rules]]\nname = \"r1\"\noutput = [\"out.txt\"]\nshell = \"{shell}\"\n"
    )
}

/// Sample-group workflow whose aggregation rule is keyed by `{sample}` in its
/// input but whose output carries no fan-out wildcard (the SCI-AGG-RACE
/// shape, mirroring the source test's `multiqc` repro).
fn wf_race(output: &str) -> String {
    format!(
        "[workflow]\nname = \"t\"\nversion = \"1.0\"\n\n[[sample_groups]]\nname = \"cohort\"\nsamples = [\"S1\", \"S2\", \"S3\"]\n\n[[rules]]\nname = \"multiqc\"\ninput = [\"qc/{{sample}}/fastqc.html\"]\noutput = [\"{output}\"]\nshell = \"multiqc -f qc -o report\"\n"
    )
}

// ---- the shared fixture table -------------------------------------------

fn fixtures() -> Vec<Fx> {
    vec![
        // ===== variant calling =====
        fx3(
            "SCI-VQSR-COHORT",
            wf_cohort("gatk VariantRecalibrator -V in.vcf", 7),
            wf_cohort("gatk VariantRecalibrator -V in.vcf", 60),
            wf_mention_tok("variantrecalibrator"),
        ),
        fx3(
            "SCI-BQSR-NO-KNOWN-SITES",
            wf("gatk BaseRecalibrator -I aln.bam -O recal.table"),
            wf("gatk BaseRecalibrator -I aln.bam --known-sites dbsnp.vcf -O recal.table"),
            "[workflow]\nname = \"t\"\nversion = \"1.0\"\n\n[[rules]]\nname = \"r1\"\ninput = [\"baserecalibrator_plan.md\"]\noutput = [\"out.txt\"]\nshell = \"echo done > out.txt\"\n".to_string(),
        ),
        fx3(
            "SCI-MUTECT2-TUMOR-ONLY",
            wf("gatk Mutect2 -R ref.fa -I tumor.bam -O out.vcf"),
            wf("gatk Mutect2 -R ref.fa -I tumor.bam -I normal.bam --normal-sample N1 -O out.vcf"),
            "[workflow]\nname = \"t\"\nversion = \"1.0\"\n\n[[rules]]\nname = \"mutect2_howto\"\ninput = [\"mutect2_howto.txt\"]\noutput = [\"out.txt\"]\nshell = \"echo ok > out.txt\"\n".to_string(),
        ),
        fx3(
            "SCI-MUTECT2-NO-PON",
            wf("gatk Mutect2 -R ref.fa -I tumor.bam --tumor-only -O out.vcf"),
            wf(
                "gatk Mutect2 -R ref.fa -I tumor.bam --tumor-only --panel-of-normals pon.vcf -O out.vcf",
            ),
            "[workflow]\nname = \"t\"\nversion = \"1.0\"\n\n[[rules]]\nname = \"r1\"\ninput = [\"pon/readme.md\"]\noutput = [\"out.txt\"]\nshell = \"echo 'see panel-of-normals docs' > out.txt\"\n".to_string(),
        ),
        fx_skip(
            "SCI-MUTECT2-NO-GERMLINE",
            wf(
                "gatk Mutect2 -R ref.fa -I tumor.bam --tumor-only --panel-of-normals pon.vcf -O out.vcf",
            ),
            "requires `--panel-of-normals` present while `--germline-resource` absent; that already fires SCI-MUTECT2-NO-PON on the same shell, so a dedicated mention-only fixture would not be rule-distinguishing",
        ),
        fx3(
            "SCI-HAPLOTYPECALLER-GVCF-ERC",
            wf_o(
                "gatk HaplotypeCaller -R ref.fa -I aln.bam -O out.g.vcf",
                "out.g.vcf",
            ),
            wf_o(
                "gatk HaplotypeCaller -R ref.fa -I aln.bam -ERC GVCF -O out.g.vcf",
                "out.g.vcf",
            ),
            "[workflow]\nname = \"t\"\nversion = \"1.0\"\n\n[[rules]]\nname = \"r1\"\noutput = [\"notes.txt\"]\nshell = \"echo 'HaplotypeCaller gVCF notes' > notes.txt\"\n".to_string(),
        ),
        fx3(
            "SCI-GENOTYPEGVCFS-NO-NEW-QUAL",
            wf("gatk3 -T GenotypeGVCFs -R ref.fa -V cohort.vcf -o out.vcf"),
            wf("gatk3 -T GenotypeGVCFs -R ref.fa --new-qual -V cohort.vcf -o out.vcf"),
            "[workflow]\nname = \"t\"\nversion = \"1.0\"\n\n[[rules]]\nname = \"r1\"\ninput = [\"genotypegvcfs_notes.txt\"]\noutput = [\"out.txt\"]\nshell = \"echo ok > out.txt\"\n".to_string(),
        ),
        fx3(
            "SCI-DEEPVARIANT-MODEL-TYPE",
            wf("run_deepvariant --ref ref.fa --reads aln.bam --output_vcf out.vcf"),
            wf(
                "run_deepvariant --ref ref.fa --reads aln.bam --model_type=WGS --output_vcf out.vcf",
            ),
            "[workflow]\nname = \"t\"\nversion = \"1.0\"\n\n[[rules]]\nname = \"r1\"\ninput = [\"deepvariant_plan.md\"]\noutput = [\"out.txt\"]\nshell = \"echo ok > out.txt\"\n".to_string(),
        ),
        fx3(
            "SCI-MANTA-NO-CONFIG",
            wf("python configManta.py --tumorBam t.bam --normalBam n.bam"),
            wf("python configManta.py --referenceFasta ref.fa --tumorBam t.bam --normalBam n.bam"),
            "[workflow]\nname = \"t\"\nversion = \"1.0\"\n\n[[rules]]\nname = \"r1\"\ninput = [\"manta_config_notes.txt\"]\noutput = [\"out.txt\"]\nshell = \"echo ok > out.txt\"\n".to_string(),
        ),
        fx3(
            "SCI-STRELKA-NO-MANTA",
            wf("python configureStrelkaSomaticWorkflow.py --tumorBam t.bam --normalBam n.bam"),
            wf(
                "python configureStrelkaSomaticWorkflow.py --indelCandidates manta.vcf.gz --tumorBam t.bam",
            ),
            "[workflow]\nname = \"t\"\nversion = \"1.0\"\n\n[[rules]]\nname = \"r1\"\ninput = [\"strelka_somatic_plan.md\"]\noutput = [\"out.txt\"]\nshell = \"echo ok > out.txt\"\n".to_string(),
        ),
        fx3(
            "SCI-STRELKA-TARGETS-UNCOMPRESSED",
            wf(
                "configureStrelkaGermlineWorkflow.py --targetBed=targets.bed --referenceFasta ref.fa",
            ),
            wf(
                "configureStrelkaGermlineWorkflow.py --targetBed=targets.bed.gz --referenceFasta ref.fa",
            ),
            "[workflow]\nname = \"t\"\nversion = \"1.0\"\n\n[[rules]]\nname = \"r1\"\ninput = [\"targets.bed\"]\noutput = [\"out.txt\"]\nshell = \"bedtools intersect -a targets.bed -b regions.bed > out.txt\"\n".to_string(),
        ),
        fx3(
            "SCI-VEP-OFFLINE-CACHE",
            wf("vep -i in.vcf -o out.vcf"),
            wf("vep -i in.vcf --offline --cache --dir_cache /vep -o out.vcf"),
            "[workflow]\nname = \"t\"\nversion = \"1.0\"\n\n[[rules]]\nname = \"r1\"\ninput = [\"vep_plan.md\"]\noutput = [\"out.txt\"]\nshell = \"echo ok > out.txt\"\n".to_string(),
        ),
        // ===== alignment =====
        fx3(
            "SCI-BWA-MEM-NO-RG",
            wf("bwa mem ref.fa r1.fq r2.fq > aln.sam"),
            wf("bwa mem -R '@RG\\tID:S1\\tSM:S1' ref.fa r1.fq r2.fq > aln.sam"),
            wf_mention("bwa_notes", "bwa_mem_plan.md"),
        ),
        fx3(
            "SCI-BOWTIE2-NO-RG",
            wf("bowtie2 -x idx -1 r1.fq -2 r2.fq -S out.sam"),
            wf("bowtie2 --rg-id S1 --rg SM:S1 -x idx -1 r1.fq -2 r2.fq -S out.sam"),
            wf_mention("bowtie2_notes", "bowtie2_readme.md"),
        ),
        fx3(
            "SCI-MINIMAP2-PRESET",
            wf("minimap2 ref.fa reads.fq > aln.sam"),
            wf("minimap2 -x map-ont ref.fa reads.fq > aln.sam"),
            wf_mention("minimap2_notes", "minimap2_presets.md"),
        ),
        fx3(
            "SCI-SORT-THREADS-MEM-PRODUCT",
            wf_res(
                "samtools sort -@ 8 -m 4G aln.sam -o sorted.bam",
                "sorted.bam",
                8,
                "16G",
            ),
            wf_res(
                "samtools sort -@ 8 -m 1G aln.sam -o sorted.bam",
                "sorted.bam",
                8,
                "16G",
            ),
            wf_mention("sort_notes", "samtools_sort_plan.md"),
        ),
        fx3(
            "SCI-SAMTOOLS-INDEX-UNSORTED",
            wf_io("samtools index in.sam", "in.sam", "in.sam.bai"),
            wf_io("samtools index in.bam", "in.bam", "in.bam.bai"),
            wf_io(
                "samtools index sorted.bam",
                "methylation.sam.txt",
                "sorted.bam.bai",
            ),
        ),
        fx3(
            "SCI-MARKDUPLICATES-REMOVE-DUPS",
            wf(
                "picard MarkDuplicates I=sorted.bam O=dedup.bam M=metrics.txt REMOVE_DUPLICATES=true",
            ),
            wf(
                "picard MarkDuplicates I=sorted.bam O=dedup.bam M=metrics.txt REMOVE_DUPLICATES=false",
            ),
            wf_mention("markdup_notes", "markduplicates_plan.md"),
        ),
        fx3(
            "SCI-HISAT2-NO-SPLICE-SITES",
            wf("hisat2 -x idx -1 r1.fq -2 r2.fq -S out.sam"),
            wf("hisat2 --known-splicesite-infile splice.txt -x idx -1 r1.fq -2 r2.fq -S out.sam"),
            wf_mention("hisat2_notes", "hisat2_splice_plan.md"),
        ),
        fx3(
            "SCI-FASTP-DEDUPLICATION",
            wf("fastp -i r1.fq -o c1.fq -I r2.fq -O c2.fq --dedup"),
            wf("fastp -i r1.fq -o c1.fq -I r2.fq -O c2.fq --dedup --umi"),
            wf_mention("fastp_notes", "fastp_dedup_plan.md"),
        ),
        // ===== RNA-seq / single-cell =====
        fx3(
            "SCI-FEATURECOUNTS-STRAND",
            wf("featurecounts -a annotation.gtf -o counts.txt aln.bam"),
            wf("featurecounts -s 2 -a annotation.gtf -o counts.txt aln.bam"),
            wf_mention_tok("featurecounts"),
        ),
        fx3(
            "SCI-FEATURECOUNTS-MULTI-OVERLAP",
            wf("featurecounts -O -a annotation.gtf -o counts.txt aln.bam"),
            wf("featurecounts -s 2 -a annotation.gtf -o counts.txt aln.bam"),
            wf_mention_tok("featurecounts_multi"),
        ),
        fx3(
            "SCI-STAR-SJDB-OVERHANG",
            wf("STAR --runMode genomeGenerate --genomeDir idx --sjdbGTFfile anno.gtf"),
            wf(
                "STAR --runMode genomeGenerate --genomeDir idx --sjdbGTFfile anno.gtf --sjdbOverhang 149",
            ),
            wf_mention_tok("star_sjdb"),
        ),
        fx3(
            "SCI-STAR-GENOME-LOAD",
            wf("STAR --genomeLoad LoadAndKeep --genomeDir idx --readFilesIn r1.fq"),
            wf("STAR --genomeLoad NoSharedMemory --genomeDir idx --readFilesIn r1.fq"),
            wf_mention_tok("star_genomeload"),
        ),
        fx3(
            "SCI-SALMON-LIBTYPE",
            wf("salmon quant -i idx -l A -r reads.fq -o out"),
            wf("salmon quant -i idx -l ISR -r reads.fq -o out"),
            wf_mention_tok("salmon_libtype"),
        ),
        fx3(
            "SCI-SALMON-NO-GC-BIAS",
            wf("salmon quant -i idx -l ISR -r reads.fq -o out"),
            wf("salmon quant -i idx -l ISR --gcBias -r reads.fq -o out"),
            wf_mention_tok("salmon_gcbias"),
        ),
        fx3(
            "SCI-KALLISTO-SINGLE-END-FRAG",
            wf("kallisto quant -i idx --single -o out reads.fq"),
            wf("kallisto quant -i idx --single -l 200 -s 20 -o out reads.fq"),
            wf_mention_tok("kallisto"),
        ),
        fx3(
            "SCI-RSEM-NO-STRAND",
            wf("rsem-calculate-expression aln.bam ref out"),
            wf("rsem-calculate-expression --strand-specific aln.bam ref out"),
            wf_mention_tok("rsem"),
        ),
        fx3(
            "SCI-STRINGTIE-NO-GUIDE",
            wf("stringtie aln.bam -o out.gtf"),
            wf("stringtie aln.bam -G anno.gtf -o out.gtf"),
            wf_mention_tok("stringtie"),
        ),
        fx3(
            "SCI-DESEQ2-MIN-REPLICATES",
            wf_cohort("Rscript deseq2.R", 2),
            wf_cohort("Rscript deseq2.R", 4),
            wf_cohort("cat deseq2_results/notes.txt > out.txt", 4),
        ),
        fx3(
            "SCI-CELLRANGER-INTRONS",
            wf("cellranger count --id=out --snrna --transcriptome=ref"),
            wf("cellranger count --id=out --snrna --include-introns=true --transcriptome=ref"),
            wf_mention_tok("cellranger"),
        ),
        fx3(
            "SCI-SEURAT-NO-NORMALIZATION",
            wf("Rscript -e 'FindClusters(obj, resolution=0.5)'"),
            wf("Rscript -e 'obj <- NormalizeData(obj); FindClusters(obj)'"),
            wf_mention_tok("seurat_findclusters"),
        ),
        // ===== epigenomics =====
        fx3(
            "SCI-MACS2-GENOME-SIZE",
            wf("macs2 callpeak -t chip.bam -f BAM -c ctrl.bam --name g"),
            wf("macs2 callpeak -t chip.bam -f BAM -c ctrl.bam -g hs --name g"),
            wf_mention_tok("macs2_genomesize"),
        ),
        fx3(
            "SCI-MACS2-BROAD-PEAKS",
            wf("macs2 callpeak -t chip.bam -f BAM -c ctrl.bam -g hs --name H3K27me3"),
            wf("macs2 callpeak -t chip.bam -f BAM -c ctrl.bam -g hs --broad --name H3K27me3"),
            wf_mention_tok("macs2_broad"),
        ),
        fx3(
            "SCI-MACS2-PAIRED-END-FORMAT",
            wf_io(
                "macs2 callpeak -t chip.bam -f BAM -c ctrl.bam -g hs --name g",
                "pe/chip.bam",
                "out.txt",
            ),
            wf_io(
                "macs2 callpeak -t chip.bam -f BAMPE -c ctrl.bam -g hs --name g",
                "pe/chip.bam",
                "out.txt",
            ),
            wf_io(
                "macs2 callpeak -t chip.bam -f BAMPE -c ctrl.bam -g hs --name g",
                "paired/chip.bam",
                "out.txt",
            ),
        ),
        fx3(
            "SCI-CHIP-NO-CONTROL",
            wf("macs2 callpeak -t chip.bam -f BAM -g hs --name g"),
            wf("macs2 callpeak -t chip.bam -f BAM -c ctrl.bam -g hs --name g"),
            wf_mention_tok("macs2_control"),
        ),
        fx3(
            "SCI-ATAC-SHIFT-COORDINATES",
            wf("macs2 callpeak -t atac.bam -f BAM -g hs --name g"),
            wf(
                "alignmentSieve --ATACshift -b atac.bam -o shift.bam && macs2 callpeak -t shift.bam -f BAM -g hs --name g",
            ),
            wf_mention_tok("atac_shift"),
        ),
        fx3(
            "SCI-BISMARK-NON-DIRECTIONAL",
            wf("bismark --swift -1 r1.fq -2 r2.fq"),
            wf("bismark --non_directional -1 r1.fq -2 r2.fq"),
            wf_mention_tok("bismark_pbat"),
        ),
        fx3(
            "SCI-BOWTIE-BISULFITE-MISMATCH",
            wf("bismark --bowtie2 --score-min L,0,0 -1 r1.fq -2 r2.fq"),
            wf("bismark --bowtie2 --score-min L,0,-0.2 -1 r1.fq -2 r2.fq"),
            wf_mention_tok("bismark_scoremin"),
        ),
        fx3(
            "SCI-BEDTOOLS-GENOME-FILE",
            wf("bedtools slop -i in.bed -b 100 > out.bed"),
            wf("bedtools slop -i in.bed -b 100 -g chrom.sizes > out.bed"),
            wf_mention_tok("bedtools_slop"),
        ),
        // ===== metagenomics / microbiome =====
        fx3(
            "SCI-KRAKEN2-CONFIDENCE",
            wf("kraken2 --db db --output out.kraken reads.fa"),
            wf("kraken2 --db db --confidence 0.1 --output out.kraken reads.fa"),
            wf_mention("kraken2_notes", "kraken2_plan.md"),
        ),
        fx3(
            "SCI-BRACKEN-READ-LEN",
            wf("bracken -d db -i out.kraken -o out.bracken"),
            wf("bracken -d db -i out.kraken -r 150 -o out.bracken"),
            wf_mention("bracken_notes", "bracken_plan.md"),
        ),
        fx3(
            "SCI-QIIME2-DENOISE-TRIM",
            wf("qiime dada2 denoise-paired --p-trunc-len 0 --i-demultiplexed-seqs seqs.qza"),
            wf("qiime dada2 denoise-paired --p-trunc-len 150 --i-demultiplexed-seqs seqs.qza"),
            wf_mention_tok("qiime_dada2"),
        ),
        fx3(
            "SCI-HUMANN-NUCLEOTIDE-DB",
            wf("humann --input sample.fq --output out"),
            wf("humann --input sample.fq --nucleotide-database chocophlan --output out"),
            wf_mention("humann_notes", "humann_plan.md"),
        ),
        fx3(
            "SCI-METAPHLAN-UNKNOWN-ESTIMATION",
            wf("metaphlan --input_type fastq --unknown_estimation --input sample.fq"),
            wf(
                "metaphlan --input_type fastq --unknown_estimation --bowtie2db markers --input sample.fq",
            ),
            wf_mention("metaphlan_notes", "metaphlan_plan.md"),
        ),
        fx3(
            "SCI-CHECKM-NO-LINEAGE",
            wf("checkm lineage_wf bins out"),
            wf("checkm lineage_wf -x fa bins out"),
            wf_mention("checkm_notes", "checkm_lineage_plan.md"),
        ),
        // ===== concurrency / hardware / dataflow =====
        fx3(
            "SCI-TMPDIR-NOT-SCRATCH",
            wf_o(
                "samtools sort -T /tmp/sort aln.bam -o sorted.bam",
                "sorted.bam",
            ),
            wf_o(
                "samtools sort -T ${TMPDIR}/sort aln.bam -o sorted.bam",
                "sorted.bam",
            ),
            wf_mention("samtools_notes", "samtools_sort_plan.md"),
        ),
        fx3(
            "SCI-PIPE-FAIL-DISABLED",
            wf_o(
                "samtools view -b aln.sam | samtools sort -o sorted.bam | samtools index sorted.bam",
                "sorted.bam",
            ),
            wf_o(
                "set -eo pipefail; samtools view -b aln.sam | samtools sort -o sorted.bam | samtools index sorted.bam",
                "sorted.bam",
            ),
            wf_o(
                "echo 'a | b | c pipeline without pipefail' > solid.txt",
                "solid.txt",
            ),
        ),
        fx3(
            "SCI-ZERO-BYTE-COMPLETION",
            wf_o(
                "samtools sort aln.bam -o sorted.bam || touch sorted.bam",
                "sorted.bam",
            ),
            wf_o("samtools sort aln.bam -o sorted.bam", "sorted.bam"),
            wf_o("touch log.bam.sha256", "log.bam.sha256"),
        ),
        fx3(
            "SCI-MULTI-TENANT-PORT-COLLISION",
            wf("jupyter lab --port 8888 --no-browser"),
            wf("jupyter lab --port ${PORT:-0} --no-browser"),
            wf_mention_tok("jupyter_port"),
        ),
        fx3(
            "SCI-AGG-RACE",
            wf_race("report/multiqc.html"),
            wf_race("report/{sample}/multiqc.html"),
            wf_mention_tok("multiqc_summary"),
        ),
    ]
}

fn codes_of(toml: &str) -> Vec<String> {
    let cfg = WorkflowConfig::parse(toml).expect("fixture TOML must parse");
    analyze_scientific_constraints(&cfg)
        .into_iter()
        .map(|w| w.code)
        .collect()
}

fn run_row(fx: &Fx) {
    let pos = codes_of(&fx.positive);
    assert!(
        pos.iter().any(|c| c == fx.code),
        "positive fixture MUST fire {} but got {:?}",
        fx.code,
        pos
    );
    if let Some(cor) = &fx.corrected {
        let got = codes_of(cor);
        assert!(
            !got.iter().any(|c| c == fx.code),
            "corrected fixture MUST silence {} but got {:?}",
            fx.code,
            got
        );
    }
    if let Some(men) = &fx.mention_only {
        let got = codes_of(men);
        assert!(
            !got.iter().any(|c| c == fx.code),
            "mention-only fixture MUST keep {} silent but got {:?}",
            fx.code,
            got
        );
    }
}

#[test]
fn adversarial_table_positive_and_negatives() {
    let rows = fixtures();
    for fx in &rows {
        run_row(fx);
    }
}

/// The distinct `code: "SCI-…"` ids the engine can emit, parsed from the
/// source at compile time so the id list is never retyped here.
fn source_rule_ids() -> BTreeSet<String> {
    let src = include_str!("../src/scientific_preflight.rs");
    let mut ids = BTreeSet::new();
    for part in src.split("code: \"").skip(1) {
        if let Some(end) = part.find('"') {
            let id = &part[..end];
            if id.starts_with("SCI-") {
                ids.insert(id.to_string());
            }
        }
    }
    ids
}

#[test]
fn fixture_table_covers_every_source_rule_exactly_once() {
    let src_ids = source_rule_ids();
    assert!(
        !src_ids.is_empty(),
        "failed to parse any SCI- id from source"
    );

    let rows = fixtures();
    let mut seen: BTreeSet<&str> = BTreeSet::new();
    let mut dups: Vec<&str> = Vec::new();
    for fx in &rows {
        if !seen.insert(fx.code) {
            dups.push(fx.code);
        }
    }
    assert!(dups.is_empty(), "duplicate fixture rows: {dups:?}");

    let table_ids: BTreeSet<String> = rows.iter().map(|f| f.code.to_string()).collect();
    assert_eq!(
        table_ids, src_ids,
        "fixture table must enumerate exactly the source rule ids"
    );
    assert_eq!(
        table_ids.len(),
        51,
        "expected 51 curated SCI-* rules in source"
    );

    // Every non-skipped rule must be fully exercised; skipped rules must
    // carry a reason (the NOT_ADVERSARIALLY_EXERCISED list).
    for fx in &rows {
        if fx.skip.is_none() {
            assert!(
                fx.corrected.is_some() && fx.mention_only.is_some(),
                "rule {} is neither fully exercised nor listed as not-exercised",
                fx.code
            );
        } else {
            assert!(fx.corrected.is_none() && fx.mention_only.is_none());
        }
    }

    let not_exercised: Vec<(&str, &str)> = rows
        .iter()
        .filter_map(|f| f.skip.map(|r| (f.code, r)))
        .collect();
    eprintln!(
        "NOT_ADVERSARIALLY_EXERCISED ({} of {}):",
        not_exercised.len(),
        rows.len()
    );
    for (code, reason) in &not_exercised {
        eprintln!("  - {code}: {reason}");
    }
}

// ---- measured latency ----------------------------------------------------

/// Timing harness: a synthetic workflow with a large rule count and a wide
/// fan-out, timed end to end through `analyze_scientific_constraints`.
/// Prints the measured elapsed time; the assertion is a generous
/// non-flaky ceiling.
#[test]
fn scientific_preflight_latency_large_workflow() {
    use std::time::Instant;

    const RULES: usize = 200;
    const SAMPLES: usize = 1000;

    let list = (0..SAMPLES)
        .map(|i| format!("\"s{i}\""))
        .collect::<Vec<_>>()
        .join(", ");
    let mut toml = format!(
        "[workflow]\nname = \"perf\"\nversion = \"1.0\"\n\n[[sample_groups]]\nname = \"c\"\nsamples = [{list}]\n"
    );
    for i in 0..RULES {
        toml.push_str(&format!(
            "\n[[rules]]\nname = \"r{i}\"\ninput = [\"in/{{sample}}.bam\"]\noutput = [\"out/{{sample}}.bam\"]\nshell = \"samtools sort -@ 4 -o out/{{sample}}.bam in/{{sample}}.bam\"\n"
        ));
    }
    let cfg = WorkflowConfig::parse(&toml).unwrap();

    let start = Instant::now();
    let warnings = analyze_scientific_constraints(&cfg);
    let elapsed = start.elapsed();

    eprintln!(
        "scientific_preflight latency: {RULES} rules x {SAMPLES} samples = {elapsed:?} ({} warnings)",
        warnings.len()
    );
    assert!(
        elapsed.as_secs() < 5,
        "analyze_scientific_constraints took {elapsed:?} — regression"
    );
}
