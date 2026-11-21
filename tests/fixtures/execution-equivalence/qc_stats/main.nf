// Execution-equivalence fixture: qc_stats (Nextflow DSL2 arm).
// One stage: per-sample QC statistics (reads, bases, GC).
// Same pinned container digest, same relative output paths.
nextflow.enable.dsl = 2

params.samples = ['s1', 's2', 's3']

process QC {
    container 'python@sha256:5e2bdb1b0d542d5039cffb8181c65930093949194a65c365dfb1b0fe13a4fa5b'
    publishDir 'qc', mode: 'copy'

    input:
    tuple val(sample), path(fq)

    output:
    tuple val(sample), path("${sample}.stats.txt")

    script:
    """
    export LC_ALL=C
    reads=\$(grep -c '^@' ${fq})
    bases=\$(awk 'NR%4==2 {n+=length(\$0)} END {print n+0}' ${fq})
    gc=\$(awk 'NR%4==2 {g+=gsub(/[GCgc]/,"")} END {print g+0}' ${fq})
    echo "sample: ${sample}" > ${sample}.stats.txt
    echo "reads: \$reads" >> ${sample}.stats.txt
    echo "bases: \$bases" >> ${sample}.stats.txt
    echo "gc: \$gc" >> ${sample}.stats.txt
    """
}

workflow {
    Channel.fromList(params.samples.collect { s -> [s, file("data/${s}.fastq")] }) | QC
}
