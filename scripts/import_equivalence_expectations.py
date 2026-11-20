#!/usr/bin/env python3
"""Independent declared-artifact truth for the import-equivalence corpus.

Every expected fact below was derived by READING THE SOURCE fixture
(tests/fixtures/import/<fmt>/<name>) by hand -- NOT by copying the importer's
output. `cited` records the source line ranges each fact came from.

Memory strings are written in the canonical form the engine normalizes to
(see crates/oxo-flow-core/src/transpile/mod.rs::normalize_memory and
crates/oxo-flow-core/src/scheduler.rs::parse_memory_mb):
  Nextflow '8 GB'   -> '8G'      Snakemake mem_mb=8000 -> '8000M'
Both denote the same byte quantity; the unit spelling is a normalized form,
not a semantic change.
"""

# status: "supported" (importer is expected to accept) or "rejected" (hard error).
EXPECTED = {
    "nextflow/rnaseq.ok.nf": {
        "status": "supported",
        "cited": "process TRIM L2-13, ALIGN L15-29, COUNT L31-42; wiring L58-60",
        "workflow_name_advisory": "trim_pipeline",
        "rules": ["trim", "align", "count"],
        "edges": [["trim", "align"], ["align", "count"]],
        "rule_specs": {
            "trim": {
                "threads": 4, "memory": "8G", "time_limit": "2h",
                "environment": {"conda": "bioconda::trimmomatic=0.39"},
                "input": ["data/{sample}.fastq.gz"],
                "output": ["{sample}.trimmed.fastq.gz"],
                "shell_verb": "trimmomatic",
                "shell_tokens": ["threads", "input", "sample"],
            },
            "align": {
                "threads": 12, "memory": "32G", "time_limit": "6h",
                "environment": {"conda": "bioconda::star=2.7.10b"},
                "input": ["{sample}.trimmed.fastq.gz"],
                "output": ["{sample}.bam"],
                "shell_verb": "STAR",
                "shell_tokens": ["threads", "input", "sample"],
            },
            "count": {
                "threads": 2, "memory": None, "time_limit": None,
                "environment": {"conda": "bioconda::subread=2.0.6"},
                "input": ["{sample}.bam"],
                "output": ["{sample}.counts"],
                "shell_verb": "featureCounts",
                "shell_tokens": ["threads", "input", "sample"],
            },
        },
    },
    "nextflow/nfcore_module.ok.nf": {
        "status": "supported",
        "cited": "process FASTQC L3-36; workflow L38-40; outputs L18-20",
        "workflow_name_advisory": "fastqc_pipeline",
        "rules": ["fastqc"],
        "edges": [],
        "rule_specs": {
            "fastqc": {
                "threads": 2, "memory": "4G", "time_limit": "4h",
                # container wins over the conda fallback (Nextflow runs the container)
                "environment": {"docker": "biocontainers/fastqc:0.12.1--hdfd78af_0"},
                "input": ["data/{sample}.fastq.gz"],
                # source declares 3 outputs (html, zip, versions.yml); the
                # versions.yml output is a declared artifact -- see report.
                "output": ["{sample}_fastqc.html", "{sample}_fastqc.zip"],
                "shell_verb": "fastqc",
                "shell_tokens": ["config.ext_args", "threads", "input"],
            },
        },
    },
    "snakemake/expand.ok.smk": {
        "status": "supported",
        "cited": "rule quantify L3-18 (wildcard_constraints L4, localrules L5)",
        "workflow_name_advisory": "quantify_pipeline",
        "rules": ["quantify"],
        "edges": [],
        "wildcard_constraints": {"sample": "[A-Za-z0-9_]+"},
        "rule_specs": {
            "quantify": {
                "threads": 4, "memory": None, "time_limit": None,
                "environment": {"modules": ["salmon/1.10", "samtools/1.19"]},
                # source: input expand("raw/{sample}.fastq", sample=["A","B"])
                "input": ["raw/{sample}.fastq"],
                # source: output temp("quant/{sample}.sf")
                "output": ["quant/{sample}.sf"],
                "localrule": True,
                "temporary": True,
                "temp_output": ["quant/{sample}.sf"],
                "shell_verb": "salmon",
                "shell_tokens": ["input", "sample"],
            },
        },
    },
    "snakemake/rnaseq.ok.smk": {
        "status": "supported",
        "cited": "rule trim L6-21, align L23-34, count L36-48; rule all L50-52",
        "workflow_name_advisory": "trim_pipeline",
        "rules": ["trim", "align", "count"],
        "edges": [["trim", "align"], ["align", "count"]],
        "rule_specs": {
            "trim": {
                "threads": 4, "memory": "8000M", "time_limit": "2h",
                "environment": {"conda": "envs/trimmomatic.yaml"},
                "input": ["data/{sample}_1.fastq.gz", "data/{sample}_2.fastq.gz"],
                "output": ["trimmed/{sample}_1.fastq.gz", "trimmed/{sample}_2.fastq.gz"],
                "shell_verb": "trimmomatic",
                "shell_tokens": ["threads", "input", "output", "log"],
            },
            "align": {
                "threads": 8, "memory": "32000M", "time_limit": None,
                "environment": {"conda": "envs/star.yaml"},
                "input": ["trimmed/{sample}_1.fastq.gz", "trimmed/{sample}_2.fastq.gz"],
                "output": ["aligned/{sample}.bam"],
                "shell_verb": "STAR",
                "shell_tokens": ["threads", "input"],
            },
            "count": {
                "threads": None, "memory": "4000M", "time_limit": None,
                "environment": {"docker": "biocontainers/subread:2.0.6"},
                "input": ["aligned/{sample}.bam"],
                "output": ["counts/{sample}.txt"],
                "shell_verb": "featureCounts",
                "shell_tokens": ["output", "input"],
            },
        },
    },
    # Real-world addendum (issue #1273): derived from nf-core/modules master,
    # not from any repo fixture. See the audit doc 2026-11-20 addendum.
    "nextflow/samtools_sort.ok.nf": {
        "status": "supported",
        "cited": "nf-core/modules modules/nf-core/samtools/sort/main.nf (master, retrieved 2026-11-20): "
                 "process SAMTOOLS_SORT; input tuple L34; output L37; prelude+script L40-51; workflow wiring",
        "workflow_name_advisory": "samtools_sort_pipeline",
        "rules": ["samtools_sort"],
        "edges": [],
        "rule_specs": {
            "samtools_sort": {
                "threads": 2, "memory": "8G", "time_limit": "4h",
                # upstream declares both container and conda; Nextflow runs the
                # container, so only the container is emitted.
                "environment": {"docker": "community.wave.seqera.io/library/htslib_samtools:1.25--704fb80d75745bd4"},
                "input": ["data/{sample}.bam"],
                "output": ["{sample}.bam"],
                # HONEST FINDING (not tuned away): upstream's BARE (non-`def`)
                # `prefix = task.ext.prefix ?: "${meta.id}"` prelude is NOT
                # stripped by the importer (nextflow.rs only strips `def `
                # preludes), so the emitted shell BEGINS with that leaked line.
                "shell_verb": "prefix",
                "shell_tokens": ["config.ext_args", "threads", "input"],
            },
        },
    },
    "snakemake/rseqc_stat.ok.smk": {
        "status": "supported",
        "cited": "snakemake-workflows/rna-seq-star-deseq2 workflow/rules/qc.smk rule rseqc_stat "
                 "(upstream L56-67, retrieved 2026-11-20); copied verbatim",
        "workflow_name_advisory": "rseqc_stat_pipeline",
        "rules": ["rseqc_stat"],
        "edges": [],
        "rule_specs": {
            "rseqc_stat": {
                "threads": None, "memory": None, "time_limit": None,
                "environment": {"conda": "../envs/rseqc.yaml"},
                "input": ["results/star/{sample}-{unit}/Aligned.sortedByCoord.out.bam"],
                "output": ["results/qc/rseqc/{sample}-{unit}.stats.txt"],
                "shell_verb": "bam_stat.py",
                "shell_tokens": ["input", "output", "log"],
            },
        },
    },
}

# Every rejected fixture and the out-of-subset construct it trips on, as
# reported verbatim by the real CLI (captured 2026-11-19).
REJECTED = {
    "nextflow/bare_glob_output.err.nf": "bare glob output `*.bam` not anchored to an input variable",
    "nextflow/channel_operator.err.nf": "unsupported channel expression `.map { it }`",
    "nextflow/dynamic_container.err.nf": "dynamic `container \"${params.container}\"`",
    "nextflow/include.err.nf": "cross-file `include { FASTQC } from './modules/fastqc'`",
    "nextflow/mixed_outputs.err.nf": "mixed wildcard/constant outputs (fan-in collision)",
    "nextflow/standalone_module.err.nf": "module `path reads` never invoked in a `workflow { }`",
    "snakemake/directory.err.smk": "`directory(...)` artifact",
    "snakemake/dynamic_input.err.smk": "`lambda wildcards:` dynamic input/output",
    "snakemake/expand_fan_in.err.smk": "mixed wildcard/constant outputs (fan-in collision)",
    "snakemake/include.err.smk": "global `include:` directive",
    "snakemake/mixed_outputs.err.smk": "mixed wildcard/constant outputs (fan-in collision)",
    "snakemake/params.err.smk": "`params:` block",
    "snakemake/rules_output_fan_in.err.smk": "mixed wildcard/constant outputs (fan-in collision)",
    "snakemake/run_body.err.smk": "`run:` body",
    # Real-world addendum (issue #1273): out-of-subset constructs found in
    # published community workflows (nf-core/modules, nextflow-io/rnaseq-nf,
    # snakemake-workflows/rna-seq-star-deseq2), retrieved 2026-11-20.
    "nextflow/nfcore_dynamic_conda.err.nf": "dynamic conda spec referencing the nf-core per-module environment.yml",
    "nextflow/rnaseq_nf_interpolation.err.nf": "unresolved Nextflow interpolation for a bare-valued metadata field",
    "snakemake/unpack_input.err.smk": "dynamic input/output `unpack(get_fq)`",
    "snakemake/script_directive.err.smk": "Snakemake `script:` is not supported by the importer",
    "snakemake/multiext_output.err.smk": "cannot parse input/output declaration `multiext(...)`",
}
