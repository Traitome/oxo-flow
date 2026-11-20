# EXPECT-ERROR: unpack
# Derived fixture — upstream: snakemake-workflows/rna-seq-star-deseq2 workflow/rules/align.smk
# (https://github.com/snakemake-workflows/rna-seq-star-deseq2/blob/master/workflow/rules/align.smk,
# retrieved 2026-11-20). The `star_align` rule binds its reads with `unpack(get_fq)` — a
# helper function returning a list of paths computed from the samples table at parse time —
# so the input set is not statically resolvable. Process name, the unpack input, the named
# outputs and threads are upstream (pinned wrapper/params dropped for a smaller fixture).
rule star_align:
    input:
        unpack(get_fq),
        idx="resources/star_genome",
        gtf="resources/genome.gtf",
    output:
        aln="results/star/{sample}-{unit}/Aligned.sortedByCoord.out.bam",
        reads_per_gene="results/star/{sample}-{unit}/ReadsPerGene.out.tab",
    threads: 24
    wrapper:
        "v7.2.0/bio/star/align"
