# EXPECT-ERROR: multiext
# Derived fixture — upstream: snakemake-workflows/rna-seq-star-deseq2 workflow/rules/ref.smk
# (https://github.com/snakemake-workflows/rna-seq-star-deseq2/blob/master/workflow/rules/ref.smk,
# retrieved 2026-11-20). The `bwa_index` rule declares its five index files with `multiext(...)`,
# a multi-extension output helper the importer does not model. Rule name, input, multiext output,
# log, cache and resources are upstream (the pinned `wrapper:` is dropped for a smaller fixture).
rule bwa_index:
    input:
        "resources/genome.fasta",
    output:
        multiext("resources/genome.fasta", ".amb", ".ann", ".bwt", ".pac", ".sa"),
    log:
        "logs/bwa_index.log",
    cache: True
    resources:
        mem_mb=369000,
    shell:
        "bwa index {input}"
