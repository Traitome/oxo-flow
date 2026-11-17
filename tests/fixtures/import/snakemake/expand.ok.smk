# Supported subset: expand() input patterns, temp() outputs, wildcard
# constraints, localrules, and envmodules.
wildcard_constraints:
    sample="[A-Za-z0-9_]+"

localrules: report

rule quantify:
    input:
        "raw/{sample}.fastq"
    output:
        temp("quant/{sample}.sf")
    threads: 4
    envmodules:
        "salmon/1.10",
        "samtools/1.19"
    shell:
        "salmon quant -i index -l A -r {input} -o quant/{sample}"

rule report:
    input:
        expand("quant/{sample}.sf", sample=["A", "B"])
    output:
        "results/report.txt"
    shell:
        "cat {input} > {output}"
