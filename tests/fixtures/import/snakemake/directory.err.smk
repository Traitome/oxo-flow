# EXPECT-ERROR: directory
rule multiqc:
    input:
        "qc/{sample}/fastqc.html"
    output:
        directory("multiqc")
    shell:
        "multiqc {input} -o multiqc"
