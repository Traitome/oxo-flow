// EXPECT-ERROR: never invoked
process FASTQC {
    input: path reads
    output: "${reads}.html"
    script: "fastqc $reads"
}
