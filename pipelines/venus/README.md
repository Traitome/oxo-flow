# Venus Pipeline — Clinical-grade Tumor Variant Detection

Venus (启明星 — Morning Star) is a clinical-grade tumor variant detection, annotation, and reporting pipeline built on the oxo-flow engine. Named Venus to symbolize hope and light for cancer patients.

## Overview

Venus provides end-to-end tumor mutation analysis covering:

- **FASTQ Quality Control** — fastp for adapter trimming and quality filtering
- **Read Alignment** — BWA-MEM2 for fast, accurate short-read alignment
- **Duplicate Marking** — GATK MarkDuplicates for PCR duplicate removal
- **Base Quality Recalibration** — GATK BQSR for systematic error correction
- **Variant Calling** — GATK HaplotypeCaller (germline), Mutect2 (somatic), Strelka2 (paired somatic)
- **Variant Filtering** — FilterMutectCalls for somatic call quality control
- **Variant Annotation** — Ensembl VEP with ClinVar, COSMIC, gnomAD
- **Clinical Reporting** — Structured HTML/JSON reports for clinical review

## Supported Analysis Modes

| Mode | Description | Callers |
|------|-------------|---------|
| **Tumor-only** | Somatic analysis without matched normal | Mutect2 |
| **Normal-only** | Germline variant calling | HaplotypeCaller |
| **Tumor-Normal** | Paired somatic analysis with matched normal | Mutect2, Strelka2, HaplotypeCaller |

## Quick Start

```bash
# 1. Prepare samples.csv
cat > samples.csv << EOF
sample,r1_fastq,r2_fastq,is_tumor
TUMOR_01,raw/TUMOR_01_R1.fq.gz,raw/TUMOR_01_R2.fq.gz,true
NORMAL_01,raw/NORMAL_01_R1.fq.gz,raw/NORMAL_01_R2.fq.gz,false
EOF

# 2. Validate the pipeline
oxo-flow validate venus.oxoflow

# 3. Dry-run to preview
oxo-flow dry-run venus.oxoflow

# 4. Execute
oxo-flow run venus.oxoflow -j 8

# 5. Generate report
oxo-flow report venus.oxoflow -f html -o venus_report.html
```

## Reference Data Requirements

- **Reference genome**: GRCh38 FASTA with BWA-MEM2 index
- **Known sites**: dbSNP VCF for BQSR
- **VEP cache**: Ensembl VEP offline cache for annotation
- **Target BED** (WES/Panel only): Capture region definitions

## Pipeline DAG

```
fastp → bwa_mem2 → mark_duplicates → bqsr → mutect2 → filter → annotate → report
                                           → haplotype_caller → annotate → report
                                           → strelka2 (paired mode only)
```

## Sequencing Types

- **WGS**: Whole genome sequencing (no BED file required)
- **WES**: Whole exome sequencing (target BED required)
- **Panel**: Targeted panel sequencing (target BED required)

## Configuration

Edit `venus.oxoflow` to customize:

```toml
[config]
reference_fasta = "/path/to/GRCh38/genome.fa"
known_sites = "/path/to/dbsnp.vcf.gz"
genome_build = "GRCh38"
```

## Output Structure

```
venus_output/
├── trimmed/          # Quality-trimmed FASTQ files
├── qc/               # fastp QC reports (JSON)
├── aligned/          # Sorted BAM files
├── dedup/            # Deduplicated BAM files + metrics
├── recal/            # Recalibrated BAM files
├── variants/         # VCF files (raw + filtered)
├── annotated/        # Annotated VCF files
├── reports/          # Clinical HTML/JSON reports
└── logs/             # Per-step execution logs
```

## Clinical Report Sections

1. **Patient/Sample Information** — Sample metadata and sequencing QC
2. **Quality Metrics** — Read counts, mapping rate, coverage, duplicate rate
3. **Variant Summary** — Classified variants with ACMG interpretation
4. **Actionable Mutations** — Clinically significant findings
5. **Provenance** — Software versions, timestamps, pipeline parameters
