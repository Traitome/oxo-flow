# Venus Pipeline Extraction Design

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Extract Venus from oxo-flow workspace into independent repository as a clinical-grade tumor variant detection pipeline.

**Architecture:** Hybrid approach - Rust CLI for config validation/workflow generation + workflow files/scripts for pipeline execution.

**Tech Stack:** Rust 2024, oxo-flow-core, conda environments, BWA-MEM2, GATK 4, VEP

---

## 1. Repository Structure

```
oxo-flow-venus/                    # Independent git repository
├── Cargo.toml                      # Rust library + CLI binary
├── src/
│   ├── lib.rs                      # Config types, validation, generator
│   └── main.rs                     # CLI: generate, validate, list-steps
├── venus.oxoflow                   # Main workflow (generated template)
├── rules/                          # Modular sub-workflows
│   ├── qc.oxoflow                  # fastp, FastQC
│   ├── alignment.oxoflow           # BWA-MEM2, samtools
│   ├── preprocessing.oxoflow       # MarkDuplicates, BQSR
│   ├── snv_callers.oxoflow         # Mutect2, Strelka2
│   ├── snv_filtering.oxoflow       # FilterMutectCalls, VAF filters
│   ├── annotation.oxoflow          # VEP annotation
│   └── report.oxoflow              # Clinical report generation
├── scripts/                        # Processing scripts
│   ├── filter_vcf.R                # VCF filtering utilities
│   ├── merge_callers.py            # Merge caller outputs
│   ├── clinical_report.Rmd         # RMarkdown report template
│   └── tmb_calculation.R           # TMB calculation
├── envs/                           # Conda environments
│   ├── venus-qc.yaml
│   ├── venus-alignment.yaml
│   ├── venus-gatk.yaml
│   ├── venus-vep.yaml
│   └── venus-report.yaml
├── config.example.toml             # Example configuration
├── samplesheet.example.csv        # Example samplesheet
├── README.md                       # Documentation
├── CHANGELOG.md                    # Version history
├── LICENSE                         # MIT (dual with oxo-flow)
└── .github/
    └── workflows/
        ├── ci.yml                  # Build and test
        ├── lint.yml                # Lint workflow files
        └── release.yml             # Release automation
```

---

## 2. Pipeline Steps

### Core Pipeline (SNV/Indel Focus)

| Step | Tool | Input | Output | Notes |
|------|------|-------|--------|-------|
| fastqc | FastQC | FASTQ | HTML report | Quality metrics |
| fastp | fastp | FASTQ | trimmed FASTQ | Adapter trimming, QC |
| bwa_mem2 | BWA-MEM2 | FASTQ | BAM | Alignment |
| samtools_sort | samtools | BAM | sorted BAM | Coordinate sort |
| samtools_index | samtools | BAM | BAI | Index |
| mark_duplicates | GATK | BAM | dedup BAM | PCR duplicate marking |
| bqsr | GATK | dedup BAM | recalibrated BAM | Base quality recalibration |
| mutect2 | GATK | BAM | VCF | Somatic calling |
| filter_mutect | GATK | VCF | filtered VCF | Germline filter |
| strelka2 | Strelka2 | BAM | VCF | Alternative caller (optional) |
| merge_callers | Python | VCFs | merged VCF | Consensus calling |
| vep | VEP | VCF | annotated VCF | Functional annotation |
| clinical_report | R | annotated VCF | PDF/HTML | Report generation |

### Mode-Specific Variations

**experiment-only (tumor-only):**
- Mutect2 tumor-only mode
- PON filtering required
- Germline resource for filtering

**control-only (germline):**
- HaplotypeCaller instead of Mutect2
- Standard germline variant calling

**experiment-control (paired):**
- Mutect2 paired mode with matched normal
- Optional Strelka2 for consensus

### Sequencing Type Adjustments

| Type | Parameters |
|------|------------|
| WGS | Full genome intervals, lower depth thresholds |
| WES | Target BED required, exome-specific filters |
| Panel | Panel BED required, higher sensitivity settings |

---

## 3. Configuration Format

### venus.toml (Main Configuration)

```toml
# Venus Pipeline Configuration
# Generate workflow: venus generate config.toml -o venus.oxoflow

[venus]
mode = "experiment-control"    # experiment-only, control-only, experiment-control
seq_type = "WES"               # WGS, WES, Panel
genome_build = "GRCh38"         # GRCh37, GRCh38
output_dir = "results"          # Output directory

# Reference files (use reference_dir for convenience)
reference_dir = "/data/GRCh38"
reference_fasta = "/data/GRCh38/genome.fa"     # Or auto-derived
target_bed = "/data/GRCh38/exome.bed"           # Required for WES/Panel
dbsnp = "/data/GRCh38/dbsnp.vcf.gz"
known_indels = "/data/GRCh38/Mills.indels.vcf.gz"
vep_cache = "/data/vep_cache"

# Optional resources
pon = ""                    # Panel of normals VCF
gnomad = ""                 # Population frequencies
germline_resource = ""      # Germline variants for filtering

[defaults]
threads = 8
memory = "16G"
env_group = "default"       # Reference to env_groups

# Environment definitions
[env_groups.default]
conda = "envs/venus-gatk.yaml"

[env_groups.qc]
conda = "envs/venus-qc.yaml"

[env_groups.alignment]
conda = "envs/venus-alignment.yaml"

# Sample definitions
[[samples]]
name = "TUMOR_001"
type = "tumor"
r1 = "data/TUMOR_001_R1.fastq.gz"
r2 = "data/TUMOR_001_R2.fastq.gz"
metadata = { patient_id = "P001", tissue = "lung" }

[[samples]]
name = "NORMAL_001"
type = "normal"
pair_id = "TUMOR_001"        # Links to tumor
r1 = "data/NORMAL_001_R1.fastq.gz"
r2 = "data/NORMAL_001_R2.fastq.gz"
```

### Configuration Schema

**Required fields:**
- `venus.mode`: Analysis mode
- `venus.seq_type`: Sequencing type
- `venus.genome_build`: Reference genome
- `venus.reference_fasta`: Reference FASTA path
- `samples[]`: At least one sample

**Conditionally required:**
- `venus.target_bed`: Required for WES/Panel
- `samples[].pair_id`: Required for experiment-control mode

**Optional:**
- `venus.output_dir`: Output directory (default: "results")
- `venus.reference_dir`: Base directory for auto-derivation
- `defaults.*`: Default resource settings
- `env_groups.*`: Named environment groups

---

## 4. CLI Commands

### venus generate

```bash
venus generate config.toml -o venus.oxoflow
```

- Validates configuration
- Generates complete `.oxoflow` workflow file
- Expands wildcards based on samples
- Includes all necessary rules

### venus validate

```bash
venus validate config.toml
```

- Validates configuration syntax
- Checks required fields
- Verifies file paths exist (optional with --skip-file-check)
- Reports sample counts and mode

### venus list-steps

```bash
venus list-steps
```

- Lists all available pipeline steps
- Shows step dependencies
- Displays resource requirements

---

## 5. Workflow File Structure

### venus.oxoflow (Generated Template)

```toml
# Venus Pipeline - Generated from config.toml
# Run: oxo-flow run venus.oxoflow -j 8

[workflow]
name = "venus"
version = "1.0.0"
description = "Clinical-grade tumor variant detection"
author = "Venus Pipeline"
format_version = "1.0"
genome_build = "GRCh38"

[config]
reference_fasta = "/data/GRCh38/genome.fa"
target_bed = "/data/GRCh38/exome.bed"
# ... (copied from config)

[defaults]
threads = 8
memory = "16G"

[[include]]
path = "rules/qc.oxoflow"

[[include]]
path = "rules/alignment.oxoflow"

[[include]]
path = "rules/preprocessing.oxoflow"

[[include]]
path = "rules/snv_callers.oxoflow"

[[include]]
path = "rules/snv_filtering.oxoflow"

[[include]]
path = "rules/annotation.oxoflow"

[[include]]
path = "rules/report.oxoflow"
```

### Sample Expansion (in generated workflow)

Rules use wildcards that are expanded based on samples:

```toml
[[rules]]
name = "fastp_{sample}"
input = ["{r1}", "{r2}"]
output = ["results/{sample}_R1.trimmed.fastq.gz", "results/{sample}_R2.trimmed.fastq.gz"]
shell = """
fastp -i {input[0]} -I {input[1]} \
      -o {output[0]} -O {output[1]} \
      -j results/{sample}_fastp.json \
      -h results/{sample}_fastp.html
"""
env_group = "qc"
```

---

## 6. Environment Definitions

### envs/venus-gatk.yaml

```yaml
name: venus-gatk
channels:
  - bioconda
  - conda-forge
dependencies:
  - gatk4=4.4
  - samtools=1.19
  - bcftools=1.19
  - python>=3.9
```

### envs/venus-qc.yaml

```yaml
name: venus-qc
channels:
  - bioconda
  - conda-forge
dependencies:
  - fastp=0.23
  - fastqc=0.12
  - multiqc=1.18
```

### envs/venus-vep.yaml

```yaml
name: venus-vep
channels:
  - bioconda
  - conda-forge
dependencies:
  - ensembl-vep=110
  - bcftools=1.19
```

---

## 7. Error Codes

| Code | Category | Message |
|------|----------|---------|
| V001 | Config | Missing required field |
| V002 | Config | Invalid mode value |
| V003 | Config | Invalid seq_type value |
| V004 | Config | Missing target_bed for WES/Panel |
| V005 | Config | Sample missing required field |
| V006 | Config | Pair_id reference not found |
| V007 | Config | File path does not exist |
| V008 | Generation | Failed to generate workflow |
| V009 | Validation | Inconsistent mode/samples |

---

## 8. Testing Strategy

### Unit Tests
- Configuration parsing
- Validation logic
- Workflow generation

### Integration Tests
- `venus generate` produces valid `.oxoflow`
- Generated workflow passes `oxo-flow validate`
- `oxo-flow dry-run` succeeds

### End-to-End Tests (mini test data)
- Small test dataset (chr21 subset)
- Full pipeline execution
- Output validation

---

## 9. CI/CD Pipeline

### .github/workflows/ci.yml

```yaml
name: CI
on: [push, pull_request]
jobs:
  build:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4
      - uses: dtolnay/rust-toolchain@stable
      - run: cargo build --release
      - run: cargo test

  lint:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4
      - uses: dtolnay/rust-toolchain@stable
      - run: cargo clippy -- -D warnings

  validate-workflows:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4
      - name: Install oxo-flow
        run: cargo install oxo-flow --git https://github.com/Traitome/oxo-flow
      - name: Validate workflow files
        run: |
          for f in rules/*.oxoflow; do
            oxo-flow validate "$f" --as-include
          done
```

---

## 10. Documentation

### README.md Sections
1. Overview (Venus, 启明星)
2. Features
3. Installation
4. Quick Start
5. Configuration
6. Pipeline Steps
7. Output
8. Advanced Usage
9. Troubleshooting
10. Contributing
11. License

### CHANGELOG.md
- Follow Keep a Changelog format
- Track version changes

### Inline Documentation
- All public APIs documented with rustdoc
- Workflow files include comments
- Configuration examples annotated

---

## 11. Version Strategy

**Versioning:** SemVer (MAJOR.MINOR.PATCH)

**Version 1.0.0 (Initial Release):**
- Core SNV/Indel pipeline
- Three analysis modes
- WGS/WES/Panel support
- VEP annotation
- Clinical report generation

**Future versions:**
- 1.1.0: Optional CNV module
- 1.2.0: Additional callers (VarDict, Lofreq)
- 2.0.0: Breaking changes if needed

---

## 12. Migration from oxo-flow Workspace

**Extraction steps:**
1. Create new repository `oxo-flow-venus`
2. Copy `crates/venus/src/` to new repo
3. Update Cargo.toml for standalone
4. Add workflow files, scripts, envs
5. Setup CI/CD
6. Update documentation

**Removal from oxo-flow:**
1. Remove `crates/venus/` from workspace
2. Update workspace Cargo.toml
3. Update dependencies

---

## Summary

This design extracts Venus into an independent repository with:
- Focused scope (tumor SNV/Indel pipeline)
- Hybrid Rust CLI + workflow files architecture
- Clean configuration format
- Modular rules structure
- Comprehensive documentation and CI/CD
