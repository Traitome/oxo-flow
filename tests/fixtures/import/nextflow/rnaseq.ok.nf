// Representative supported Nextflow DSL2 pipeline: tuple (metadata, path)
// channels, named emits, resource directives, and workflow wiring.
process TRIM {
    tag "$meta.id"
    label 'process_single'
    conda "bioconda::trimmomatic=0.39"
    cpus 4
    memory '8 GB'
    time '2h'

    input:
    tuple val(meta), path(reads)

    output:
    tuple val(meta), path("${meta.id}.trimmed.fastq.gz"), emit: trimmed

    script:
    """
    trimmomatic SE -threads $task.cpus $reads ${meta.id}.trimmed.fastq.gz LEADING:3
    """
}

process ALIGN {
    conda "bioconda::star=2.7.10b"
    cpus 12
    memory '32 GB'
    time '6h'

    input:
    tuple val(meta), path(reads)

    output:
    tuple val(meta), path("${meta.id}.bam"), emit: bam

    script:
    """
    STAR --readFilesIn $reads --runThreadN $task.cpus --outFileNamePrefix ${meta.id}.
    """
}

process COUNT {
    conda "bioconda::subread=2.0.6"
    cpus 2

    input:
    tuple val(meta), path(bam)

    output:
    tuple val(meta), path("${meta.id}.counts"), emit: counts

    script:
    """
    featureCounts -T $task.cpus -a genes.gtf -o ${meta.id}.counts $bam
    """
}

workflow {
    TRIM(Channel.fromPath("data/*.fastq.gz"))
    ALIGN(TRIM.out.trimmed)
    COUNT(ALIGN.out.bam)
}
