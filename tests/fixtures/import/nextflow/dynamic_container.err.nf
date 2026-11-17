// EXPECT-ERROR: dynamic
process ALIGN {
    container "${params.container}"
    input: path reads
    output: "${reads}.bam"
    script: "align $reads > ${reads}.bam"
}
workflow { ALIGN(Channel.fromPath("data/*.fastq.gz")) }
