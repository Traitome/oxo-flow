# EXPECT-ERROR: lambda
rule dynamic:
    input:
        lambda wildcards: "data/%s.fastq" % wildcards.sample
    output:
        "out/{sample}.txt"
    shell:
        "cp {input} {output}"
