// EXPECT-ERROR: is empty or dynamic
// Derived fixture — upstream: nf-core/modules modules/nf-core/samtools/faidx/main.nf
// (https://github.com/nf-core/modules/blob/master/modules/nf-core/samtools/faidx/main.nf,
// retrieved 2026-11-20). Every community nf-core module declares its environment as
// `conda "${moduleDir}/environment.yml"` — a pointer resolved from the module's
// directory at runtime, never a literal spec — which the importer cannot turn into a
// reproducible environment, so it refuses the module. Process name, the dynamic conda
// line, the ternary container and the I/O are unchanged from upstream.
process SAMTOOLS_FAIDX {
    tag "${meta.id}"

    conda "${moduleDir}/environment.yml"
    container "${ workflow.containerEngine in ['singularity', 'apptainer'] ?
        'community.wave.seqera.io/library/htslib_samtools:1.25--704fb80d75745bd4' :
        'community.wave.seqera.io/library/htslib_samtools:1.25--704fb80d75745bd4' }"

    input:
    tuple val(meta), path(fasta), path(fai)

    output:
    tuple val(meta), path("*.fai"), emit: fai

    script:
    """
    samtools faidx ${fasta}
    """
}
