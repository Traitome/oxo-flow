# EXPECT-ERROR: include:
include: "rules/alignment.smk"

rule local:
    input:
        "data/{sample}.txt"
    output:
        "out/{sample}.txt"
    shell:
        "cp {input} {output}"
