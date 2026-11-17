# EXPECT-ERROR: fan-in
# The same fan-in shape through rules.<name>.output.
rule quantify:
    input:
        "raw/{sample}.fastq"
    output:
        "quant/{sample}.sf"
    shell:
        "salmon -r {input} -o quant/{sample}"

rule report:
    input:
        rules.quantify.output
    output:
        "results/report.txt"
    shell:
        "cat {input} > {output}"
