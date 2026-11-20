// EXPECT-ERROR: unresolved Nextflow interpolation
// Derived fixture — upstream: nextflow-io/rnaseq-nf modules/fastqc/main.nf
// (https://github.com/nextflow-io/rnaseq-nf/blob/master/modules/fastqc/main.nf,
// retrieved 2026-11-20). The module binds its samples to a bare `id` value rather
// than the `meta.id` metadata tuple form the importer's documented subset understands,
// so every `${id}` interpolation is an unresolved Nextflow variable the importer refuses
// to emit as literal shell text. Process name, resources, conda spec and shell are upstream.
process FASTQC {
    tag "${id}"
    conda 'bioconda::fastqc=0.12.1'

    input:
    tuple val(id), path(fastq_1), path(fastq_2)

    output:
    tuple val(id), path("fastqc_${id}_logs")

    script:
    """
    fastqc.sh "${id}" "${fastq_1} ${fastq_2}"
    """
}
