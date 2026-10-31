#![allow(deprecated)]
/// Integration tests for Nextflow and Snakemake transpilation into oxo-flow workflows.
use oxo_flow_core::config::WorkflowConfig;
use oxo_flow_core::dag::WorkflowDag;
use oxo_flow_core::transpile::{SourceFormat, transpile};

#[test]
fn test_transpile_nf_core_fastqc_module() {
    let nf_fastqc = r#"
process FASTQC {
    tag "$meta.id"
    label 'process_single'

    conda "bioconda::fastqc=0.12.1"
    container "biocontainers/fastqc:0.12.1--hdfd78af_0"

    cpus 2
    memory '4 GB'
    time '4h'

    input:
    tuple val(meta), path(reads)

    output:
    tuple val(meta), path("*.html"), emit: html
    tuple val(meta), path("*.zip") , emit: zip
    path  "versions.yml"           , emit: versions

    when:
    task.ext.when == null || task.ext.when

    script:
    def args = task.ext.args ?: ''
    def prefix = task.ext.prefix ?: "${meta.id}"
    """
    fastqc \
        $args \
        --threads $task.cpus \
        $reads

    cat <<-END_VERSIONS > versions.yml
    "${task.process}":
        fastqc: $( fastqc --version | sed -e "s/FastQC v//g" )
    END_VERSIONS
    """
}
    "#;

    let toml_str = transpile(nf_fastqc, SourceFormat::Nextflow).expect("transpile should succeed");
    assert!(toml_str.contains("name = \"fastqc\""));
    assert!(toml_str.contains("threads = 2"));
    assert!(toml_str.contains("fastqc"));
    assert!(!toml_str.contains("END_VERSIONS")); // Versions block stripped

    let config =
        WorkflowConfig::parse(&toml_str).expect("generated toml must parse as valid workflow");
    assert_eq!(config.rules.len(), 1);
    assert_eq!(config.rules[0].name, "fastqc");
    assert_eq!(config.rules[0].threads, Some(2));

    let dag = WorkflowDag::from_rules(&config.rules).expect("dag should construct cleanly");
    assert_eq!(dag.node_count(), 1);
}

#[test]
fn test_transpile_nf_core_bwa_mem_module() {
    let nf_bwa = r#"
process BWA_MEM {
    tag "$meta.id"
    label 'process_high'

    conda "bioconda::bwa=0.7.17 bioconda::samtools=1.19.2"
    container "biocontainers/mulled-v2-fe8caa4220cc41a6caa800b93d839e77ff3e2fec:1005232b6947690184421b1b0fe300895318c4b7-0"

    cpus 8
    memory '16 GB'
    time '8h'

    input:
    tuple val(meta), path(reads)
    tuple val(meta2), path(index)

    output:
    tuple val(meta), path("*.bam"), emit: bam

    script:
    """
    bwa mem -t $task.cpus $index $reads | samtools sort -@ $task.cpus -o ${meta.id}.sorted.bam -
    """
}
    "#;

    let toml_str = transpile(nf_bwa, SourceFormat::Nextflow).expect("transpile should succeed");
    assert!(toml_str.contains("name = \"bwa_mem\""));
    assert!(toml_str.contains("threads = 8"));
    assert!(toml_str.contains("bwa mem -t {threads}"));
    assert!(toml_str.contains("types = {"));

    let config =
        WorkflowConfig::parse(&toml_str).expect("generated toml must parse as valid workflow");
    assert_eq!(config.rules.len(), 1);
    assert_eq!(config.rules[0].name, "bwa_mem");
    assert_eq!(config.rules[0].threads, Some(8));
}

#[test]
fn test_transpile_multi_process_nextflow_pipeline() {
    let nf_pipeline = r#"
process FASTQC {
    cpus 2
    input:
    tuple val(meta), path(reads)
    output:
    tuple val(meta), path("*.zip"), emit: zip
    script:
    """
    fastqc --threads $task.cpus $reads
    """
}

process SAMTOOLS_SORT {
    cpus 4
    memory '8 GB'
    input:
    tuple val(meta), path(bam)
    output:
    tuple val(meta), path("*.sorted.bam"), emit: bam
    script:
    """
    samtools sort -@ $task.cpus -o ${prefix}.sorted.bam $bam
    """
}
    "#;

    let toml_str =
        transpile(nf_pipeline, SourceFormat::Nextflow).expect("transpile should succeed");
    let config = WorkflowConfig::parse(&toml_str).expect("multi-rule toml must parse");
    assert_eq!(config.rules.len(), 2);
    assert_eq!(config.rules[0].name, "fastqc");
    assert_eq!(config.rules[1].name, "samtools_sort");
}

#[test]
fn test_transpile_snakemake_multi_rule_pipeline() {
    let smk_pipeline = r#"
rule fastqc:
    input:
        "data/{sample}.fastq.gz"
    output:
        "qc/{sample}_fastqc.html",
        "qc/{sample}_fastqc.zip"
    threads: 2
    conda:
        "envs/fastqc.yaml"
    shell:
        "fastqc -t {threads} {input} -o qc/"

rule bwa_mem:
    input:
        reads="data/{sample}.fastq.gz",
        ref="refs/genome.fa"
    output:
        bam="aligned/{sample}.sorted.bam",
        bai="aligned/{sample}.sorted.bam.bai"
    threads: 8
    resources:
        mem_mb=16000,
        time="2h"
    conda:
        "envs/bwa.yaml"
    container:
        "docker://biocontainers/bwa:0.7.17"
    shell:
        """
        bwa mem -t {threads} {input.ref} {input.reads} | samtools sort -@ {threads} -o {output.bam} -
        samtools index {output.bam}
        """
    "#;

    let toml_str =
        transpile(smk_pipeline, SourceFormat::Snakemake).expect("transpile should succeed");
    assert!(toml_str.contains("name = \"fastqc\""));
    assert!(toml_str.contains("name = \"bwa_mem\""));
    assert!(toml_str.contains("threads = 8"));
    assert!(toml_str.contains("memory = \"16G\""));
    assert!(toml_str.contains("time_limit = \"2h\""));
    assert!(toml_str.contains("docker = \"biocontainers/bwa:0.7.17\""));

    let config =
        WorkflowConfig::parse(&toml_str).expect("generated toml must parse as valid workflow");
    assert_eq!(config.rules.len(), 2);
    assert_eq!(config.rules[0].name, "fastqc");
    assert_eq!(config.rules[1].name, "bwa_mem");
    assert_eq!(config.rules[1].threads, Some(8));
}

#[test]
fn test_transpile_snakemake_temp_protected_flags() {
    let smk = r#"
rule call_variants:
    input:
        "aligned/{sample}.bam"
    output:
        vcf=protected("variants/{sample}.vcf.gz"),
        tmp_idx=temp("tmp/{sample}.idx")
    threads: 4
    shell:
        "freebayes -f ref.fa {input} > {output.vcf}"
    "#;

    let toml_str = transpile(smk, SourceFormat::Snakemake).expect("transpile should succeed");
    assert!(toml_str.contains("protected_output"));
    assert!(toml_str.contains("temporary = true"));

    let config = WorkflowConfig::parse(&toml_str).expect("toml must parse");
    assert_eq!(config.rules.len(), 1);
    assert!(!config.rules[0].protected_output.is_empty());
    assert!(config.rules[0].temporary);
}

#[test]
fn test_transpile_nf_core_star_align() {
    let nf = r#"
process STAR_ALIGN {
    tag "$meta.id"
    label 'process_high'

    conda "bioconda::star=2.7.10b bioconda::samtools=1.19.2"
    container "biocontainers/mulled-v2-1fa26bc1b1063e1e4f621522747b95ec823d6a36:ebd8424e8b07d80e34c4e0435b80fcdcf910543e-0"

    cpus 12
    memory '32 GB'
    time '6h'

    input:
    tuple val(meta), path(reads)
    path(index)

    output:
    tuple val(meta), path("*Aligned.sortedByCoord.out.bam"), emit: bam

    script:
    """
    STAR \
        --genomeDir $index \
        --readFilesIn $reads \
        --runThreadN $task.cpus \
        --outSAMtype BAM SortedByCoordinate
    """
}
    "#;

    let toml_str = transpile(nf, SourceFormat::Nextflow).expect("transpile should succeed");
    let config = WorkflowConfig::parse(&toml_str).expect("parsed workflow");
    assert_eq!(config.rules.len(), 1);
    assert_eq!(config.rules[0].threads, Some(12));
    assert!(config.rules[0].types.is_some());

    let dag = WorkflowDag::from_rules(&config.rules).expect("dag valid");
    assert_eq!(dag.node_count(), 1);
}

#[test]
fn test_transpile_nf_core_salmon_quant() {
    let nf = r#"
process SALMON_QUANT {
    tag "$meta.id"
    label 'process_medium'

    conda "bioconda::salmon=1.10.1"
    container "biocontainers/salmon:1.10.1--h7e5ed60_0"

    cpus 4
    memory '12 GB'

    input:
    tuple val(meta), path(reads)
    path(index)

    output:
    tuple val(meta), path("quant.sf"), emit: counts

    script:
    """
    salmon quant \
        -i $index \
        -l A \
        -1 ${reads[0]} \
        -2 ${reads[1]} \
        -p $task.cpus \
        -o .
    """
}
    "#;

    let toml_str = transpile(nf, SourceFormat::Nextflow).expect("transpile should succeed");
    let config = WorkflowConfig::parse(&toml_str).expect("parsed workflow");
    assert_eq!(config.rules.len(), 1);
    assert_eq!(config.rules[0].name, "salmon_quant");
    assert_eq!(config.rules[0].threads, Some(4));
}

#[test]
fn test_transpile_snakemake_trimmomatic_to_star_pipeline() {
    let smk = r#"
rule trimmomatic:
    input:
        r1="data/{sample}_1.fastq.gz",
        r2="data/{sample}_2.fastq.gz"
    output:
        r1="trimmed/{sample}_1.paired.fastq.gz",
        r2="trimmed/{sample}_2.paired.fastq.gz"
    threads: 4
    resources:
        mem_mb=8000
    conda:
        "envs/trimmomatic.yaml"
    shell:
        "trimmomatic PE -threads {threads} {input.r1} {input.r2} {output.r1} /dev/null {output.r2} /dev/null LEADING:3 TRAILING:3 SLIDINGWINDOW:4:15 MINLEN:36"

rule star_align:
    input:
        r1="trimmed/{sample}_1.paired.fastq.gz",
        r2="trimmed/{sample}_2.paired.fastq.gz"
    output:
        bam="aligned/{sample}.Aligned.sortedByCoord.out.bam"
    threads: 8
    resources:
        mem_mb=32000
    conda:
        "envs/star.yaml"
    shell:
        "STAR --runThreadN {threads} --genomeDir refs/star_index --readFilesIn {input.r1} {input.r2} --readFilesCommand zcat --outSAMtype BAM SortedByCoordinate --outFileNamePrefix aligned/{sample}."
    "#;

    let toml_str = transpile(smk, SourceFormat::Snakemake).expect("transpile should succeed");
    let config = WorkflowConfig::parse(&toml_str).expect("parsed workflow");
    assert_eq!(config.rules.len(), 2);
    assert_eq!(config.rules[0].name, "trimmomatic");
    assert_eq!(config.rules[1].name, "star_align");

    let dag = WorkflowDag::from_rules(&config.rules).expect("dag valid");
    assert_eq!(dag.node_count(), 2);
    // Rule dependency trimmomatic -> star_align
    assert_eq!(dag.edge_count(), 1);
}

#[test]
fn test_transpile_snakemake_samtools_index_and_flagstat() {
    let smk = r#"
rule samtools_index:
    input:
        "aligned/{sample}.bam"
    output:
        "aligned/{sample}.bam.bai"
    threads: 2
    shell:
        "samtools index -@ {threads} {input}"

rule samtools_flagstat:
    input:
        bam="aligned/{sample}.bam",
        bai="aligned/{sample}.bam.bai"
    output:
        "stats/{sample}.flagstat.txt"
    threads: 1
    shell:
        "samtools flagstat {input.bam} > {output}"
    "#;

    let toml_str = transpile(smk, SourceFormat::Snakemake).expect("transpile should succeed");
    let config = WorkflowConfig::parse(&toml_str).expect("parsed workflow");
    assert_eq!(config.rules.len(), 2);

    let dag = WorkflowDag::from_rules(&config.rules).expect("dag valid");
    assert_eq!(dag.node_count(), 2);
    assert_eq!(dag.edge_count(), 1);
}
