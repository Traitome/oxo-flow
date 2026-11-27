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
//      static fixture cannot evaluate `workflow.containerEngine`). The
//      module-local conda `environment.yml` pins `htslib=1.25` and
//      `samtools=1.25`; the fixture materializes only
//      `bioconda::samtools=1.25` because htslib is samtools' own library
//      dependency and `samtools` is the invoked binary (no behavior change).
//   4. Resources are materialized from the nf-core `process_single` label
//      default (base.config: cpus 1, memory 6.GB, time 4h), with cpus 2 and
//      memory lowered to 2 GB so the task fits the 3 GiB qualification host
//      (Nextflow refuses to schedule a 6 GB task against ~3.6 GB available).
//      cpus 2 also keeps the non-default thread count explicit through the
//      export round trip (the exporter omits `threads = 1`).
//   5. `get_sizes_command` is dropped because `get_sizes` is not available in
//      the wrapper.
//   6. A `workflow { }` call is added because the upstream file is a module
//      only (it carries no workflow block).
//   7. `publishDir "GENOME_PATH", mode: 'copy'` is added because nf-core
//      modules publish at the pipeline level, not in the module; without it
//      Nextflow leaves the declared output inside `work/`, so a standalone run
//      produces no artifact in the launch directory (observed in the first
//      campaign attempt: exit 0, no file).
//   8. The upstream `stub:` block is dropped: it is the test-only body the
//      importer ignores (it never runs in a real execution), and keeping it
//      would add directives the static fixture does not need.
// The `samtools faidx` command, the container image and the module identifier
// are upstream. The fixture's declared output is one text index, so the
// execution-equivalence harness compares deterministic bytes.
process SAMTOOLS_FAIDX {
    tag "${fasta}"
    label 'process_single'

    conda "bioconda::samtools=1.25"
    container "community.wave.seqera.io/library/htslib_samtools:1.25--704fb80d75745bd4"

    cpus 2
    memory '2 GB'
    time '4h'

    publishDir "GENOME_PATH", mode: 'copy'

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
