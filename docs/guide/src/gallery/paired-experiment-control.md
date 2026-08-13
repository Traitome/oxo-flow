# Paired Experiment-Control (Single Pair)

Somatic variant calling for one tumor/control pair with hardcoded sample names — the readable version of the [[pairs]]-driven pattern shown next.

## What It Demonstrates

- Both tumor and control flow through identical QC → align → dedup → BQSR stages in parallel branches
- Mutect2 `-normal CTRL_01` resolves via the read-group SM tag set at alignment
- BQSR table and application run in one rule (intermediate table kept inside the shell)

## Workflow Definition

```toml
# examples/gallery/14_paired_experiment_control.oxoflow
# 14 — Paired Experiment-Control (Single Pair)
# Somatic variant calling for one tumor/control pair, with hardcoded
# sample names for readability: fastp → BWA-MEM2 → MarkDuplicates →
# BQSR → Mutect2 → FilterMutectCalls → VEP → clinical report.

[workflow]
name = "paired-experiment-control"
version = "1.0.0"
description = "Paired experiment-control variant calling for general diagnostics"
author = "oxo-flow examples"

[config]
reference_fasta = "/data/references/GRCh38/genome.fa"
known_sites = "/data/references/GRCh38/dbsnp_146.hg38.vcf.gz"
genome_build = "GRCh38"

[defaults]
threads = 8
memory = "16G"

[report]
template = "report/clinical_template.html"
format = [
    "html",
    "json",
]
sections = [
    "sample_info",
    "qc",
    "variants",
    "clinical",
    "provenance",
    "disclaimer",
]

[[rules]]
name = "fastp_experiment"
input = [
    "raw/EXP_01_R1.fq.gz",
    "raw/EXP_01_R2.fq.gz",
]
output = [
    "trimmed/EXP_01_R1.fq.gz",
    "trimmed/EXP_01_R2.fq.gz",
    "qc/EXP_01_fastp.json",
]
shell = "fastp -i {input[0]} -I {input[1]} -o {output[0]} -O {output[1]} --json {output[2]} --thread {threads}"

[rules.resources]
threads = 8

[rules.environment]
conda = "envs/fastp.yaml"

[[rules]]
name = "fastp_control"
input = [
    "raw/CTRL_01_R1.fq.gz",
    "raw/CTRL_01_R2.fq.gz",
]
output = [
    "trimmed/CTRL_01_R1.fq.gz",
    "trimmed/CTRL_01_R2.fq.gz",
    "qc/CTRL_01_fastp.json",
]
shell = "fastp -i {input[0]} -I {input[1]} -o {output[0]} -O {output[1]} --json {output[2]} --thread {threads}"

[rules.resources]
threads = 8

[rules.environment]
conda = "envs/fastp.yaml"

[[rules]]
name = "bwa_mem2_experiment"
input = [
    "trimmed/EXP_01_R1.fq.gz",
    "trimmed/EXP_01_R2.fq.gz",
]
output = ["aligned/EXP_01.sorted.bam"]
shell = "bwa-mem2 mem -t {threads} -R '@RG\tID:EXP_01\tSM:EXP_01\tPL:ILLUMINA' {config.reference_fasta} {input[0]} {input[1]} | samtools sort -@ 4 -o {output[0]}"

[rules.resources]
threads = 16
memory = "32G"

[rules.environment]
conda = "envs/bwa_mem2.yaml"

[[rules]]
name = "bwa_mem2_control"
input = [
    "trimmed/CTRL_01_R1.fq.gz",
    "trimmed/CTRL_01_R2.fq.gz",
]
output = ["aligned/CTRL_01.sorted.bam"]
shell = "bwa-mem2 mem -t {threads} -R '@RG\tID:EXP_01\tSM:EXP_01\tPL:ILLUMINA' {config.reference_fasta} {input[0]} {input[1]} | samtools sort -@ 4 -o {output[0]}"

[rules.resources]
threads = 16
memory = "32G"

[rules.environment]
conda = "envs/bwa_mem2.yaml"

[[rules]]
name = "markdup_experiment"
input = ["aligned/EXP_01.sorted.bam"]
output = [
    "dedup/EXP_01.dedup.bam",
    "dedup/EXP_01.metrics.txt",
]
shell = "gatk MarkDuplicates -I {input[0]} -O {output[0]} -M {output[1]}"

[rules.resources]
threads = 4
memory = "16G"

[rules.environment]
conda = "envs/gatk.yaml"

[[rules]]
name = "markdup_control"
input = ["aligned/CTRL_01.sorted.bam"]
output = [
    "dedup/CTRL_01.dedup.bam",
    "dedup/CTRL_01.metrics.txt",
]
shell = "gatk MarkDuplicates -I {input[0]} -O {output[0]} -M {output[1]}"

[rules.resources]
threads = 4
memory = "16G"

[rules.environment]
conda = "envs/gatk.yaml"

[[rules]]
name = "bqsr_experiment"
input = ["dedup/EXP_01.dedup.bam"]
output = ["recal/EXP_01.recal.bam"]
shell = "gatk BaseRecalibrator -I {input[0]} -R {config.reference_fasta} --known-sites {config.known_sites} -O recal/EXP_01.recal.table && gatk ApplyBQSR -I {input[0]} -R /data/references/GRCh38/genome.fa --bqsr-recal-file recal/EXP_01.recal.table -O {output[0]}"

[rules.resources]
threads = 4
memory = "16G"

[rules.environment]
conda = "envs/gatk.yaml"

[[rules]]
name = "bqsr_control"
input = ["dedup/CTRL_01.dedup.bam"]
output = ["recal/CTRL_01.recal.bam"]
shell = "gatk BaseRecalibrator -I {input[0]} -R {config.reference_fasta} --known-sites {config.known_sites} -O recal/CTRL_01.recal.table && gatk ApplyBQSR -I {input[0]} -R /data/references/GRCh38/genome.fa --bqsr-recal-file recal/CTRL_01.recal.table -O {output[0]}"

[rules.resources]
threads = 4
memory = "16G"

[rules.environment]
conda = "envs/gatk.yaml"

[[rules]]
name = "mutect2"
input = [
    "recal/EXP_01.recal.bam",
    "recal/CTRL_01.recal.bam",
]
output = ["variants/EXP_01.mutect2.vcf.gz"]
shell = "gatk Mutect2 -I {input[0]} -I {input[1]} -normal CTRL_01 -R {config.reference_fasta} -O {output[0]}"

[rules.resources]
threads = 4
memory = "16G"

[rules.environment]
conda = "envs/gatk.yaml"

[[rules]]
name = "filter_mutect_calls"
input = ["variants/EXP_01.mutect2.vcf.gz"]
output = ["variants/EXP_01.mutect2.filtered.vcf.gz"]
shell = "gatk FilterMutectCalls -V {input[0]} -R {config.reference_fasta} -O {output[0]}"
description = "Filter raw Mutect2 calls"

[rules.resources]
threads = 2
memory = "8G"

[rules.environment]
conda = "envs/gatk.yaml"

[[rules]]
name = "haplotype_caller"
input = ["recal/CTRL_01.recal.bam"]
output = ["variants/CTRL_01.g.vcf.gz"]
shell = "gatk HaplotypeCaller -I {input[0]} -R {config.reference_fasta} -O {output[0]} -ERC GVCF"
description = "Variant calling for control sample"

[rules.resources]
threads = 4
memory = "16G"

[rules.environment]
conda = "envs/gatk.yaml"

[[rules]]
name = "annotate_variants"
input = ["variants/EXP_01.mutect2.filtered.vcf.gz"]
output = ["annotated/EXP_01.annotated.vcf.gz"]
shell = "vep --input_file {input[0]} --output_file {output[0]} --format vcf --vcf --offline --cache"

[rules.resources]
threads = 4
memory = "8G"

[rules.environment]
conda = "envs/vep.yaml"

[[rules]]
name = "clinical_report"
input = ["annotated/EXP_01.annotated.vcf.gz"]
output = ["reports/EXP_01_report.html"]
shell = "python scripts/generate_report.py --input {input[0]} --output {output[0]} --sample EXP_01"
description = "Generate variant report"

[rules.resources]
threads = 1
memory = "4G"

[rules.environment]
conda = "envs/report.yaml"
```

## Try It

```bash
# Inspect the expanded plan first — no data needed:
oxo-flow dry-run examples/gallery/14_paired_experiment_control.oxoflow

# Copy the environment specs next to the workflow, adapt [config]
# paths to your data, then run:
oxo-flow run examples/gallery/14_paired_experiment_control.oxoflow
```

!!! note "Input data and environments"

    Input paths under `/data/references/...` and `raw/` are placeholders —
    replace them with your own data. The referenced `envs/*.yaml` specs ship
    in [`examples/envs/`](https://github.com/Traitome/oxo-flow/tree/main/examples/envs).

