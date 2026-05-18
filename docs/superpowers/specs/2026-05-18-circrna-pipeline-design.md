# circRNA Detection Pipeline Design Specification

**Date**: 2026-05-18
**Author**: Claude (Anthropic)
**Status**: Draft for Review

---

## 1. Executive Summary

This document specifies the design for `oxo-flow-circrna`, a circRNA detection and analysis pipeline built on the oxo-flow engine. The pipeline detects circRNAs from paired-end RNA-seq FASTQ files using four complementary methods (CIRIquant, Circexplorer2, find_circ, circRNA_finder), aggregates results across methods, and produces annotated, reliable circRNA calls with comprehensive HTML reports.

---

## 2. Background and Motivation

### 2.1 Reference Projects

| Project | Description | Role |
|---------|-------------|------|
| [circrna-pipeline-main](https://github.com/OncoHarmony-Network/circrna-pipeline) | Original bash-based pipeline | Primary reference |
| [BLIT-circrna-pipeline-main](https://github.com/WangLabCSU/BLIT-circrna-pipeline) | BLIT R-package refactor | Secondary reference |

### 2.2 Why oxo-flow?

- **Rust-native performance**: Fast execution with zero interpreter overhead
- **Declarative TOML workflows**: Human-readable, composable configuration
- **Built-in environment management**: First-class conda/pixi support
- **Clinical-grade reproducibility**: Config checksums, execution provenance
- **Existing patterns**: Venus and clindet demonstrate proven pipeline architecture

---

## 3. Design Goals

### 3.1 Primary Goals

1. **Complete and Accurate**: Implement all four circRNA detection methods faithfully
2. **Reproducible**: Deterministic outputs given same inputs and configuration
3. **User-friendly**: Sensible defaults with clear configuration options
4. **Extensible**: Modular design allows adding new callers or analysis steps

### 3.2 Non-Goals

- Real-time circRNA detection
- Single-cell circRNA analysis (out of scope)
- circRNA functional prediction (downstream analysis)

---

## 4. Architecture

### 4.1 Directory Structure

```
oxo-flow-circrna/
├── circrna.oxoflow              # Main workflow orchestrator
├── config/
│   ├── ciriquant_hg38.yml       # CIRIquant config template
│   └── config.example.toml      # Example user configuration
├── rules/
│   ├── qc.oxoflow               # fastp + MultiQC module
│   ├── callers.oxoflow          # All 4 circRNA callers
│   ├── annotation.oxoflow       # Gene annotation from GTF
│   ├── aggregation.oxoflow      # Ensemble aggregation
│   └── report.oxoflow           # HTML report generation
├── envs/
│   ├── fastp.yaml
│   ├── multiqc.yaml
│   ├── ciriquant.yaml
│   ├── circexplorer2.yaml
│   ├── findcirc.yaml
│   ├── circrna_finder.yaml
│   ├── annotation.yaml
│   └── report.yaml
├── scripts/
│   ├── common.py                # circRNA overlap detection
│   ├── annotate_circrna.R       # Gene annotation script
│   ├── aggregate_beds.R         # Ensemble aggregation
│   ├── aggregate_dataset.R      # Multi-sample aggregation
│   └── generate_report.R        # HTML report generation
├── templates/
│   └── report_template.html     # Report HTML template
├── test_data/                   # Small test dataset
├── samplesheet.example.csv
├── README.md
├── LICENSE
└── .gitignore
```

### 4.2 Execution Flow

```
┌─────────────────────────────────────────────────────────────┐
│                        FASTQ Input                          │
│              {sample}_1.fastq.gz, {sample}_2.fastq.gz        │
└─────────────────────────┬───────────────────────────────────┘
                          │
                          ▼
                   ┌────────────┐
                   │   fastp    │  QC + Trimming
                   └─────┬──────┘
                         │
          ┌──────────────┼──────────────┐
          │              │              │
          ▼              ▼              ▼
    ┌──────────┐   ┌──────────┐   ┌──────────┐
    │ MultiQC  │   │ Caller 1 │   │ Caller 2 │   ...
    │(QC汇总)   │   │ CIRIquant│   │Circexplo-│
    └──────────┘   └────┬─────┘   │   rer2   │
                        │         └────┬────┘
                        │              │
                        └──────┬───────┘
                               │
                        ┌──────▼──────┐
                        │  Aggregate  │  Find common circRNAs
                        └──────┬──────┘
                               │
                        ┌──────▼──────┐
                        │  Annotate   │  Add gene names
                        └──────┬──────┘
                               │
                        ┌──────▼──────┐
                        │   Report    │  HTML + TSV outputs
                        └─────────────┘
```

### 4.3 Dependency Graph

```
fastp ─┬─► MultiQC
       │
       ├─► CIRIquant ──────┐
       ├─► Circexplorer2 ──┼─► Annotate ─► Aggregate ─► Report
       ├─► find_circ ──────┤
       └─► circRNA_finder ─┘
```

- 4 callers run **in parallel** after fastp
- MultiQC is **independent** of callers (aggregates fastp QC only)
- Aggregation requires all 4 caller outputs per sample

---

## 5. Module Specifications

### 5.1 QC Module (rules/qc.oxoflow)

#### 5.1.1 fastp Rule

```toml
[[rules]]
name = "fastp"
input = ["raw/{sample}_1.fastq.gz", "raw/{sample}_2.fastq.gz"]
output = [
    "trimmed/{sample}_1.fastq.gz",
    "trimmed/{sample}_2.fastq.gz",
    "qc/{sample}_fastp.json",
    "qc/{sample}_fastp.html"
]
threads = 8
memory = "8G"
shell = """
mkdir -p trimmed qc
fastp -i {input[0]} -I {input[1]} \
      -o {output[0]} -O {output[1]} \
      --json {output[2]} --html {output[3]} \
      --thread {threads} \
      --qualified_quality_phred 20 \
      --length_required 50
"""

[rules.environment]
conda = "envs/fastp.yaml"
```

#### 5.1.2 multiqc Rule

```toml
[[rules]]
name = "multiqc"
input = ["qc/"]  # Directory input
output = ["results/multiqc_report.html"]
depends_on = ["fastp"]
shell = """
mkdir -p results
multiqc qc/ -o results/ --force
"""

[rules.environment]
conda = "envs/multiqc.yaml"
```

### 5.2 Callers Module (rules/callers.oxoflow)

#### 5.2.1 CIRIquant Rule

**Input**: Trimmed FASTQ files
**Output**: `{sample}.CIRI.bed` (7 columns: chr, start, end, strand, gene_id, gene_name, counts)

```toml
[[rules]]
name = "ciriquant"
input = ["trimmed/{sample}_1.fastq.gz", "trimmed/{sample}_2.fastq.gz"]
output = ["results/{sample}.CIRI.bed"]
threads = 8
memory = "32G"
shell = """
mkdir -p results/{sample}.CIRI
cd results/{sample}.CIRI

CIRIquant -t {threads} \
    -1 {input[0]} -2 {input[1]} \
    --config {config.ciriquant_config} \
    --no-gene \
    -o . -p {sample} -v

# Convert GTF to BED format (7 columns)
grep -v "#" {sample}.gtf | awk '{print $14}' | cut -d '.' -f1 > {sample}.counts
grep -v "#" {sample}.gtf | awk -v OFS="\\t" '{gsub(/[";]/, "", $20); gsub(/[";]/, "", $22); print $1,$4-1,$5,$7,$20,$22}' > {sample}.tmp
paste {sample}.tmp {sample}.counts > ../{sample}.CIRI.bed

rm -f {sample}.tmp {sample}.counts
rm -rf align circ
"""

[rules.environment]
conda = "envs/ciriquant.yaml"
```

**CIRIquant Configuration** (`config/ciriquant_hg38.yml`):
```yaml
tools:
  bwa: bwa
  hisat2: hisat2
  stringtie: stringtie
  samtools: samtools

reference:
  fasta: /path/to/GRCh38.primary_assembly.genome.fa
  gtf: /path/to/gencode.v34.annotation.gtf
  bwa_index: /path/to/GRCh38.primary_assembly.genome.fa
  hisat_index: /path/to/GRCh38.primary_assembly.genome.fa
```

#### 5.2.2 Circexplorer2 Rule

**Input**: Trimmed FASTQ files
**Output**: `{sample}.circexplorer2.bed` (5 columns: chr, start, end, strand, counts)

```toml
[[rules]]
name = "circexplorer2"
input = ["trimmed/{sample}_1.fastq.gz", "trimmed/{sample}_2.fastq.gz"]
output = ["results/{sample}.circexplorer2.bed"]
threads = 8
memory = "16G"
shell = """
mkdir -p results/{sample}.circexplorer2
cd results/{sample}.circexplorer2

# Step 1: BWA alignment
bwa mem -t {threads} -T 19 {config.reference_fasta} {input[0]} {input[1]} \
    > {sample}_unmapped_bwa.sam 2> {sample}_bwa.log

# Step 2: Parse junction reads
CIRCexplorer2 parse -t BWA -b {sample}_circ2_result.txt {sample}_unmapped_bwa.sam

# Step 3: Annotate circRNAs
CIRCexplorer2 annotate -r {config.circexplorer2_ref} -g {config.reference_fasta} \
    -b {sample}_circ2_result.txt -o {sample}_circ2_result_ann.txt

# Extract BED (5 columns)
awk -v OFS="\\t" '{print $1,$2,$3,$6,$13}' {sample}_circ2_result_ann.txt > ../{sample}.circexplorer2.bed

rm -f *.sam
"""

[rules.environment]
conda = "envs/circexplorer2.yaml"
```

#### 5.2.3 find_circ Rule

**Input**: Trimmed FASTQ files
**Output**: `{sample}.find_circ.bed` (6 columns: chr, start, end, strand, counts, sample)

```toml
[[rules]]
name = "find_circ"
input = ["trimmed/{sample}_1.fastq.gz", "trimmed/{sample}_2.fastq.gz"]
output = ["results/{sample}.find_circ.bed"]
threads = 8
memory = "16G"
shell = """
mkdir -p results/{sample}.find_circ
cd results/{sample}.find_circ

# Step 1: Bowtie2 alignment
bowtie2 -x {config.bowtie2_index} \
    -1 {input[0]} -2 {input[1]} \
    --threads {threads} \
    --very-sensitive --score-min=C,-15,0 --reorder --mm \
    2> {sample}.bowtie2.log | samtools sort -@ {threads} -o {sample}.bam -
samtools index -@ {threads} {sample}.bam

# Step 2: Extract unmapped reads
samtools view -@ {threads} -hbf 4 {sample}.bam > {sample}.unmapped.bam
samtools index -@ {threads} {sample}.unmapped.bam

# Step 3: Split into anchors
unmapped2anchors.py {sample}.unmapped.bam > {sample}.unmapped.fastq

# Step 4: Re-align anchors and detect circRNAs
bowtie2 -q -U {sample}.unmapped.fastq -x {config.bowtie2_index} \
    --threads {threads} --reorder --mm --very-sensitive --score-min=C,-15,0 \
    2> {sample}.bowtie2.2nd.log | \
    find_circ.py -G {config.reference_fasta} -n {sample} \
    --stats {sample}.sites.log \
    --reads {sample}.spliced_reads.fa \
    > {sample}.splice_sites.bed

# Filter and format
grep CIRCULAR {sample}.splice_sites.bed | \
    grep -v chrM | \
    grep UNAMBIGUOUS_BP | grep ANCHOR_UNIQUE | \
    maxlength.py 100000 > {sample}.filtered.txt

awk -v OFS="\\t" '{print $1,$2,$3,$6,$5,$4}' {sample}.filtered.txt > ../{sample}.find_circ.bed

rm -f *.bam *.bai *.fastq *.fa
"""

[rules.environment]
conda = "envs/findcirc.yaml"
```

#### 5.2.4 circRNA_finder Rule

**Input**: Trimmed FASTQ files
**Output**: `{sample}.circRNA_finder.bed` (5 columns: chr, start, end, strand, counts)

```toml
[[rules]]
name = "circrna_finder"
input = ["trimmed/{sample}_1.fastq.gz", "trimmed/{sample}_2.fastq.gz"]
output = ["results/{sample}.circRNA_finder.bed"]
threads = 8
memory = "32G"
shell = """
mkdir -p results/{sample}.circRNA_finder

STAR --readFilesIn {input[0]} {input[1]} \
    --runThreadN {threads} \
    --genomeDir {config.star_index} \
    --chimSegmentMin 20 --chimScoreMin 1 --alignIntronMax 100000 \
    --chimOutType Junctions SeparateSAMold \
    --outFilterMismatchNmax 4 \
    --alignTranscriptsPerReadNmax 100000 \
    --outFilterMultimapNmax 2 \
    --outFileNamePrefix results/{sample}.circRNA_finder/{sample}. \
    --readFilesCommand zcat

postProcessStarAlignment.pl --starDir results/{sample}.circRNA_finder --outDir results/{sample}.circRNA_finder

# Extract BED
awk -v OFS="\\t" -F"\\t" '{print $1,$2,$3,$6,$5}' \
    results/{sample}.circRNA_finder/{sample}.filteredJunctions.bed > {output[0]}

rm -f results/{sample}.circRNA_finder/*.sam
"""

[rules.environment]
conda = "envs/circrna_finder.yaml"
```

### 5.3 Aggregation Module (rules/aggregation.oxoflow)

#### 5.3.1 Aggregation Logic

The aggregation follows the reference pipeline:

1. **Find common circRNAs**: Use `common.py` to find circRNAs detected by ≥2 methods (allowing 3bp position deviation)
2. **Filter by counts**: Keep only circRNAs where at least one method has counts ≥ 2
3. **Annotate genes**: Map circRNAs to genes using GTF
4. **Average counts**: For circRNAs detected by multiple methods, report mean counts

```toml
[[rules]]
name = "aggregate"
input = [
    "results/{sample}.CIRI.bed",
    "results/{sample}.circexplorer2.bed",
    "results/{sample}.find_circ.bed",
    "results/{sample}.circRNA_finder.bed"
]
output = ["results/{sample}.aggr.txt"]
depends_on = ["ciriquant", "circexplorer2", "find_circ", "circrna_finder"]
shell = """
Rscript scripts/aggregate_beds.R results results {sample}
"""

[rules.environment]
conda = "envs/annotation.yaml"
```

### 5.4 Report Module (rules/report.oxoflow)

```toml
[[rules]]
name = "report"
input = ["results/*.aggr.txt"]
output = ["results/circrna_report.html"]
depends_on = ["aggregate"]
shell = """
Rscript scripts/generate_report.R results results/circrna_report.html
"""

[rules.environment]
conda = "envs/report.yaml"
```

---

## 6. Output Specifications

### 6.1 Per-Sample BED Files

| File | Columns | Description |
|------|---------|-------------|
| `{sample}.CIRI.bed` | 7 | chr, start, end, strand, gene_id, gene_name, counts |
| `{sample}.circexplorer2.bed` | 5 | chr, start, end, strand, counts |
| `{sample}.find_circ.bed` | 6 | chr, start, end, strand, counts, sample |
| `{sample}.circRNA_finder.bed` | 5 | chr, start, end, strand, counts |

### 6.2 Aggregated Output

`{sample}.aggr.txt` - Final circRNA calls with columns:
- `chr`: Chromosome
- `start`: Start position (0-based)
- `end`: End position
- `strand`: Strand (+/-)
- `gene`: Gene symbol
- `tool`: Comma-separated list of detecting tools
- `count`: Average junction read counts
- `sample`: Sample identifier

### 6.3 HTML Report Sections

1. **Summary**: Sample count, total circRNAs, method comparison
2. **QC Metrics**: fastp statistics, read counts, quality scores
3. **Method Comparison**: Venn diagram of circRNA overlap across methods
4. **circRNA Distribution**: Genomic distribution, strand bias
5. **Top circRNAs**: Most abundant circRNAs per sample
6. **Provenance**: Software versions, parameters, timestamps

---

## 7. Configuration

### 7.1 Main Configuration (config.example.toml)

```toml
[config]
# Reference genome
reference_fasta = "/path/to/GRCh38.primary_assembly.genome.fa"
gene_annotation = "/path/to/gencode.v34.annotation.gtf"

# Index paths (auto-generated if missing)
bwa_index = "/path/to/GRCh38.primary_assembly.genome.fa"
hisat2_index = "/path/to/GRCh38.primary_assembly.genome.fa"
bowtie2_index = "/path/to/GRCh38.primary_assembly.bt2"
star_index = "/path/to/STAR_index"

# CIRIquant-specific config
ciriquant_config = "config/ciriquant_hg38.yml"

# Circexplorer2 reference
circexplorer2_ref = "/path/to/hg38_ref.txt"

[defaults]
threads = 8
memory = "16G"
```

### 7.2 Samplesheet Format

`samplesheet.csv`:
```csv
sample,r1_fastq,r2_fastq
SAMPLE_01,raw/SAMPLE_01_1.fastq.gz,raw/SAMPLE_01_2.fastq.gz
SAMPLE_02,raw/SAMPLE_02_1.fastq.gz,raw/SAMPLE_02_2.fastq.gz
```

---

## 8. Environment Specifications

### 8.1 fastp.yaml

```yaml
name: fastp
channels:
  - conda-forge
  - bioconda
dependencies:
  - fastp=0.23.4
```

### 8.2 ciriquant.yaml

```yaml
name: ciriquant
channels:
  - conda-forge
  - bioconda
dependencies:
  - python=2.7
  - bwa=0.7.17
  - hisat2=2.2.0
  - stringtie=2.1.1
  - samtools=1.10
  - r-base=3.6
  - r-optparse=1.6.6
  - r-statmod=1.4.35
  - bioconductor-edger=3.28.0
  - bioconductor-limma=3.42.0
  - pip
  - pip:
    - CIRIquant==1.1.2
```

### 8.3 circexplorer2.yaml

```yaml
name: circexplorer2
channels:
  - conda-forge
  - bioconda
dependencies:
  - python=3.7
  - bwa
  - pip
  - pip:
    - circexplorer2
```

### 8.4 findcirc.yaml

```yaml
name: findcirc
channels:
  - conda-forge
  - bioconda
dependencies:
  - find_circ=1.2
  - bowtie2
  - samtools
```

### 8.5 circrna_finder.yaml

```yaml
name: circrna_finder
channels:
  - conda-forge
  - bioconda
dependencies:
  - circrna_finder
  - star
  - samtools
```

### 8.6 annotation.yaml

```yaml
name: annotation
channels:
  - conda-forge
  - bioconda
dependencies:
  - r-base=4.2
  - r-data.table
```

### 8.7 report.yaml

```yaml
name: report
channels:
  - conda-forge
  - bioconda
dependencies:
  - r-base=4.2
  - r-rmarkdown
  - r-plotly
  - r-ggplot2
  - r-data.table
```

---

## 9. Testing Strategy

### 9.1 Unit Tests

- Each module can be tested independently
- Test common.py overlap detection algorithm
- Test BED format parsing

### 9.2 Integration Tests

- Small test dataset (chr21 subset)
- Verify all 4 callers produce expected output
- Verify aggregation produces correct overlap

### 9.3 Test Data

- Download from CIRIquant test data or generate synthetic
- Include at least 2 samples for multi-sample testing

---

## 10. oxo-flow Improvements (Feedback)

During implementation, the following oxo-flow improvements may be needed:

1. **Directory input support**: MultiQC requires directory input
2. **Config file validation**: Verify external config files exist
3. **Wildcard constraints**: Support regex constraints on wildcards
4. **Multi-output handling**: Handle different caller output formats

---

## 11. Implementation Phases

### Phase 1: Core Pipeline
- qc.oxoflow
- callers.oxoflow
- Basic testing

### Phase 2: Aggregation
- annotation.oxoflow
- aggregation.oxoflow
- common.py integration

### Phase 3: Reporting
- report.oxoflow
- HTML template

### Phase 4: Polish
- Documentation
- Test data
- CI/CD integration

---

## 12. References

1. CIRIquant: https://github.com/bioinfo-biols/CIRIquant
2. CIRCexplorer2: https://circexplorer2.readthedocs.io/
3. find_circ: https://github.com/marvin-jens/find_circ
4. circRNA_finder: https://github.com/orzechoj/circRNA_finder
5. Original pipeline: https://github.com/OncoHarmony-Network/circrna-pipeline

---

## Appendix A: common.py

```python
#!/usr/bin/env python3
# Find common circRNA entries across multiple BED files
# Allows position deviation and configurable overlap threshold

import argparse
import sys
from collections import defaultdict

def parse_bed_entry(line):
    chromosome, start, end = line.strip().split('\t')[:3]
    return (chromosome, int(start), int(end))

def find_common_entries(bed_entries, deviation=3, count_threshold=2):
    bed_entries_dict = defaultdict(int)
    common_entries = []

    for entry in bed_entries:
        if len(bed_entries_dict) == 0:
            bed_entries_dict[entry] += 1
        else:
            flag = []
            for other_entry, count in bed_entries_dict.items():
                if entry[0] == other_entry[0] and \
                    abs(entry[1] - other_entry[1]) <= deviation and \
                    abs(entry[2] - other_entry[2]) <= deviation:
                    bed_entries_dict[other_entry] += 1
                    flag.append(True)
                else:
                    flag.append(False)
            if not any(flag):
                bed_entries_dict[entry] += 1

    for entry, count in bed_entries_dict.items():
        if bed_entries_dict[entry] >= count_threshold:
            common_entries.append(entry)

    return common_entries

def main():
    parser = argparse.ArgumentParser(description='Find common entries in BED input.')
    parser.add_argument('bed_file', nargs='?',
                        help='Input BED file. If not provided, read from stdin.')
    parser.add_argument('-d', '--deviation', type=int, default=0,
                        help='Position deviation tolerance (default: 0)')
    parser.add_argument('-t', '--count-threshold', type=int, default=2,
                        help='Minimum number of methods (default: 2)')
    args = parser.parse_args()

    bed_entries = []
    if args.bed_file:
        with open(args.bed_file, 'r') as file:
            for line in file:
                entry = parse_bed_entry(line)
                bed_entries.append(entry)
    else:
        for line in sys.stdin:
            entry = parse_bed_entry(line)
            bed_entries.append(entry)

    common_entries = find_common_entries(bed_entries, args.deviation, args.count_threshold)

    for entry in common_entries:
        print('\t'.join(map(str, entry)))

if __name__ == '__main__':
    main()
```

---

**End of Specification**
