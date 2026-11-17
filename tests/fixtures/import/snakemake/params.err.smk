# EXPECT-ERROR: `params:` is not supported
rule trim:
    input:
        "raw/{sample}.fastq"
    params:
        adapter="AGATCGGAAGAGC"
    output:
        "trimmed/{sample}.fastq"
    shell:
        "cutadapt -a {params.adapter} -o {output} {input}"
