// EXPECT-ERROR: bare glob
process ALIGN {
    input: path reads
    output: path("*.bam")
    script: "align $reads"
}
workflow { ALIGN(Channel.fromPath("data/*.fastq.gz")) }
