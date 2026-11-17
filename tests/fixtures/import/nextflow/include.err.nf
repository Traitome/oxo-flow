// EXPECT-ERROR: include
include { FASTQC } from './modules/fastqc'

workflow {
    FASTQC(Channel.fromPath("data/*.fastq.gz"))
}
