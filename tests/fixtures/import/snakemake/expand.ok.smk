# Supported subset: fan-OUT expand (the rule output carries the same
# wildcard), temp() output, wildcard constraints, localrules, envmodules.
wildcard_constraints:
    sample="[A-Za-z0-9_]+"

localrules: quantify

rule quantify:
    input:
        expand("raw/{sample}.fastq", sample=["A", "B"])
    output:
        temp("quant/{sample}.sf")
    threads: 4
    envmodules:
        "salmon/1.10",
        "samtools/1.19"
    shell:
        "salmon quant -i index -l A -r {input} -o quant/{sample}"
