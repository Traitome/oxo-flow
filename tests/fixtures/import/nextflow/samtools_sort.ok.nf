// Derived fixture — upstream: nf-core/modules modules/nf-core/samtools/sort/main.nf
// (https://github.com/nf-core/modules/blob/master/modules/nf-core/samtools/sort/main.nf,
// retrieved 2026-11-20). Faithful derivation for a STATIC fixture: the dynamic
// `conda "${moduleDir}/environment.yml"` was materialized to its literal spec and
// the engine-ternary `container` collapsed to its docker branch (the runtime
// default). Process name is upstream; resources are materialized from the
// `process_medium` label; I/O is reduced to the single pinned bam pair. Only the
// `prefix` line is copied verbatim from upstream line 30 (it carries finding O12);
// the shell body is adapted/trimmed from upstream lines 66-74 (dynamic
// reconciliation dropped for a static fixture).
process SAMTOOLS_SORT {
    tag "${meta.id}"
    label 'process_medium'

    conda "bioconda::samtools=1.25"
    container "community.wave.seqera.io/library/htslib_samtools:1.25--704fb80d75745bd4"

    cpus 2
    memory '8 GB'
    time '4h'

    input:
    tuple val(meta), path(bam)

    output:
    tuple val(meta), path("${prefix}.bam"), emit: bam

    when:
    task.ext.when == null || task.ext.when

    script:
    def args = task.ext.args ?: ''
    prefix = task.ext.prefix ?: "${meta.id}"
    """
    samtools sort \\
        ${args} \\
        -T ${prefix} \\
        --threads ${task.cpus} \\
        -o ${prefix}.bam \\
        ${bam}
    """
}

workflow {
    SAMTOOLS_SORT(Channel.fromPath("data/*.bam"))
}
