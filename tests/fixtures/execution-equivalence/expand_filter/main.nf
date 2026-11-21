// Execution-equivalence fixture: expand_filter (Nextflow DSL2 arm).
// Expand over samples -> per-sample score -> fan-in threshold filter.
// Same pinned container digest, same relative output paths.
nextflow.enable.dsl = 2

params.samples = ['s1', 's2', 's3']

process SCORE {
    container 'python@sha256:5e2bdb1b0d542d5039cffb8181c65930093949194a65c365dfb1b0fe13a4fa5b'
    publishDir 'quant', mode: 'copy'

    input:
    tuple val(sample), path(fq)

    output:
    tuple val(sample), path("${sample}.sf")

    script:
    """
    export LC_ALL=C
    reads=\$(grep -c '^@' ${fq})
    bases=\$(awk 'NR%4==2 {n+=length(\$0)} END {print n+0}' ${fq})
    gc=\$(awk 'NR%4==2 {g+=gsub(/[GCgc]/,"")} END {print g+0}' ${fq})
    score=\$((gc * 10 / bases + reads))
    echo "sample: ${sample}" > ${sample}.sf
    echo "score: \$score" >> ${sample}.sf
    """
}

process FILTER {
    container 'python@sha256:5e2bdb1b0d542d5039cffb8181c65930093949194a65c365dfb1b0fe13a4fa5b'
    publishDir 'results', mode: 'copy'

    input:
    path sfs

    output:
    path 'kept.tsv'

    script:
    def files = sfs.collect { it.toString() }.sort()
    """
    export LC_ALL=C
    awk -F': ' '/^sample:/ {s=\$2} /^score:/ {if (\$2+0 >= 7) print s}' ${files.join(' ')} | sort > kept.tsv
    """
}

workflow {
    Channel.fromList(params.samples.collect { s -> [s, file("data/${s}.fastq")] }) \
        | SCORE \
        | map { s, f -> f } \
        | collect \
        | FILTER
}
