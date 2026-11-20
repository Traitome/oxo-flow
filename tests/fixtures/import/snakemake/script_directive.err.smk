# EXPECT-ERROR: is not supported by the importer
# Derived fixture — upstream: snakemake-workflows/rna-seq-star-deseq2 workflow/rules/qc.smk
# (https://github.com/snakemake-workflows/rna-seq-star-deseq2/blob/master/workflow/rules/qc.smk,
# retrieved 2026-11-20). The `rseqc_gtf2bed` rule (upstream lines 4-16) delegates its body to
# an external `script:` file (`../scripts/gtf2bed.py`), which changes the command in a way the
# deterministic subset cannot preserve. Rule name, I/O, log and script path are upstream.
rule rseqc_gtf2bed:
    input:
        "resources/genome.gtf",
    output:
        bed="results/qc/rseqc/annotation.bed",
        db=temp("results/qc/rseqc/annotation.db"),
    log:
        "logs/rseqc/gtf2bed.log",
    conda:
        "../envs/gffutils.yaml"
    script:
        "../scripts/gtf2bed.py"
