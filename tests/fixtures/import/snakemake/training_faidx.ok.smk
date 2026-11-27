# Derived fixture — upstream: troycomi/snakemake-training 2_containers/Snakefile
# (https://github.com/troycomi/snakemake-training/blob/1b2861ccc52bda56c0838cdd5b6f55c23fc16e2e/2_containers/Snakefile,
# retrieved 2026-11-27), rule `index_reference` at upstream lines 49-59. This is
# the real tutorial rule: input/output/shell are copied verbatim (same test-data
# filename, same samtools command, same image). Wrapper changes, all listed:
#   - `rule all` is added as the concrete target: the upstream file's default
#     target is `download_data`, which fetches test data over the network; this
#     extraction keeps only the index rule and runs it on planted input.
#   - The container value is written on its own line (valid Snakemake; this is
#     the multi-line directive shape real workflows use, e.g. the NBIS
#     workshop-reproducible-research final workflow). The quoted value is
#     byte-identical to upstream.
rule all:
    input:
        'GENOME_PATH/GCF_000001215.4_Release_6_plus_ISO1_MT_genomic.fna.fai'

rule index_reference:
    input:
        'GENOME_PATH/GCF_000001215.4_Release_6_plus_ISO1_MT_genomic.fna'

    output:
        'GENOME_PATH/GCF_000001215.4_Release_6_plus_ISO1_MT_genomic.fna.fai'

    container:
        'docker://biocontainers/samtools:v1.3_cv3'

    shell:
        'samtools faidx {input}'
