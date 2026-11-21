// Execution-equivalence fixture: rnaseq_chain (Nextflow DSL2 arm).
// Same intent as workflow.oxoflow: trim -> per-sample count -> fan-in summary.
// Same pinned container digest, same relative output paths.
nextflow.enable.dsl = 2

params.samples = ['s1', 's2', 's3']

process TRIM {
    container 'python@sha256:5e2bdb1b0d542d5039cffb8181c65930093949194a65c365dfb1b0fe13a4fa5b'
    publishDir 'trimmed', mode: 'copy'

    input:
    tuple val(sample), path(fq, stageAs: 'in.fastq')

    output:
    tuple val(sample), path("${sample}.fastq")

    script:
    """
    export LC_ALL=C
    grep -v '^\$' ${fq} > ${sample}.fastq
    """
}

process COUNT {
    container 'python@sha256:5e2bdb1b0d542d5039cffb8181c65930093949194a65c365dfb1b0fe13a4fa5b'
    publishDir 'counts', mode: 'copy'

    input:
    tuple val(sample), path(fq)

    output:
    tuple val(sample), path("${sample}.tsv")

    script:
    """
    export LC_ALL=C
    echo "sample: ${sample}" > ${sample}.tsv
    echo "reads: \$(grep -c '^@' ${fq})" >> ${sample}.tsv
    echo "bases: \$(awk 'NR%4==2 {n+=length(\$0)} END {print n+0}' ${fq})" >> ${sample}.tsv
    """
}

process AGGREGATE {
    container 'python@sha256:5e2bdb1b0d542d5039cffb8181c65930093949194a65c365dfb1b0fe13a4fa5b'
    publishDir 'report', mode: 'copy'

    input:
    path tsvs

    output:
    path 'summary.tsv'

    script:
    def files = tsvs.collect { it.toString() }.sort()
    """
    export LC_ALL=C
    cat ${files.join(' ')} > summary.tsv
    """
}

workflow {
    Channel.fromList(params.samples.collect { s -> [s, file("data/${s}.fastq")] }) \
        | TRIM \
        | COUNT \
        | map { s, t -> t } \
        | collect \
        | AGGREGATE
}
