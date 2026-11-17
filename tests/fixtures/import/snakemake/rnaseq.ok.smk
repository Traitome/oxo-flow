# Representative supported Snakemake pipeline: named I/O, threads/resources,
# conda environments, log/benchmark declarations, and rules.X.output wiring.
rule trim:
    input:
        r1="data/{sample}_1.fastq.gz",
        r2="data/{sample}_2.fastq.gz"
    output:
        r1="trimmed/{sample}_1.fastq.gz",
        r2="trimmed/{sample}_2.fastq.gz"
    threads: 4
    resources:
        mem_mb=8000,
        runtime="2h"
    conda:
        "envs/trimmomatic.yaml"
    log:
        "logs/trim/{sample}.log"
    benchmark:
        "bench/trim/{sample}.tsv"
    shell:
        "trimmomatic PE -threads {threads} {input.r1} {input.r2} {output.r1} /dev/null {output.r2} /dev/null 2> {log}"

rule align:
    input:
        r1="trimmed/{sample}_1.fastq.gz",
        r2="trimmed/{sample}_2.fastq.gz"
    output:
        bam="aligned/{sample}.bam"
    threads: 8
    resources:
        mem_mb=32000
    conda:
        "envs/star.yaml"
    shell:
        "STAR --runThreadN {threads} --readFilesIn {input.r1} {input.r2} --outFileNamePrefix aligned/{sample}."

rule count:
    input:
        bam="aligned/{sample}.bam"
    output:
        "counts/{sample}.txt"
    resources:
        mem_mb=4000,
        gpu=0
    container:
        "docker://biocontainers/subread:2.0.6"
    shell:
        "featureCounts -T 2 -a genes.gtf -o {output} {input.bam}"

rule all:
    input:
        rules.count.output
