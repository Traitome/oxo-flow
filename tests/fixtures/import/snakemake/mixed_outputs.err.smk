# EXPECT-ERROR: mixed wildcard/constant
# A wildcard fan-out plus a constant output: every per-sample instance would
# write reports/summary.txt.
rule mixed:
    input:
        "raw/{sample}.fastq"
    output:
        "out/{sample}.bam",
        "reports/summary.txt"
    shell:
        "align {input} > {output[0]}; echo ok > {output[1]}"
