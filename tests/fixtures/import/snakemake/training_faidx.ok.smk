# Derived fixture — upstream: troycomi/snakemake-training 2_containers/Snakefile
# (https://github.com/troycomi/snakemake-training/blob/1b2861ccc52bda56c0838cdd5b6f55c23fc16e2e/2_containers/Snakefile,
# retrieved 2026-11-27), rule `index_reference` at upstream lines 49-59. This is
# the real tutorial rule: input/output/shell are copied verbatim (same test-data
# filename, same samtools command). Wrapper changes, all listed:
#   - `rule all` is added as the concrete target: the upstream file's default
#     target is `download_data`, which fetches test data over the network; this
#     extraction keeps only the index rule and runs it on planted input.
#   - The container value is written on its own line (valid Snakemake; this is
#     the multi-line directive shape real workflows use, e.g. the NBIS
#     workshop-reproducible-research final workflow).
#   - The upstream `biocontainers/samtools:v1.3_cv3` image is replaced with the
#     pinned nf-core samtools image (samtools 1.25) because the upstream image
#     is 1.26 GB on a qualification host with ~15 GB free; the pinned image is
#     202 MB. Both arms (Snakemake and the imported oxo-flow rule) run this
#     same image, so the comparison remains same-tool/same-input. The shell is
#     unchanged and `samtools faidx` exists in both images.
rule all:
    input:
        'GENOME_PATH/GCF_000001215.4_Release_6_plus_ISO1_MT_genomic.fna.fai'

rule index_reference:
    input:
        'GENOME_PATH/GCF_000001215.4_Release_6_plus_ISO1_MT_genomic.fna'

    output:
        'GENOME_PATH/GCF_000001215.4_Release_6_plus_ISO1_MT_genomic.fna.fai'

    container:
        'docker://community.wave.seqera.io/library/htslib_samtools:1.25--704fb80d75745bd4'

    shell:
        'samtools faidx {input}'
