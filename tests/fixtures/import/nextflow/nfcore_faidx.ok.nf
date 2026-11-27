// Derived fixture — upstream: nf-core/modules modules/nf-core/samtools/faidx/main.nf
// (https://github.com/nf-core/modules/blob/49d0ca7a887eeb1b85cfb68ab18c8039753bd1de/modules/nf-core/samtools/faidx/main.nf,
// retrieved 2026-11-27). This is the real nf-core module, wrapped minimally so it
// runs standalone and stays inside the importer's documented subset. Every
// wrapper change is listed here; nothing else is altered:
//   1. The channel input `tuple val(meta), path(fasta), path(fai)` plus the
//      `val get_sizes` argument collapse to `path fasta`: the importer has no
//      tuple/val channel syntax, and the standalone wrapper feeds one FASTA
//      path. `meta`/`get_sizes` are unused by the faidx command itself.
//   2. Outputs reduce to `path "${fasta}.fai"`: the module's optional
//      fa/sizes/gzi outputs are not produced without extra args, and the
//      `versions` topic is not a file artifact.
//   3. The dynamic container ternary is materialized to its docker branch (a
//      static fixture cannot evaluate `workflow.containerEngine`), and the
//      module-local conda `environment.yml` reference is materialized to the
//      literal spec it contains.
//   4. Resources are materialized from the nf-core `process_single` label
//      default (base.config: cpus 1, memory 6.GB, time 4h).
//   5. `get_sizes_command` is dropped because `get_sizes` is not available in
//      the wrapper.
//   6. A `workflow { }` call is added because the upstream file is a module
//      only (it carries no workflow block).
// The `samtools faidx` command, the container image and the module identifier
// are upstream. The fixture's declared output is one text index, so the
// execution-equivalence harness compares deterministic bytes.
process SAMTOOLS_FAIDX {
    tag "${fasta}"
    label 'process_single'

    conda "bioconda::samtools=1.25"
    container "community.wave.seqera.io/library/htslib_samtools:1.25--704fb80d75745bd4"

    cpus 1
    memory '6 GB'
    time '4h'

    input:
    path fasta

    output:
    path "${fasta}.fai", emit: fai

    when:
    task.ext.when == null || task.ext.when

    script:
    def args = task.ext.args ?: ''
    """
    samtools \\
        faidx \\
        ${fasta} \\
        ${args}
    """
}

workflow {
    SAMTOOLS_FAIDX(Channel.fromPath("GENOME_PATH/genome.fna"))
}
