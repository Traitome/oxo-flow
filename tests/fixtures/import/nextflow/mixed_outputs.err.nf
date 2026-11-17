// EXPECT-ERROR: constant output
// A wildcard fan-out plus a constant output: Nextflow gives each task its own
// workdir, but oxo-flow shares the rule workdir, so every instance would
// overwrite summary.txt.
process MIXED {
    input: path reads
    output:
    tuple val(meta), path("${meta.id}.bam")
    path("summary.txt")
    script: "align $reads > ${meta.id}.bam; echo ok > summary.txt"
}
workflow { MIXED(Channel.fromPath("data/*.fastq.gz")) }
