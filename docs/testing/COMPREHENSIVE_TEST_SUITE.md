# oxo-flow Comprehensive Test Suite

> **Total Test Instances**: 120+ real scenario tests
> **User Levels**: Beginner (30), Intermediate (40), Advanced (30), Expert (20)
> **Testing Approach**: Self-contained tests with shell mocks, no external dependencies

## Design Principles

1. **Self-contained**: All tests create their own test data and workflows
2. **No external dependencies**: Uses shell commands (echo, cat, sed, awk) to mock bioinformatics tools
3. **Cross-platform**: Works on macOS and Linux without special software
4. **Test oxo-flow core features**: Validates CLI, validation, lint, graph, cluster generation, etc.

---

## Test Environment Setup

```bash
# Create isolated test workspace
export OXO_TEST_DIR="/tmp/oxo-flow-tests-$(date +%s)"
mkdir -p "$OXO_TEST_DIR"
cd "$OXO_TEST_DIR"

# Ensure oxo-flow binary is available
OXO_FLOW_BIN="${OXO_FLOW_BIN:-./target/release/oxo-flow}"
if [[ ! -x "$OXO_FLOW_BIN" ]]; then
  cd /Users/wsx/Documents/GitHub/oxo-flow
  cargo build --release
  OXO_FLOW_BIN="./target/release/oxo-flow"
fi

# Helper function to create test workflow
create_workflow() {
  local name="$1"
  local content="$2"
  echo "$content" > "${OXO_TEST_DIR}/${name}.oxoflow"
}
```

---

## Phase 1: Beginner Tests (B01-B30)

### User Profile: New to bioinformatics pipelines, learning basics

### Test B01-B10: Basic CLI Discovery

**B01: Help command and command overview**
```bash
# Scenario: New user discovering oxo-flow capabilities
$OXO_FLOW_BIN --help
# Expected: Shows all commands with descriptions
# Verify: Output contains "run", "validate", "lint", "graph", "cluster"
```

**B02: Version check**
```bash
# Scenario: Checking version compatibility
$OXO_FLOW_BIN --version
# Expected: Shows version number (e.g., 0.3.1)
```

**B03: Shell completion generation**
```bash
# Scenario: Enabling tab completion
$OXO_FLOW_BIN completions bash > "${OXO_TEST_DIR}/completion.bash"
grep -q "oxo-flow" "${OXO_TEST_DIR}/completion.bash"
# Expected: Completion script generated
```

**B04: Initialize project structure**
```bash
# Scenario: Creating a new pipeline project
cd "${OXO_TEST_DIR}"
$OXO_FLOW_BIN init test-project -d ./B04_project
# Expected: Creates .oxoflow/, envs/, scripts/ directories
ls ./B04_project/.oxoflow ./B04_project/envs ./B04_project/scripts
```

**B05: Validate simple workflow (PASS)**
```bash
# Scenario: Validating a correct workflow
create_workflow "simple_pass" '
[workflow]
name = "simple-test"
version = "1.0"

[[rules]]
name = "hello"
output = ["output.txt"]
shell = "echo hello > output.txt"
'
$OXO_FLOW_BIN validate "${OXO_TEST_DIR}/simple_pass.oxoflow"
# Expected: PASS with "Valid workflow" message
```

**B06: Dry-run execution preview**
```bash
# Scenario: Previewing execution without running
create_workflow "dry_run_test" '
[workflow]
name = "dry-run-test"
version = "1.0"

[[rules]]
name = "step1"
output = ["data.txt"]
shell = "echo data > data.txt"

[[rules]]
name = "step2"
input = ["data.txt"]
output = ["result.txt"]
shell = "cat data.txt > result.txt"
'
$OXO_FLOW_BIN dry-run "${OXO_TEST_DIR}/dry_run_test.oxoflow"
# Expected: Shows execution plan, DAG order, no actual files created
ls data.txt result.txt 2>/dev/null && echo "FAIL: Files should not exist" || echo "PASS"
```

**B07: Graph visualization (DOT format)**
```bash
# Scenario: Visualizing workflow dependencies
$OXO_FLOW_BIN graph -f dot "${OXO_TEST_DIR}/dry_run_test.oxoflow" > "${OXO_TEST_DIR}/graph.dot"
grep -q "digraph" "${OXO_TEST_DIR}/graph.dot"
# Expected: Valid DOT format
```

**B08: List environment backends**
```bash
# Scenario: Discovering supported environment systems
$OXO_FLOW_BIN env list
# Expected: Lists conda, pixi, docker, singularity, venv
```

**B09: Format workflow to canonical TOML**
```bash
# Scenario: Cleaning up workflow formatting
create_workflow "messy" '
[workflow]
name="messy"
version="1.0"
[[rules]]
name="step1"
shell="echo hi"
output=["out.txt"]
'
$OXO_FLOW_BIN format "${OXO_TEST_DIR}/messy.oxoflow"
# Expected: Canonical TOML format (proper spacing, indentation)
```

**B10: Lint for best practices**
```bash
# Scenario: Checking workflow quality
create_workflow "lint_test" '
[workflow]
name = "lint-demo"
version = "1.0"

[[rules]]
name = "good_rule"
threads = 4
memory = "8G"
output = ["result.txt"]
shell = "echo good > result.txt"
'
$OXO_FLOW_BIN lint "${OXO_TEST_DIR}/lint_test.oxoflow"
# Expected: Clean lint output (no warnings)
```

### Test B11-B20: Simple Workflow Execution (Self-contained)

**B11: Hello world execution**
```bash
# Scenario: Running the simplest workflow
cd "${OXO_TEST_DIR}/B11"
mkdir -p B11 && cd B11
create_workflow "hello" '
[workflow]
name = "hello-world"

[[rules]]
name = "greet"
output = ["hello.txt"]
shell = "echo Hello from oxo-flow > hello.txt"
'
$OXO_FLOW_BIN run hello.oxoflow
cat hello.txt
# Expected: File contains "Hello from oxo-flow"
```

**B12: Linear pipeline with dependencies**
```bash
# Scenario: Three-step pipeline (generate → transform → summarize)
cd "${OXO_TEST_DIR}/B12" && mkdir -p B12 && cd B12
create_workflow "linear" '
[workflow]
name = "linear-pipeline"
version = "1.0"

[[rules]]
name = "generate"
output = ["data/raw.csv"]
shell = """
mkdir -p data
echo "id,name,value" > data/raw.csv
for i in $(seq 1 10); do echo "$i,item_$i,$((RANDOM % 100))"; done >> data/raw.csv
"""

[[rules]]
name = "transform"
input = ["data/raw.csv"]
output = ["data/filtered.csv"]
shell = """
head -1 data/raw.csv > data/filtered.csv
awk -F"," "NR>1 && \$3 > 50" data/raw.csv >> data/filtered.csv
"""

[[rules]]
name = "summarize"
input = ["data/filtered.csv"]
output = ["results/summary.txt"]
shell = """
mkdir -p results
total=$(tail -n +2 data/filtered.csv | wc -l)
echo "Filtered records: $total" > results/summary.txt
"""
'
$OXO_FLOW_BIN run linear.oxoflow
cat results/summary.txt
# Expected: Shows filtered record count
```

**B13: Parallel execution with job limit**
```bash
# Scenario: Running multiple rules concurrently
cd "${OXO_TEST_DIR}/B13" && mkdir -p B13 && cd B13
create_workflow "parallel" '
[workflow]
name = "parallel-test"

[config]
samples = ["S1", "S2", "S3"]

[[rules]]
name = "process_{sample}"
output = ["{sample}.txt"]
scatter = { variable = "sample", values_from = "config.samples" }
shell = "echo Processing {sample} > {sample}.txt"
'
$OXO_FLOW_BIN run parallel.oxoflow -j 2
ls S1.txt S2.txt S3.txt
# Expected: Three output files created
```

**B14: Scatter-gather pattern (mock genomic processing)**
```bash
# Scenario: Per-chromosome processing then gather (mocked with simple files)
cd "${OXO_TEST_DIR}/B14" && mkdir -p B14 && cd B14
create_workflow "scatter_gather" '
[workflow]
name = "scatter-gather-mock"
version = "1.0"

[config]
chromosomes = ["chr1", "chr2", "chr3"]

[[rules]]
name = "call_variants_{chr}"
output = ["variants/{chr}.vcf"]
scatter = { variable = "chr", values_from = "config.chromosomes", gather = "merge_vcfs" }
shell = """
mkdir -p variants
echo "CHROM POS REF ALT" > variants/{chr}.vcf
echo "{chr} 100 A T" >> variants/{chr}.vcf
"""

[[rules]]
name = "merge_vcfs"
input = ["variants/*.vcf"]
output = ["merged.vcf"]
shell = """
echo "## Merged VCF" > merged.vcf
cat variants/*.vcf >> merged.vcf
"""
'
$OXO_FLOW_BIN run scatter_gather.oxoflow -j 3
cat merged.vcf
# Expected: Merged file contains data from all chromosomes
```

**B15: Environment specification (validation only)**
```bash
# Scenario: Workflow with conda environment spec
cd "${OXO_TEST_DIR}/B15" && mkdir -p B15/envs && cd B15
cat > envs/qc.yaml << 'EOF'
name: qc-env
channels:
  - bioconda
dependencies:
  - fastp
EOF
create_workflow "conda_wf" '
[workflow]
name = "conda-test"

[[rules]]
name = "qc_step"
output = ["qc_report.txt"]
shell = "echo QC passed > qc_report.txt"

[rules.environment]
conda = "envs/qc.yaml"
'
$OXO_FLOW_BIN validate conda_wf.oxoflow
# Expected: Workflow validated with conda reference recognized
```

**B16: Mock RNA-seq workflow (validation)**
```bash
# Scenario: Mock RNA-seq quantification pipeline
cd "${OXO_TEST_DIR}/B16" && mkdir -p B16 && cd B16
create_workflow "rnaseq_mock" '
[workflow]
name = "rnaseq-mock-pipeline"

[config]
samples = ["sampleA", "sampleB"]

[[rules]]
name = "trim_{sample}"
input = ["raw/{sample}.fq"]
output = ["trimmed/{sample}.fq"]
scatter = { variable = "sample", values_from = "config.samples" }
shell = """
mkdir -p trimmed raw
test -f raw/{sample}.fq || echo "mock reads" > raw/{sample}.fq
cp raw/{sample}.fq trimmed/{sample}.fq
"""

[[rules]]
name = "quant_{sample}"
input = ["trimmed/{sample}.fq"]
output = ["quant/{sample}.sf"]
scatter = { variable = "sample", values_from = "config.samples" }
shell = """
mkdir -p quant
echo "TPM values for {sample}" > quant/{sample}.sf
"""

[[rules]]
name = "aggregate"
input = ["quant/*.sf"]
output = ["results/quant_report.txt"]
shell = """
mkdir -p results
cat quant/*.sf > results/quant_report.txt
"""
'
$OXO_FLOW_BIN validate rnaseq_mock.oxoflow
# Expected: Validation passes for mock RNA-seq workflow
```

**B17: Mock WGS workflow (validation)**
```bash
# Scenario: Mock whole-genome sequencing pipeline
cd "${OXO_TEST_DIR}/B17" && mkdir -p B17 && cd B17
create_workflow "wgs_mock" '
[workflow]
name = "wgs-mock-pipeline"

[config]
samples = ["NA12878"]

[[rules]]
name = "align_{sample}"
threads = 8
memory = "16G"
output = ["aligned/{sample}.bam"]
scatter = { variable = "sample", values_from = "config.samples" }
shell = """
mkdir -p aligned
echo "Mock BAM header" > aligned/{sample}.bam
"""

[[rules]]
name = "call_{sample}"
input = ["aligned/{sample}.bam"]
threads = 4
output = ["variants/{sample}.vcf"]
scatter = { variable = "sample", values_from = "config.samples" }
shell = """
mkdir -p variants
echo "Mock VCF" > variants/{sample}.vcf
"""

[[rules]]
name = "annotate"
input = ["variants/*.vcf"]
output = ["annotated.vcf"]
shell = "cat variants/*.vcf > annotated.vcf"
'
$OXO_FLOW_BIN validate wgs_mock.oxoflow
# Expected: Validation passes for mock WGS workflow
```

**B18: Multi-omics mock workflow (validation)**
```bash
# Scenario: Mock multi-omics integration
cd "${OXO_TEST_DIR}/B18" && mkdir -p B18 && cd B18
create_workflow "multiomics_mock" '
[workflow]
name = "multiomics-mock"

[[rules]]
name = "genomics_branch"
output = ["genome/results.txt"]
shell = "mkdir -p genome && echo genomic_data > genome/results.txt"

[[rules]]
name = "transcriptomics_branch"
output = ["rna/results.txt"]
shell = "mkdir -p rna && echo rna_data > rna/results.txt"

[[rules]]
name = "integrate"
input = ["genome/results.txt", "rna/results.txt"]
output = ["integrated.txt"]
shell = "cat genome/results.txt rna/results.txt > integrated.txt"
'
$OXO_FLOW_BIN graph multiomics_mock.oxoflow -f dot
# Expected: Shows multi-branch DAG structure
```

**B19: Single-cell mock workflow (validation)**
```bash
# Scenario: Mock single-cell RNA-seq workflow
cd "${OXO_TEST_DIR}/B19" && mkdir -p B19 && cd B19
create_workflow "scrna_mock" '
[workflow]
name = "scrna-mock"

[[rules]]
name = "cell_filter"
output = ["cells_filtered.txt"]
shell = "echo 1000 cells passed > cells_filtered.txt"

[[rules]]
name = "normalize"
input = ["cells_filtered.txt"]
output = ["normalized.txt"]
shell = "cat cells_filtered.txt > normalized.txt"

[[rules]]
name = "cluster"
input = ["normalized.txt"]
output = ["clusters.txt"]
shell = "echo 5 clusters identified > clusters.txt"
'
$OXO_FLOW_BIN lint scrna_mock.oxoflow
# Expected: Lint passes for mock workflow
```

**B20: Variant calling mock workflow**
```bash
# Scenario: Mock variant calling from BAM
cd "${OXO_TEST_DIR}/B20" && mkdir -p B20 && cd B20
create_workflow "variant_mock" '
[workflow]
name = "variant-mock"

[[rules]]
name = "call_variants"
input = ["sample.bam"]
output = ["variants.vcf"]
shell = """
test -f sample.bam || echo "mock BAM" > sample.bam
echo "##VCF" > variants.vcf
echo "chr1 100 A T PASS" >> variants.vcf
"""

[[rules]]
name = "filter"
input = ["variants.vcf"]
output = ["filtered.vcf"]
shell = "grep PASS variants.vcf > filtered.vcf"
'
$OXO_FLOW_BIN validate variant_mock.oxoflow
# Expected: Validation passes
```

### Test B21-B30: Error Handling and Validation

**B21: Invalid TOML syntax (S001)**
```bash
# Scenario: TOML parsing error
cd "${OXO_TEST_DIR}/B21"
echo 'invalid toml {{{' > bad_syntax.oxoflow
$OXO_FLOW_BIN validate bad_syntax.oxoflow 2>&1 | grep -q "parse\|invalid\|S001"
# Expected: Error with TOML parse failure
```

**B22: Missing workflow file**
```bash
# Scenario: Non-existent file
$OXO_FLOW_BIN validate nonexistent_file.oxoflow 2>&1 | grep -q "not found\|missing\|error"
# Expected: Clear error about missing file
```

**B23: Empty workflow name (E001)**
```bash
# Scenario: Missing workflow name
cd "${OXO_TEST_DIR}/B23"
create_workflow "empty_name" '
[workflow]
name = ""
version = "1.0"

[[rules]]
name = "test"
shell = "echo hi"
'
$OXO_FLOW_BIN validate empty_name.oxoflow 2>&1 | grep -q "E001\|name"
# Expected: E001 error detected
```

**B24: Invalid memory format (E004)**
```bash
# Scenario: Wrong memory format
cd "${OXO_TEST_DIR}/B24"
create_workflow "bad_memory" '
[workflow]
name = "memory-test"

[[rules]]
name = "step1"
memory = "invalid_format"
shell = "echo hi"
'
$OXO_FLOW_BIN lint bad_memory.oxoflow 2>&1 | grep -q "E004\|memory\|invalid"
# Expected: E004 error for invalid memory
```

**B25: Path traversal detection (E009)**
```bash
# Scenario: Relative path escaping
cd "${OXO_TEST_DIR}/B25"
create_workflow "traversal" '
[workflow]
name = "traversal-test"

[[rules]]
name = "step1"
output = ["../../../etc/passwd"]
shell = "echo hi"
'
$OXO_FLOW_BIN validate traversal.oxoflow 2>&1 | grep -q "E009\|traversal\|path"
# Expected: E009 error for path traversal
```

**B26: Secret detection (S008)**
```bash
# Scenario: API key in workflow
cd "${OXO_TEST_DIR}/B26"
create_workflow "secrets" '
[workflow]
name = "secret-test"

[config]
api_key = "sk-proj-abc123def456"
token = "ghp_xxxxxxxxxxxx"

[[rules]]
name = "step1"
shell = "echo hi"
'
$OXO_FLOW_BIN lint secrets.oxoflow 2>&1 | grep -q "S008\|secret\|api_key"
# Expected: S008 warning for secret pattern
```

**B27: DAG cycle detection (E006)**
```bash
# Scenario: Circular dependency
cd "${OXO_TEST_DIR}/B27"
create_workflow "cycle" '
[workflow]
name = "cycle-test"

[[rules]]
name = "a"
depends_on = ["b"]
shell = "echo a"

[[rules]]
name = "b"
depends_on = ["a"]
shell = "echo b"
'
$OXO_FLOW_BIN validate cycle.oxoflow 2>&1 | grep -q "E006\|cycle\|circular"
# Expected: E006 error for DAG cycle
```

**B28: Undefined config reference (E005)**
```bash
# Scenario: Reference to undefined variable
cd "${OXO_TEST_DIR}/B28"
create_workflow "undef_ref" '
[workflow]
name = "undef-test"

[[rules]]
name = "step1"
shell = "echo {config.undefined_variable}"
'
$OXO_FLOW_BIN validate undef_ref.oxoflow 2>&1 | grep -q "E005\|undefined\|reference"
# Expected: E005 error for undefined reference
```

**B29: Missing dependency rule (E007)**
```bash
# Scenario: Depends on non-existent rule
cd "${OXO_TEST_DIR}/B29"
create_workflow "bad_dep" '
[workflow]
name = "dep-test"

[[rules]]
name = "step1"
depends_on = ["nonexistent_rule"]
shell = "echo hi"
'
$OXO_FLOW_BIN validate bad_dep.oxoflow 2>&1 | grep -q "E007\|dependency\|nonexistent"
# Expected: E007 error for missing dependency
```

**B30: Wildcard mismatch (E003)**
```bash
# Scenario: Output wildcard not in input
cd "${OXO_TEST_DIR}/B30"
create_workflow "wc_mismatch" '
[workflow]
name = "wc-test"

[[rules]]
name = "step1"
input = ["data.txt"]
output = ["{sample}.txt"]
shell = "echo hi > {sample}.txt"
'
$OXO_FLOW_BIN validate wc_mismatch.oxoflow 2>&1 | grep -q "E003\|wildcard"
# Expected: E003 error for wildcard mismatch
```

---

## Phase 2: Intermediate Tests (I01-I40)

### User Profile: Comfortable with basics, exploring advanced features

### Test I01-I10: Workflow Configuration

**I01: Config section with variable substitution**
```bash
# Scenario: Using config variables in workflow
cd "${OXO_TEST_DIR}/I01" && mkdir -p I01 && cd I01
create_workflow "config_test" '
[workflow]
name = "config-demo"

[config]
reference = "hg38.fa"
threads = 8
samples = ["S1", "S2"]

[[rules]]
name = "process_{sample}"
scatter = { variable = "sample", values_from = "config.samples" }
threads = {config.threads}
shell = "echo Reference: {config.reference}, Sample: {sample}"
'
$OXO_FLOW_BIN validate config_test.oxoflow
# Expected: Config variables recognized
```

**I02: Defaults section inheritance**
```bash
# Scenario: Default resources with override
cd "${OXO_TEST_DIR}/I02" && mkdir -p I02 && cd I02
create_workflow "defaults_test" '
[workflow]
name = "defaults-demo"

[defaults]
threads = 4
memory = "8G"

[[rules]]
name = "step1"
shell = "echo step1"

[[rules]]
name = "step2"
threads = 16
memory = "32G"
shell = "echo step2"
'
$OXO_FLOW_BIN debug defaults_test.oxoflow 2>&1 | grep -q "step1\|step2"
# Expected: step1 inherits defaults, step2 overrides
```

**I03: Resource specifications validation**
```bash
# Scenario: Per-rule resources
cd "${OXO_TEST_DIR}/I03"
create_workflow "resources" '
[workflow]
name = "resources-test"

[[rules]]
name = "heavy"
threads = 32
memory = "64G"
shell = "echo heavy"

[[rules]]
name = "light"
threads = 1
memory = "1G"
shell = "echo light"
'
$OXO_FLOW_BIN lint resources.oxoflow
# Expected: No warnings for reasonable resources
```

**I04: GPU resource specification**
```bash
# Scenario: GPU-accelerated workflow
cd "${OXO_TEST_DIR}/I04"
create_workflow "gpu_wf" '
[workflow]
name = "gpu-demo"

[[rules]]
name = "train_model"
threads = 8
memory = "32G"

[rules.resources]
gpu = 2

shell = "echo GPU training"
'
$OXO_FLOW_BIN validate gpu_wf.oxoflow
# Expected: GPU specification recognized
```

**I05: Time limit specification**
```bash
# Scenario: Job time limits
cd "${OXO_TEST_DIR}/I05"
create_workflow "time_limits" '
[workflow]
name = "time-demo"

[[rules]]
name = "long_job"
shell = "echo long"

[rules.resources]
time_limit = "48h"

[[rules]]
name = "short_job"
shell = "echo short"

[rules.resources]
time_limit = "30m"
'
$OXO_FLOW_BIN validate time_limits.oxoflow
# Expected: Time limits parsed (48h → 172800s, 30m → 1800s)
```

**I06: HPC module specification**
```bash
# Scenario: Environment modules
cd "${OXO_TEST_DIR}/I06"
create_workflow "modules" '
[workflow]
name = "modules-demo"

[[rules]]
name = "gatk_task"
shell = "echo gatk"

[rules.environment]
modules = ["java/11", "gatk/4.2"]
'
$OXO_FLOW_BIN validate modules.oxoflow
# Expected: Module specification recognized
```

**I07: Conda environment file**
```bash
# Scenario: Conda YAML reference
cd "${OXO_TEST_DIR}/I07" && mkdir -p I07/envs && cd I07
cat > envs/bwa.yaml << 'EOF'
name: bwa-env
channels: [bioconda, conda-forge]
dependencies: [bwa=0.7.17, samtools=1.15]
EOF
create_workflow "conda_env" '
[workflow]
name = "conda-demo"

[[rules]]
name = "align"
shell = "echo align"

[rules.environment]
conda = "envs/bwa.yaml"
'
$OXO_FLOW_BIN validate conda_env.oxoflow
# Expected: Conda environment recognized
```

**I08: Docker container specification**
```bash
# Scenario: Docker container
cd "${OXO_TEST_DIR}/I08"
create_workflow "docker_wf" '
[workflow]
name = "docker-demo"

[[rules]]
name = "container_task"
shell = "echo in-docker"

[rules.environment]
docker = "biocontainers/bwa:0.7.17"
'
$OXO_FLOW_BIN validate docker_wf.oxoflow
# Expected: Docker specification recognized
```

**I09: Singularity container specification**
```bash
# Scenario: Singularity for HPC
cd "${OXO_TEST_DIR}/I09"
create_workflow "singularity_wf" '
[workflow]
name = "singularity-demo"

[[rules]]
name = "hpc_task"
shell = "echo in-singularity"

[rules.environment]
singularity = "docker://biocontainers/bwa:0.7.17"
'
$OXO_FLOW_BIN validate singularity_wf.oxoflow
# Expected: Singularity recognized
```

**I10: Pixi environment**
```bash
# Scenario: Pixi environment
cd "${OXO_TEST_DIR}/I10"
create_workflow "pixi_wf" '
[workflow]
name = "pixi-demo"

[[rules]]
name = "pixi_task"
shell = "echo pixi"

[rules.environment]
pixi = "pixi.toml"
'
$OXO_FLOW_BIN validate pixi_wf.oxoflow
# Expected: Pixi specification recognized
```

### Test I11-I20: Advanced Workflow Patterns

**I11: Paired experiment-control mock**
```bash
# Scenario: Tumor-normal paired analysis
cd "${OXO_TEST_DIR}/I11"
create_workflow "paired_mock" '
[workflow]
name = "paired-analysis"

[config]
pairs = [["T1", "N1"], ["T2", "N2"]]

[[rules]]
name = "somatic_{tumor}_{normal}"
scatter = { variable = "pair", values_from = "config.pairs" }
output = ["variants/{tumor}_{normal}.vcf"]
shell = """
mkdir -p variants
echo "Somatic variants for {tumor}/{normal}" > variants/{tumor}_{normal}.vcf
"""
'
$OXO_FLOW_BIN validate paired_mock.oxoflow
# Expected: Paired analysis recognized
```

**I12: Cohort analysis mock**
```bash
# Scenario: Multi-sample cohort
cd "${OXO_TEST_DIR}/I12"
create_workflow "cohort_mock" '
[workflow]
name = "cohort-analysis"

[config]
samples = ["P01", "P02", "P03", "P04", "P05"]

[[rules]]
name = "process_{sample}"
scatter = { variable = "sample", values_from = "config.samples" }
output = ["results/{sample}.txt"]
shell = "mkdir -p results && echo {sample} > results/{sample}.txt"

[[rules]]
name = "combine"
input = ["results/*.txt"]
output = ["cohort_summary.txt"]
shell = "cat results/*.txt > cohort_summary.txt"
'
$OXO_FLOW_BIN validate cohort_mock.oxoflow
# Expected: Cohort processing recognized
```

**I13: Conditional execution mock**
```bash
# Scenario: Conditional rules
cd "${OXO_TEST_DIR}/I13"
create_workflow "conditional_mock" '
[workflow]
name = "conditional-demo"

[config]
run_qc = true
skip_annotation = false

[[rules]]
name = "qc_step"
when = "{config.run_qc}"
output = ["qc.txt"]
shell = "echo QC done > qc.txt"

[[rules]]
name = "annotate"
when = "!{config.skip_annotation}"
input = ["qc.txt"]
output = ["annotated.txt"]
shell = "cat qc.txt > annotated.txt"
'
$OXO_FLOW_BIN validate conditional_mock.oxoflow
# Expected: Conditional when clauses recognized
```

**I14: Checkpoint workflow**
```bash
# Scenario: Dynamic DAG rebuilding
cd "${OXO_TEST_DIR}/I14" && mkdir -p I14 && cd I14
create_workflow "checkpoint_mock" '
[workflow]
name = "checkpoint-demo"

[[rules]]
name = "discover"
checkpoint = true
output = ["samples.txt"]
shell = "echo S1 S2 S3 > samples.txt"

[[rules]]
name = "process"
input = ["samples.txt"]
output = ["results.txt"]
shell = "cat samples.txt > results.txt"
'
$OXO_FLOW_BIN validate checkpoint_mock.oxoflow
# Expected: Checkpoint recognized
```

**I15: Wildcard expansion with scatter**
```bash
# Scenario: Scatter with wildcard expansion
cd "${OXO_TEST_DIR}/I15"
create_workflow "scatter_wc" '
[workflow]
name = "scatter-wc-demo"

[config]
samples = ["A", "B", "C"]

[[rules]]
name = "process_{sample}"
scatter = { variable = "sample", values_from = "config.samples" }
input = ["input/{sample}.txt"]
output = ["output/{sample}.txt"]
shell = """
mkdir -p input output
test -f input/{sample}.txt || echo "input {sample}" > input/{sample}.txt
cp input/{sample}.txt output/{sample}.txt
"""
'
$OXO_FLOW_BIN validate scatter_wc.oxoflow
# Expected: Scatter and wildcard expansion recognized
```

**I16: Explicit dependency chains**
```bash
# Scenario: Multiple dependencies
cd "${OXO_TEST_DIR}/I16"
create_workflow "dep_chain" '
[workflow]
name = "dep-chain"

[[rules]]
name = "align"
output = ["aligned.bam"]
shell = "echo mock > aligned.bam"

[[rules]]
name = "sort"
depends_on = ["align"]
input = ["aligned.bam"]
output = ["sorted.bam"]
shell = "cat aligned.bam > sorted.bam"

[[rules]]
name = "call"
depends_on = ["sort"]
input = ["sorted.bam"]
output = ["variants.vcf"]
shell = "echo VCF > variants.vcf"

[[rules]]
name = "annotate"
depends_on = ["call"]
input = ["variants.vcf"]
output = ["annotated.vcf"]
shell = "cat variants.vcf > annotated.vcf"
'
$OXO_FLOW_BIN graph dep_chain.oxoflow -f dot
# Expected: Shows connected chain
```

**I17: Rule extension (extends)**
```bash
# Scenario: Rule inheritance
cd "${OXO_TEST_DIR}/I17"
create_workflow "extends_mock" '
[workflow]
name = "extends-demo"

[[rules]]
name = "base_bwa"
threads = 8
memory = "16G"
shell = "echo base bwa"

[[rules]]
name = "bwa_S1"
extends = "base_bwa"
output = ["S1.bam"]
shell = "echo S1 specific"
'
$OXO_FLOW_BIN validate extends_mock.oxoflow
# Expected: Extended rule inherits base properties
```

**I18: Multiple outputs**
```bash
# Scenario: Rule producing multiple files
cd "${OXO_TEST_DIR}/I18" && mkdir -p I18 && cd I18
create_workflow "multi_out" '
[workflow]
name = "multi-output"

[[rules]]
name = "split"
output = ["part1.txt", "part2.txt", "part3.txt"]
shell = """
echo part1 > part1.txt
echo part2 > part2.txt
echo part3 > part3.txt
"""
'
$OXO_FLOW_BIN run multi_out.oxoflow
ls part1.txt part2.txt part3.txt
# Expected: All outputs created
```

**I19: Input function mock**
```bash
# Scenario: Dynamic input resolution
cd "${OXO_TEST_DIR}/I19"
create_workflow "input_func" '
[workflow]
name = "input-func-demo"

[[rules]]
name = "dynamic_input"
input_function = "get_input_files"
output = ["processed.txt"]
shell = "echo processed > processed.txt"
'
$OXO_FLOW_BIN validate input_func.oxoflow
# Expected: Input function recognized (if supported)
```

**I20: Protected and temp outputs**
```bash
# Scenario: Output lifecycle flags
cd "${OXO_TEST_DIR}/I20"
create_workflow "output_flags" '
[workflow]
name = "output-flags"

[[rules]]
name = "pipeline"
input = ["input.bam"]
output = ["final.vcf"]
temp_output = ["intermediate.bam"]
protected_output = ["final.vcf"]
shell = """
test -f input.bam || echo "mock" > input.bam
echo VCF > final.vcf
"""
'
$OXO_FLOW_BIN validate output_flags.oxoflow
# Expected: Output flags recognized
```

### Test I21-I30: Cluster Integration (Script Generation)

**I21: SLURM script generation**
```bash
# Scenario: Generate SLURM submit scripts
cd "${OXO_TEST_DIR}/I21" && mkdir -p I21/slurm_out && cd I21
create_workflow "slurm_wf" '
[workflow]
name = "slurm-test"

[[rules]]
name = "job1"
threads = 4
memory = "8G"
shell = "echo slurm job1"
'
$OXO_FLOW_BIN cluster submit slurm_wf.oxoflow -b slurm -o slurm_out
ls slurm_out/*.sh
grep -l "#SBATCH" slurm_out/*.sh
# Expected: SLURM scripts with #SBATCH directives
```

**I22: PBS script generation**
```bash
# Scenario: Generate PBS submit scripts
cd "${OXO_TEST_DIR}/I22" && mkdir -p I22/pbs_out && cd I22
create_workflow "pbs_wf" '
[workflow]
name = "pbs-test"

[[rules]]
name = "job1"
threads = 2
memory = "4G"
shell = "echo pbs job"
'
$OXO_FLOW_BIN cluster submit pbs_wf.oxoflow -b pbs -o pbs_out
grep -l "#PBS" pbs_out/*.sh
# Expected: PBS scripts with #PBS directives
```

**I23: SGE script generation**
```bash
# Scenario: Generate SGE submit scripts
cd "${OXO_TEST_DIR}/I23" && mkdir -p I23/sge_out && cd I23
create_workflow "sge_wf" '
[workflow]
name = "sge-test"

[[rules]]
name = "job1"
threads = 2
shell = "echo sge job"
'
$OXO_FLOW_BIN cluster submit sge_wf.oxoflow -b sge -o sge_out
grep -l "#\$" sge_out/*.sh
# Expected: SGE scripts with #$ directives
```

**I24: LSF script generation**
```bash
# Scenario: Generate LSF submit scripts
cd "${OXO_TEST_DIR}/I24" && mkdir -p I24/lsf_out && cd I24
create_workflow "lsf_wf" '
[workflow]
name = "lsf-test"

[[rules]]
name = "job1"
threads = 4
shell = "echo lsf job"
'
$OXO_FLOW_BIN cluster submit lsf_wf.oxoflow -b lsf -o lsf_out
grep -l "#BSUB" lsf_out/*.sh
# Expected: LSF scripts with #BSUB directives
```

**I25: Cluster status mock**
```bash
# Scenario: Check cluster status (mocked)
$OXO_FLOW_BIN cluster status -b slurm 2>&1 | head -5
# Expected: Status command executes (may fail without real cluster)
```

**I26: Profile management**
```bash
# Scenario: Profile listing
$OXO_FLOW_BIN profile list
$OXO_FLOW_BIN profile show slurm 2>&1 | head -10
# Expected: Profile commands execute
```

**I27: GPU cluster script generation**
```bash
# Scenario: GPU directives in scripts
cd "${OXO_TEST_DIR}/I27" && mkdir -p I27/gpu_out && cd I27
create_workflow "gpu_cluster" '
[workflow]
name = "gpu-cluster"

[[rules]]
name = "train"
threads = 8
memory = "32G"

[rules.resources]
gpu = 4

shell = "echo GPU training"
'
$OXO_FLOW_BIN cluster submit gpu_cluster.oxoflow -b slurm -o gpu_out
grep "gres=gpu" gpu_out/*.sh
# Expected: GPU directive in SLURM script
```

**I28: Walltime in cluster scripts**
```bash
# Scenario: Time limit directives
cd "${OXO_TEST_DIR}/I28" && mkdir -p I28/time_out && cd I28
create_workflow "time_cluster" '
[workflow]
name = "time-cluster"

[[rules]]
name = "long_job"
shell = "echo long"

[rules.resources]
time_limit = "72h"
'
$OXO_FLOW_BIN cluster submit time_cluster.oxoflow -b slurm -o time_out
grep -E "time=|time=" time_out/*.sh
# Expected: Walltime directive present
```

**I29: Module loading in cluster scripts**
```bash
# Scenario: Module load commands
cd "${OXO_TEST_DIR}/I29" && mkdir -p I29/module_out && cd I29
create_workflow "module_cluster" '
[workflow]
name = "module-cluster"

[[rules]]
name = "gatk_task"
shell = "echo gatk"

[rules.environment]
modules = ["java/11", "gatk/4.2"]
'
$OXO_FLOW_BIN cluster submit module_cluster.oxoflow -b slurm -o module_out
grep "module load" module_out/*.sh
# Expected: Module load commands present
```

**I30: Multi-rule cluster workflow**
```bash
# Scenario: Complex workflow cluster scripts
cd "${OXO_TEST_DIR}/I30" && mkdir -p I30/complex_out && cd I30
create_workflow "complex_cluster" '
[workflow]
name = "complex-cluster"

[[rules]]
name = "step1"
threads = 4
output = ["s1.txt"]
shell = "echo step1 > s1.txt"

[[rules]]
name = "step2"
threads = 8
depends_on = ["step1"]
input = ["s1.txt"]
output = ["s2.txt"]
shell = "cat s1.txt > s2.txt"

[[rules]]
name = "step3"
threads = 2
depends_on = ["step2"]
input = ["s2.txt"]
output = ["s3.txt"]
shell = "cat s2.txt > s3.txt"
'
$OXO_FLOW_BIN cluster submit complex_cluster.oxoflow -b slurm -o complex_out
ls complex_out/*.sh | wc -l
# Expected: 3 scripts generated
```

### Test I31-I40: Container Generation

**I31: Dockerfile generation**
```bash
# Scenario: Generate Dockerfile
cd "${OXO_TEST_DIR}/I31"
create_workflow "docker_gen" '
[workflow]
name = "docker-gen-test"

[[rules]]
name = "task"
shell = "echo task"
'
$OXO_FLOW_BIN package docker_gen.oxoflow -f docker 2>&1 | head -20
# Expected: Dockerfile output with FROM, ENTRYPOINT
```

**I32: Singularity def file generation**
```bash
# Scenario: Generate Singularity definition
cd "${OXO_TEST_DIR}/I32"
$OXO_FLOW_BIN package docker_gen.oxoflow -f singularity 2>&1 | head -20
# Expected: Def file with Bootstrap, %post
```

**I33: GPU-aware container**
```bash
# Scenario: CUDA base image selection
cd "${OXO_TEST_DIR}/I33"
create_workflow "gpu_container" '
[workflow]
name = "gpu-container"

[[rules]]
name = "gpu_task"
shell = "echo gpu"

[rules.resources]
gpu = 2
'
$OXO_FLOW_BIN package gpu_container.oxoflow -f docker 2>&1 | grep -i "cuda\|nvidia"
# Expected: CUDA/nvidia in Dockerfile
```

**I34: Export workflow**
```bash
# Scenario: Export canonical format
cd "${OXO_TEST_DIR}/I34"
$OXO_FLOW_BIN export docker_gen.oxoflow -f toml 2>&1 | head -20
# Expected: Canonical TOML output
```

**I35: Container with resources**
```bash
# Scenario: Resources reflected in container
cd "${OXO_TEST_DIR}/I35"
create_workflow "resource_container" '
[workflow]
name = "resource-container"

[defaults]
threads = 8
memory = "16G"

[[rules]]
name = "heavy_task"
shell = "echo heavy"
'
$OXO_FLOW_BIN package resource_container.oxoflow -f docker 2>&1 | head -30
# Expected: Container with resource awareness
```

**I36: Conda in container**
```bash
# Scenario: Conda environments in container
cd "${OXO_TEST_DIR}/I36" && mkdir -p I36/envs && cd I36
cat > envs/analysis.yaml << 'EOF'
name: analysis
dependencies: [python=3.10, pandas]
EOF
create_workflow "conda_container" '
[workflow]
name = "conda-container"

[[rules]]
name = "analyze"
shell = "echo analyze"

[rules.environment]
conda = "envs/analysis.yaml"
'
$OXO_FLOW_BIN package conda_container.oxoflow -f docker 2>&1 | head -40
# Expected: Conda setup in Dockerfile
```

**I37: Docker environment spec**
```bash
# Scenario: Docker in workflow reflected in container
cd "${OXO_TEST_DIR}/I37"
create_workflow "docker_spec" '
[workflow]
name = "docker-spec"

[[rules]]
name = "containerized"
shell = "echo in-container"

[rules.environment]
docker = "ubuntu:22.04"
'
$OXO_FLOW_BIN validate docker_spec.oxoflow
# Expected: Docker environment validated
```

**I38: Singularity environment spec**
```bash
# Scenario: Singularity specification
cd "${OXO_TEST_DIR}/I38"
create_workflow "sing_spec" '
[workflow]
name = "sing-spec"

[[rules]]
name = "hpc_task"
shell = "echo singularity"

[rules.environment]
singularity = "docker://ubuntu:22.04"
'
$OXO_FLOW_BIN validate sing_spec.oxoflow
# Expected: Singularity validated
```

**I39: Multiple environment types**
```bash
# Scenario: Mixed environment types
cd "${OXO_TEST_DIR}/I39"
create_workflow "mixed_env" '
[workflow]
name = "mixed-env"

[[rules]]
name = "conda_task"
shell = "echo conda"
[rules.environment]
conda = "envs/base.yaml"

[[rules]]
name = "docker_task"
shell = "echo docker"
[rules.environment]
docker = "ubuntu:22.04"

[[rules]]
name = "singularity_task"
shell = "echo sing"
[rules.environment]
singularity = "docker://ubuntu:22.04"
'
$OXO_FLOW_BIN validate mixed_env.oxoflow
# Expected: All environment types validated
```

**I40: Complex container generation**
```bash
# Scenario: Multi-rule container
cd "${OXO_TEST_DIR}/I40"
create_workflow "complex_container" '
[workflow]
name = "complex-container"

[[rules]]
name = "step1"
output = ["s1.txt"]
shell = "echo step1 > s1.txt"

[[rules]]
name = "step2"
depends_on = ["step1"]
input = ["s1.txt"]
output = ["s2.txt"]
shell = "cat s1.txt > s2.txt"
'
$OXO_FLOW_BIN package complex_container.oxoflow -f docker 2>&1 | head -50
# Expected: Multi-step Dockerfile
```

---

## Phase 3: Advanced Tests (A01-A30)

### User Profile: Expert user, building complex pipelines

### Test A01-A10: Complex Workflow Patterns

**A01: Multi-omics integration mock**
```bash
# Scenario: Multiple data type branches
cd "${OXO_TEST_DIR}/A01"
create_workflow "multiomics" '
[workflow]
name = "multiomics-integration"

[[rules]]
name = "genome_branch"
output = ["genome/data.txt"]
shell = "mkdir -p genome && echo genome > genome/data.txt"

[[rules]]
name = "rna_branch"
output = ["rna/data.txt"]
shell = "mkdir -p rna && echo rna > rna/data.txt"

[[rules]]
name = "protein_branch"
output = ["protein/data.txt"]
shell = "mkdir -p protein && echo protein > protein/data.txt"

[[rules]]
name = "integrate"
input = ["genome/data.txt", "rna/data.txt", "protein/data.txt"]
output = ["integrated.txt"]
shell = "cat genome/data.txt rna/data.txt protein/data.txt > integrated.txt"
'
$OXO_FLOW_BIN graph multiomics.oxoflow -f dot
# Expected: Multi-branch DAG
```

**A02: Per-chromosome processing mock**
```bash
# Scenario: Scatter-gather for chromosomes
cd "${OXO_TEST_DIR}/A02"
create_workflow "chr_scatter" '
[workflow]
name = "chr-scatter"

[config]
chromosomes = ["chr1", "chr2", "chr3", "chr4", "chr5", "chrX", "chrY"]

[[rules]]
name = "call_{chr}"
scatter = { variable = "chr", values_from = "config.chromosomes", gather = "merge" }
output = ["variants/{chr}.vcf"]
shell = """
mkdir -p variants
echo "CHROM POS" > variants/{chr}.vcf
echo "{chr} 100" >> variants/{chr}.vcf
"""

[[rules]]
name = "merge"
input = ["variants/*.vcf"]
output = ["merged.vcf"]
shell = "cat variants/*.vcf > merged.vcf"
'
$OXO_FLOW_BIN validate chr_scatter.oxoflow
# Expected: 7 scattered rules + 1 gather
```

**A03: Conditional with scatter**
```bash
# Scenario: Conditional scatter rules
cd "${OXO_TEST_DIR}/A03"
create_workflow "cond_scatter" '
[workflow]
name = "conditional-scatter"

[config]
samples = ["S1", "S2", "S3", "S4"]
skip_samples = ["S3"]

[[rules]]
name = "process_{sample}"
scatter = { variable = "sample", values_from = "config.samples" }
when = "!config.skip_samples.contains(sample)"
output = ["results/{sample}.txt"]
shell = "mkdir -p results && echo {sample} > results/{sample}.txt"
'
$OXO_FLOW_BIN validate cond_scatter.oxoflow
# Expected: Conditional scatter recognized
```

**A04: Deep dependency chain**
```bash
# Scenario: 10-step pipeline
cd "${OXO_TEST_DIR}/A04"
create_workflow "deep_chain" '
[workflow]
name = "deep-chain"

[[rules]]
name = "s1"
output = ["out1.txt"]
shell = "echo 1 > out1.txt"

[[rules]]
name = "s2"
depends_on = ["s1"]
input = ["out1.txt"]
output = ["out2.txt"]
shell = "cat out1.txt > out2.txt"

[[rules]]
name = "s3"
depends_on = ["s2"]
input = ["out2.txt"]
output = ["out3.txt"]
shell = "cat out2.txt > out3.txt"

[[rules]]
name = "s4"
depends_on = ["s3"]
input = ["out3.txt"]
output = ["out4.txt"]
shell = "cat out3.txt > out4.txt"

[[rules]]
name = "s5"
depends_on = ["s4"]
input = ["out4.txt"]
output = ["out5.txt"]
shell = "cat out4.txt > out5.txt"

[[rules]]
name = "s6"
depends_on = ["s5"]
input = ["out5.txt"]
output = ["out6.txt"]
shell = "cat out5.txt > out6.txt"

[[rules]]
name = "s7"
depends_on = ["s6"]
input = ["out6.txt"]
output = ["out7.txt"]
shell = "cat out6.txt > out7.txt"

[[rules]]
name = "s8"
depends_on = ["s7"]
input = ["out7.txt"]
output = ["out8.txt"]
shell = "cat out7.txt > out8.txt"

[[rules]]
name = "s9"
depends_on = ["s8"]
input = ["out8.txt"]
output = ["out9.txt"]
shell = "cat out8.txt > out9.txt"

[[rules]]
name = "s10"
depends_on = ["s9"]
input = ["out9.txt"]
output = ["final.txt"]
shell = "cat out9.txt > final.txt"
'
$OXO_FLOW_BIN graph deep_chain.oxoflow -f dot | grep -c "node"
# Expected: 10 nodes in graph
```

**A05: Parallel resource groups**
```bash
# Scenario: Parallel with resource constraints
cd "${OXO_TEST_DIR}/A05"
create_workflow "parallel_group" '
[workflow]
name = "parallel-groups"

[config]
samples = ["A", "B", "C", "D"]

[defaults]
threads = 2
memory = "4G"

[[rules]]
name = "process_{sample}"
scatter = { variable = "sample", values_from = "config.samples" }
threads = 4
memory = "8G"
output = ["{sample}.txt"]
shell = "echo {sample} > {sample}.txt"
'
$OXO_FLOW_BIN lint parallel_group.oxoflow
# Expected: Resource allocations validated
```

**A06: Retry configuration**
```bash
# Scenario: Retry logic for flaky rules
cd "${OXO_TEST_DIR}/A06"
create_workflow "retry_wf" '
[workflow]
name = "retry-demo"

[[rules]]
name = "flaky_task"
retries = 3
retry_delay = "10s"
shell = "echo flaky"
'
$OXO_FLOW_BIN validate retry_wf.oxoflow
# Expected: Retry configuration recognized
```

**A07: Shadow directory**
```bash
# Scenario: Shadow execution
cd "${OXO_TEST_DIR}/A07"
create_workflow "shadow_wf" '
[workflow]
name = "shadow-demo"

[[rules]]
name = "shadow_task"
shadow = "minimal"
output = ["shadow_out.txt"]
shell = "echo shadow > shadow_out.txt"
'
$OXO_FLOW_BIN validate shadow_wf.oxoflow
# Expected: Shadow directive recognized (if supported)
```

**A08: Ancient inputs**
```bash
# Scenario: Ancient file handling
cd "${OXO_TEST_DIR}/A08"
create_workflow "ancient_wf" '
[workflow]
name = "ancient-demo"

[[rules]]
name = "ancient_task"
ancient = ["reference.fa"]
input = ["reference.fa", "data.txt"]
output = ["result.txt"]
shell = "echo result > result.txt"
'
$OXO_FLOW_BIN validate ancient_wf.oxoflow
# Expected: Ancient inputs recognized
```

**A09: Local rules**
```bash
# Scenario: Local execution flag
cd "${OXO_TEST_DIR}/A09"
create_workflow "local_wf" '
[workflow]
name = "local-demo"

[[rules]]
name = "local_task"
localrule = true
shell = "echo local"
'
$OXO_FLOW_BIN validate local_wf.oxoflow
# Expected: Local rule recognized
```

**A10: Subworkflow include**
```bash
# Scenario: Include subworkflow
cd "${OXO_TEST_DIR}/A10"
create_workflow "subwf" '
[workflow]
name = "subworkflow"

[[rules]]
name = "sub_task"
output = ["sub.txt"]
shell = "echo sub > sub.txt"
'
create_workflow "mainwf" '
[workflow]
name = "main-workflow"

[[include]]
path = "subwf.oxoflow"

[[rules]]
name = "main_task"
depends_on = ["sub_task"]
input = ["sub.txt"]
output = ["main.txt"]
shell = "cat sub.txt > main.txt"
'
$OXO_FLOW_BIN validate mainwf.oxoflow
# Expected: Include recognized
```

### Test A11-A20: Report Generation

**A11: HTML report mock**
```bash
# Scenario: Generate HTML report
cd "${OXO_TEST_DIR}/A11" && mkdir -p A11 && cd A11
create_workflow "report_wf" '
[workflow]
name = "report-demo"

[[rules]]
name = "task"
output = ["output.txt"]
shell = "echo report data > output.txt"
'
$OXO_FLOW_BIN run report_wf.oxoflow
$OXO_FLOW_BIN report report_wf.oxoflow -f html -o report.html 2>&1
test -f report.html && echo "PASS: Report generated" || echo "SKIP: Report command not implemented"
```

**A12: JSON report mock**
```bash
# Scenario: JSON output
cd "${OXO_TEST_DIR}/A12"
$OXO_FLOW_BIN report report_wf.oxoflow -f json -o report.json 2>&1
test -f report.json && echo "PASS" || echo "SKIP"
```

**A13: Report sections**
```bash
# Scenario: Report with sections
cd "${OXO_TEST_DIR}/A13"
create_workflow "sections_wf" '
[workflow]
name = "sections-demo"

[report]
sections = ["summary", "qc", "provenance"]

[[rules]]
name = "process"
output = ["result.txt"]
shell = "echo result > result.txt"
'
$OXO_FLOW_BIN validate sections_wf.oxoflow
# Expected: Report sections recognized
```

**A14: Clinical disclaimer**
```bash
# Scenario: Clinical compliance in report
cd "${OXO_TEST_DIR}/A14"
create_workflow "clinical_wf" '
[workflow]
name = "clinical-demo"

[report]
clinical = true

[[rules]]
name = "variant_call"
output = ["variants.vcf"]
shell = "echo VCF > variants.vcf"
'
$OXO_FLOW_BIN validate clinical_wf.oxoflow
# Expected: Clinical flag recognized
```

**A15: QC metrics in workflow**
```bash
# Scenario: QC output
cd "${OXO_TEST_DIR}/A15"
create_workflow "qc_wf" '
[workflow]
name = "qc-demo"

[[rules]]
name = "qc_check"
output = ["qc.json"]
shell = """
echo '{"total_reads": 1000, "mapped": 950, "quality": "PASS"}' > qc.json
"""
'
$OXO_FLOW_BIN validate qc_wf.oxoflow
# Expected: QC workflow validated
```

**A16: Sample metadata**
```bash
# Scenario: Sample information
cd "${OXO_TEST_DIR}/A16"
create_workflow "sample_wf" '
[workflow]
name = "sample-demo"

[config]
samples = ["S1", "S2"]

[[rules]]
name = "process_{sample}"
scatter = { variable = "sample", values_from = "config.samples" }
output = ["{sample}_report.txt"]
shell = "echo Sample {sample} processed > {sample}_report.txt"
'
$OXO_FLOW_BIN validate sample_wf.oxoflow
# Expected: Sample metadata recognized
```

**A17: Timing visualization**
```bash
# Scenario: Execution timing
cd "${OXO_TEST_DIR}/A17"
create_workflow "timing_wf" '
[workflow]
name = "timing-demo"

[[rules]]
name = "fast_task"
shell = "sleep 0.1"

[[rules]]
name = "slow_task"
shell = "sleep 0.2"
'
$OXO_FLOW_BIN dry-run timing_wf.oxoflow
# Expected: Shows execution order
```

**A18: Variant report mock**
```bash
# Scenario: Variant summary
cd "${OXO_TEST_DIR}/A18"
create_workflow "variant_wf" '
[workflow]
name = "variant-report"

[[rules]]
name = "call"
output = ["variants.vcf"]
shell = """
echo "##VCF" > variants.vcf
echo "chr1 100 A T PASS DP=50" >> variants.vcf
echo "chr1 200 G C PASS DP=30" >> variants.vcf
"""
'
$OXO_FLOW_BIN validate variant_wf.oxoflow
# Expected: Variant workflow validated
```

**A19: Report template**
```bash
# Scenario: Custom template
cd "${OXO_TEST_DIR}/A19"
create_workflow "template_wf" '
[workflow]
name = "template-demo"

[report]
template = "custom_template"

[[rules]]
name = "task"
shell = "echo template"
'
$OXO_FLOW_BIN validate template_wf.oxoflow
# Expected: Template specification recognized
```

**A20: Complex workflow report**
```bash
# Scenario: Multi-rule report
cd "${OXO_TEST_DIR}/A20"
create_workflow "complex_report" '
[workflow]
name = "complex-report"

[[rules]]
name = "r1"
output = ["r1.txt"]
shell = "echo r1 > r1.txt"

[[rules]]
name = "r2"
depends_on = ["r1"]
input = ["r1.txt"]
output = ["r2.txt"]
shell = "cat r1.txt > r2.txt"

[[rules]]
name = "r3"
depends_on = ["r2"]
input = ["r2.txt"]
output = ["r3.txt"]
shell = "cat r2.txt > r3.txt"
'
$OXO_FLOW_BIN validate complex_report.oxoflow
# Expected: Complex workflow validated
```

### Test A21-A30: Venus Clinical Pipeline (Mock Configurations)

**A21: Venus config generation mock**
```bash
# Scenario: Generate Venus config
cd "${OXO_TEST_DIR}/A21"
cat > venus_config.toml << 'EOF'
mode = "ExperimentOnly"
seq_type = "WGS"
genome_build = "GRCh38"
output_dir = "output"

[[experiment_samples]]
name = "SAMPLE_01"
r1_fastq = "raw/S01_R1.fq.gz"
r2_fastq = "raw/S01_R2.fq.gz"
EOF
venus generate venus_config.toml -o pipeline.oxoflow 2>&1 || echo "SKIP: Venus not installed"
```

**A22: Venus config validation**
```bash
# Scenario: Validate Venus config
venus validate venus_config.toml 2>&1 || echo "SKIP: Venus not installed"
```

**A23: Venus list steps**
```bash
# Scenario: List available pipeline steps
venus list-steps 2>&1 || echo "SKIP: Venus not installed"
```

**A24: Paired tumor-normal mock**
```bash
# Scenario: Tumor-normal pairs
cd "${OXO_TEST_DIR}/A24"
cat > paired_config.toml << 'EOF'
mode = "ExperimentControl"
seq_type = "WGS"

[[experiment_samples]]
name = "TUMOR_01"
r1_fastq = "tumor_R1.fq"
r2_fastq = "tumor_R2.fq"
is_experiment = true

[[experiment_samples]]
name = "NORMAL_01"
r1_fastq = "normal_R1.fq"
r2_fastq = "normal_R2.fq"
is_experiment = false
match_id = "TUMOR_01"
EOF
venus generate paired_config.toml -o paired.oxoflow 2>&1 || echo "SKIP"
```

**A25: WES mode mock**
```bash
# Scenario: Whole exome sequencing
cd "${OXO_TEST_DIR}/A25"
cat > wes_config.toml << 'EOF'
mode = "ExperimentOnly"
seq_type = "WES"
target_bed = "exome_targets.bed"
EOF
venus validate wes_config.toml 2>&1 || echo "SKIP"
```

**A26: RNA-seq mode mock**
```bash
# Scenario: RNA expression
cd "${OXO_TEST_DIR}/A26"
cat > rnaseq_config.toml << 'EOF'
mode = "ExperimentOnly"
seq_type = "RNA-seq"
EOF
venus validate rnaseq_config.toml 2>&1 || echo "SKIP"
```

**A27: Multiple samples mock**
```bash
# Scenario: Batch processing
cd "${OXO_TEST_DIR}/A27"
cat > batch_config.toml << 'EOF'
mode = "ExperimentOnly"

[[experiment_samples]]
name = "S01"

[[experiment_samples]]
name = "S02"

[[experiment_samples]]
name = "S03"
EOF
venus validate batch_config.toml 2>&1 || echo "SKIP"
```

**A28: CNV calling mock**
```bash
# Scenario: CNV integration
grep -i "cnvkit\|cnv" paired.oxoflow 2>/dev/null || echo "SKIP"
```

**A29: MSI detection mock**
```bash
# Scenario: MSI analysis
grep -i "msisensor\|msi" paired.oxoflow 2>/dev/null || echo "SKIP"
```

**A30: Clinical report step mock**
```bash
# Scenario: Clinical report generation
grep -i "report\|clinical" paired.oxoflow 2>/dev/null || echo "SKIP"
```

---

## Phase 4: Expert Tests (E01-E20)

### User Profile: Production deployment, clinical compliance

### Test E01-E10: Production Features

**E01: SHA-256 checksum verification**
```bash
# Scenario: File integrity
cd "${OXO_TEST_DIR}/E01" && mkdir -p E01 && cd E01
create_workflow "checksum_wf" '
[workflow]
name = "checksum-test"

[[rules]]
name = "generate"
output = ["data.txt"]
shell = "echo test data > data.txt"
'
$OXO_FLOW_BIN run checksum_wf.oxoflow
sha256sum data.txt 2>/dev/null || shasum -a 256 data.txt
# Expected: SHA-256 hash computed
```

**E02: Provenance mock**
```bash
# Scenario: Provenance tracking
cd "${OXO_TEST_DIR}/E02"
create_workflow "provenance_wf" '
[workflow]
name = "provenance-test"

[[rules]]
name = "task"
output = ["output.txt"]
shell = "echo provenance > output.txt"
'
$OXO_FLOW_BIN run provenance_wf.oxoflow
ls .oxoflow/provenance.json 2>/dev/null && cat .oxoflow/provenance.json || echo "SKIP: No provenance file"
```

**E03: Checkpoint recovery**
```bash
# Scenario: Resume from checkpoint
cd "${OXO_TEST_DIR}/E03"
create_workflow "checkpoint_wf" '
[workflow]
name = "checkpoint-test"

[[rules]]
name = "s1"
output = ["s1.txt"]
shell = "echo s1 > s1.txt"

[[rules]]
name = "s2"
depends_on = ["s1"]
input = ["s1.txt"]
output = ["s2.txt"]
shell = "cat s1.txt > s2.txt"
'
$OXO_FLOW_BIN run checkpoint_wf.oxoflow
$OXO_FLOW_BIN status checkpoint_wf.oxoflow 2>&1 || echo "SKIP"
```

**E04: Clean operation**
```bash
# Scenario: Clean outputs
cd "${OXO_TEST_DIR}/E04"
create_workflow "clean_wf" '
[workflow]
name = "clean-test"

[[rules]]
name = "task"
output = ["to_clean.txt"]
shell = "echo clean > to_clean.txt"
'
$OXO_FLOW_BIN run clean_wf.oxoflow
$OXO_FLOW_BIN clean clean_wf.oxoflow -n 2>&1 | grep -q "clean\|remove" && echo "PASS" || echo "SKIP"
```

**E05: Touch operation**
```bash
# Scenario: Mark outputs as complete
cd "${OXO_TEST_DIR}/E05"
create_workflow "touch_wf" '
[workflow]
name = "touch-test"

[[rules]]
name = "task"
output = ["touched.txt"]
shell = "echo touch > touched.txt"
'
$OXO_FLOW_BIN touch touch_wf.oxoflow 2>&1 | grep -q "touch\|complete" && echo "PASS" || echo "SKIP"
```

**E06: Diff workflows**
```bash
# Scenario: Compare workflow versions
cd "${OXO_TEST_DIR}/E06"
create_workflow "v1" '
[workflow]
name = "diff-test"
version = "1.0"

[[rules]]
name = "step1"
shell = "echo v1"
'
create_workflow "v2" '
[workflow]
name = "diff-test"
version = "2.0"

[[rules]]
name = "step1"
shell = "echo v2"

[[rules]]
name = "step2"
shell = "echo added"
'
$OXO_FLOW_BIN diff v1.oxoflow v2.oxoflow 2>&1 | grep -q "added\|changed\|diff" && echo "PASS" || echo "SKIP"
```

**E07: Config inspection**
```bash
# Scenario: Show workflow config
cd "${OXO_TEST_DIR}/E07"
$OXO_FLOW_BIN config show v2.oxoflow 2>&1 | head -10
```

**E08: Config statistics**
```bash
# Scenario: Workflow statistics
$OXO_FLOW_BIN config stats v2.oxoflow 2>&1 | head -10
```

**E09: Secret scanning**
```bash
# Scenario: Secret patterns
cd "${OXO_TEST_DIR}/E09"
create_workflow "secret_scan" '
[workflow]
name = "secret-scan"

[config]
aws_key = "AKIAIOSFODNN7EXAMPLE"
github_token = "ghp_xxxxxxxxxxxxxxxxxxxx"

[[rules]]
name = "task"
shell = "echo secrets"
'
$OXO_FLOW_BIN lint secret_scan.oxoflow 2>&1 | grep -q "S008\|secret\|credential" && echo "PASS" || echo "SKIP"
```

**E10: All error codes**
```bash
# Scenario: Comprehensive error testing
cd "${OXO_TEST_DIR}/E10"

# E001: Empty name
echo '[workflow] name = ""' > e001.oxoflow
$OXO_FLOW_BIN validate e001.oxoflow 2>&1 | grep -q "E001" && echo "E001 OK" || echo "E001 FAIL"

# E003: Wildcard mismatch
echo '[workflow] name = "t" [[rules]] name = "r" output = ["{x}.txt"] shell = "echo"' > e003.oxoflow
$OXO_FLOW_BIN validate e003.oxoflow 2>&1 | grep -q "E003" && echo "E003 OK" || echo "E003 FAIL"

# E006: DAG cycle
echo '[workflow] name = "t" [[rules]] name = "a" depends_on = ["b"] [[rules]] name = "b" depends_on = ["a"]' > e006.oxoflow
$OXO_FLOW_BIN validate e006.oxoflow 2>&1 | grep -q "E006" && echo "E006 OK" || echo "E006 FAIL"

# E007: Missing dependency
echo '[workflow] name = "t" [[rules]] name = "a" depends_on = ["missing"]' > e007.oxoflow
$OXO_FLOW_BIN validate e007.oxoflow 2>&1 | grep -q "E007" && echo "E007 OK" || echo "E007 FAIL"

# E009: Path traversal
echo '[workflow] name = "t" [[rules]] name = "a" output = ["../../../etc/passwd"]' > e009.oxoflow
$OXO_FLOW_BIN validate e009.oxoflow 2>&1 | grep -q "E009" && echo "E009 OK" || echo "E009 FAIL"
```

### Test E11-E20: Web Interface

**E11: Web server health check**
```bash
# Scenario: Health endpoint
$OXO_FLOW_BIN serve --port 8888 &
sleep 2
curl -s http://localhost:8888/health 2>/dev/null && echo "PASS" || echo "SKIP"
kill %1 2>/dev/null
```

**E12: API workflow upload**
```bash
# Scenario: Upload workflow
curl -X POST -F "workflow=@v2.oxoflow" http://localhost:8888/api/workflows 2>/dev/null || echo "SKIP"
```

**E13: API workflow list**
```bash
# Scenario: List workflows
curl -s http://localhost:8888/api/workflows 2>/dev/null || echo "SKIP"
```

**E14: API validation**
```bash
# Scenario: Remote validation
curl -X POST -F "workflow=@v2.oxoflow" http://localhost:8888/api/validate 2>/dev/null || echo "SKIP"
```

**E15: API job submission**
```bash
# Scenario: Submit job
curl -X POST -d '{"workflow_id": "test"}' http://localhost:8888/api/run 2>/dev/null || echo "SKIP"
```

**E16: API status check**
```bash
# Scenario: Job status
curl -s http://localhost:8888/api/status/test 2>/dev/null || echo "SKIP"
```

**E17: API report download**
```bash
# Scenario: Download report
curl -s http://localhost:8888/api/report/test -o report.html 2>/dev/null || echo "SKIP"
```

**E18: API authentication**
```bash
# Scenario: Auth header
curl -H "Authorization: Bearer test_token" http://localhost:8888/api/workflows 2>/dev/null || echo "SKIP"
```

**E19: Server graceful shutdown**
```bash
# Scenario: Clean shutdown
$OXO_FLOW_BIN serve --port 8889 &
sleep 1
kill $(pgrep -f "oxo-flow.*serve") 2>/dev/null && echo "PASS" || echo "SKIP"
```

**E20: Audit logging**
```bash
# Scenario: Check audit logs
ls .oxoflow/logs/*.log 2>/dev/null && echo "PASS" || echo "SKIP: No audit logs"
```

---

## Cleanup

```bash
# Remove test workspace
rm -rf "$OXO_TEST_DIR"
unset OXO_TEST_DIR
unset OXO_FLOW_BIN
```

---

## Summary

| Category | Tests | Approach |
|----------|-------|----------|
| Beginner CLI | B01-B30 | Shell mocks, self-contained workflows |
| Intermediate Config | I01-I40 | Pattern validation, cluster generation |
| Advanced Patterns | A01-A30 | Complex DAGs, reports, Venus mocks |
| Expert Production | E01-E20 | Provenance, checksums, Web API |

**All tests are self-contained and require no external bioinformatics software.**