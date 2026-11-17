// nf-core-style module shape: ext.args/ext.prefix preludes, the standard
// task.ext.when gate, container+conda fallbacks, and a versions block.
process FASTQC {
    tag "$meta.id"
    label 'process_single'

    conda "bioconda::fastqc=0.12.1"
    container "biocontainers/fastqc:0.12.1--hdfd78af_0"

    cpus 2
    memory '4 GB'
    time '4h'

    input:
    tuple val(meta), path(reads)

    output:
    tuple val(meta), path("${meta.id}_fastqc.html"), emit: html
    tuple val(meta), path("${meta.id}_fastqc.zip"), emit: zip
    path "versions.yml", emit: versions

    when:
    task.ext.when == null || task.ext.when

    script:
    def args = task.ext.args ?: ''
    def prefix = task.ext.prefix ?: "${meta.id}"
    """
    fastqc $args --threads $task.cpus $reads

    cat <<-END_VERSIONS > versions.yml
    "${task.process}":
        fastqc: $( fastqc --version )
    END_VERSIONS
    """
}

workflow {
    FASTQC(Channel.fromPath("data/*.fastq.gz"))
}
