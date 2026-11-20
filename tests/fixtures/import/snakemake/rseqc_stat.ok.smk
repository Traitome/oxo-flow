# Derived fixture — upstream: snakemake-workflows/rna-seq-star-deseq2 workflow/rules/qc.smk
# (https://github.com/snakemake-workflows/rna-seq-star-deseq2/blob/master/workflow/rules/qc.smk,
# retrieved 2026-11-20). The `rseqc_stat` rule copied verbatim (upstream lines 56-67):
# input/output/log/priority/conda/shell are unchanged from upstream.
rule rseqc_stat:
    input:
        "results/star/{sample}-{unit}/Aligned.sortedByCoord.out.bam",
    output:
        "results/qc/rseqc/{sample}-{unit}.stats.txt",
    log:
        "logs/rseqc/rseqc_stat/{sample}-{unit}.log",
    priority: 1
    conda:
        "../envs/rseqc.yaml"
    shell:
        "bam_stat.py -i {input} > {output} 2> {log}"
