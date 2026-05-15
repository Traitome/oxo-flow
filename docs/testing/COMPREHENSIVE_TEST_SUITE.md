# oxo-flow Comprehensive Test Suite

> **Total Test Instances**: 120+ real scenario tests
> **User Levels**: Beginner (30), Intermediate (40), Advanced (30), Expert (20)
> **Testing Approach**: Random search style - each test has unique scenario parameters

## Test Categories

### A. CLI Commands (50+ tests)
- Basic commands: help, version, init, validate
- Workflow commands: run, dry-run, graph, lint, format
- Execution commands: debug, clean, touch, status
- Container commands: package, export, serve
- Cluster commands: cluster submit/status/cancel
- Profile commands: profile list/show/current

### B. Web Interface (15 tests)
- Server startup/shutdown
- API endpoints
- Authentication
- Workflow management

### C. Venus Clinical Pipeline (15 tests)
- Config generation
- Pipeline validation
- Mode variations (ExperimentOnly, ControlOnly, ExperimentControl)
- Sample types (WGS, WES, RNA-seq)

### D. Gallery Workflows (25 tests)
- All 9 gallery workflows tested with multiple scenarios
- Each workflow with different parameters

### E. Bioinformatics Patterns (20+ tests)
- Serial execution
- Parallel execution
- Scatter-gather patterns
- Checkpoint workflows
- Conditional execution
- Wildcard expansion

---

## Test Environment Setup

```bash
# Isolated test workspace
export OXO_TEST_WORKSPACE="/tmp/oxo-flow-test-workspace"
mkdir -p "$OXO_TEST_WORKSPACE"
cd "$OXO_TEST_WORKSPACE"

# Conda environment isolation
conda create -n oxo-test-env python=3.10 -y
conda activate oxo-test-env

# Build oxo-flow binaries
cd /Users/wsx/Documents/GitHub/oxo-flow
cargo build --workspace --release
```

---

## Phase 1: Beginner Tests (30 instances)

### User Profile: New to bioinformatics pipelines, learning basics

### Test B01-B10: Basic CLI Discovery

**B01: First time user - help command**
```bash
# Scenario: New user discovering oxo-flow capabilities
oxo-flow --help
# Expected: Shows all 21 commands with descriptions
# Coverage: CLI documentation, command overview
```

**B02: Version check before installation**
```bash
# Scenario: User checking version compatibility
oxo-flow --version
# Expected: Shows version 0.3.1
# Coverage: Version display, build metadata
```

**B03: Shell completion setup**
```bash
# Scenario: User enabling tab completion in bash
oxo-flow completions bash > ~/.oxo-flow-completion.bash
source ~/.oxo-flow-completion.bash
# Expected: Tab completion works for all commands
# Coverage: Shell integration, UX enhancement
```

**B04: Initialize first project**
```bash
# Scenario: User creating their first pipeline project
cd "$OXO_TEST_WORKSPACE"
oxo-flow init my-first-pipeline -d ./test-B04
# Expected: Creates project structure with .oxoflow, envs/, scripts/
# Coverage: Project scaffolding, default templates
```

**B05: Validate simple workflow**
```bash
# Scenario: User checking if workflow is valid before running
oxo-flow validate examples/gallery/01_hello_world.oxoflow
# Expected: Shows success with rule count
# Coverage: Basic validation, E001-E009 error detection
```

**B06: Dry-run for execution preview**
```bash
# Scenario: User previewing what will happen before running
oxo-flow dry-run examples/gallery/01_hello_world.oxoflow
# Expected: Shows execution order, no actual commands run
# Coverage: DAG visualization, execution planning
```

**B07: Graph visualization**
```bash
# Scenario: User understanding workflow dependencies visually
oxo-flow graph -f dot examples/gallery/02_file_pipeline.oxoflow > workflow.dot
# Expected: Valid DOT format for Graphviz
# Coverage: Dependency graph, visual output
```

**B08: List available environments**
```bash
# Scenario: User checking supported environment backends
oxo-flow env list
# Expected: Lists conda, pixi, docker, singularity, venv
# Coverage: Environment system, backend discovery
```

**B09: Format a workflow file**
```bash
# Scenario: User cleaning up workflow formatting
oxo-flow format examples/gallery/01_hello_world.oxoflow
# Expected: Canonical TOML format output
# Coverage: TOML formatting, canonical representation
```

**B10: Lint for best practices**
```bash
# Scenario: User checking workflow quality
oxo-flow lint examples/gallery/01_hello_world.oxoflow
# Expected: W001-W016 warnings if any, clean output if good
# Coverage: Linting, best practice validation
```

### Test B11-B20: Simple Workflow Execution

**B11: Hello world execution**
```bash
# Scenario: Running the simplest possible workflow
cd "$OXO_TEST_WORKSPACE"
mkdir test-B11 && cd test-B11
oxo-flow run examples/gallery/01_hello_world.oxoflow
# Expected: 1 rule executes, success message
# Coverage: Basic execution, shell command handling
```

**B12: File pipeline with dependencies**
```bash
# Scenario: Understanding rule dependencies through execution
cd "$OXO_TEST_WORKSPACE/test-B12"
oxo-flow run examples/gallery/02_file_pipeline.oxoflow
# Expected: 3 rules execute in order (generate_data, process_data, summarize)
# Coverage: DAG execution, input/output chaining
```

**B13: Parallel samples processing**
```bash
# Scenario: Running parallel rules for multiple samples
oxo-flow run examples/gallery/03_parallel_samples.oxoflow -j 4
# Expected: Multiple rules run concurrently
# Coverage: Parallel execution, job limiting
```

**B14: Scatter-gather pattern**
```bash
# Scenario: Chromosome-based scatter-gather processing
oxo-flow run examples/gallery/04_scatter_gather.oxoflow -j 8
# Expected: Scatter rules execute, then gather
# Coverage: Scatter-gather, parallel reduction
```

**B15: Conda environment workflow**
```bash
# Scenario: Using conda environments per rule
oxo-flow run examples/gallery/05_conda_environments.oxoflow --skip-env-setup
# Expected: Rules execute with environment awareness
# Coverage: Conda integration, environment specs
```

**B16: RNA-seq quantification workflow**
```bash
# Scenario: Bioinformatics RNA-seq pipeline
oxo-flow dry-run examples/gallery/06_rnaseq_quantification.oxoflow
# Expected: Shows fastp_trim, salmon_quant, multiqc order
# Coverage: Real bioinformatics workflow, multi-tool integration
```

**B17: WGS germline calling workflow**
```bash
# Scenario: Whole genome sequencing variant calling
oxo-flow validate examples/gallery/07_wgs_germline.oxoflow
# Expected: 10 rules validated successfully
# Coverage: Complex pipeline validation, error detection
```

**B18: Multiomics integration workflow**
```bash
# Scenario: Multi-modal data integration
oxo-flow graph examples/gallery/08_multiomics_integration.oxoflow
# Expected: Shows multi-branch DAG
# Coverage: Complex DAG structure, multi-omics
```

**B19: Single-cell RNA-seq workflow**
```bash
# Scenario: Single-cell analysis pipeline
oxo-flow lint examples/gallery/09_single_cell_rnaseq.oxoflow
# Expected: Linting passes for scRNA-seq workflow
# Coverage: Specialized workflow, cell-level processing
```

**B20: Simple variant calling workflow**
```bash
# Scenario: Basic variant calling from BAM
oxo-flow validate examples/simple_variant_calling.oxoflow
# Expected: Validation passes with proper rule structure
# Coverage: Clinical workflow structure, variant calling
```

### Test B21-B30: Error Handling and Recovery

**B21: Invalid workflow validation**
```bash
# Scenario: User made a syntax error in workflow
cd "$OXO_TEST_WORKSPACE"
echo 'invalid toml {{' > bad.oxoflow
oxo-flow validate bad.oxoflow
# Expected: Error with S001 (invalid TOML)
# Coverage: Error handling, user guidance
```

**B22: Missing workflow file**
```bash
# Scenario: User specifies wrong file path
oxo-flow validate nonexistent.oxoflow
# Expected: Clear error message about missing file
# Coverage: File handling, error messaging
```

**B23: Missing workflow name (E001)**
```bash
# Scenario: User forgot to add workflow name
cd "$OXO_TEST_WORKSPACE"
cat > no-name.oxoflow << 'EOF'
[workflow]
name = ""
version = "1.0"
[[rules]]
name = "test"
shell = "echo hi"
EOF
oxo-flow validate no-name.oxoflow
# Expected: E001 error detected
# Coverage: Validation error codes, E001
```

**B24: Invalid memory format (E004)**
```bash
# Scenario: User uses wrong memory format
cd "$OXO_TEST_WORKSPACE"
cat > bad-memory.oxoflow << 'EOF'
[workflow]
name = "test"
[[rules]]
name = "step1"
memory = "invalid"
shell = "echo hi"
EOF
oxo-flow lint bad-memory.oxoflow
# Expected: E004 error for invalid memory
# Coverage: Memory validation, E004
```

**B25: Path traversal detection (E009)**
```bash
# Scenario: User accidentally uses relative path escaping
cd "$OXO_TEST_WORKSPACE"
cat > traversal.oxoflow << 'EOF'
[workflow]
name = "test"
[[rules]]
name = "step1"
output = ["../../../etc/passwd"]
shell = "echo hi"
EOF
oxo-flow validate traversal.oxoflow
# Expected: E009 error for path traversal
# Coverage: Security validation, path safety
```

**B26: Secret detection in workflow**
```bash
# Scenario: User accidentally includes API key in workflow
cd "$OXO_TEST_WORKSPACE"
cat > secrets.oxoflow << 'EOF'
[workflow]
name = "test"
[config]
api_key = "sk-test123456789"
[[rules]]
name = "step1"
shell = "echo hi"
EOF
oxo-flow lint secrets.oxoflow
# Expected: S008 warning for secret pattern
# Coverage: Security scanning, secret detection
```

**B27: DAG cycle detection (E006)**
```bash
# Scenario: User creates circular dependency
cd "$OXO_TEST_WORKSPACE"
cat > cycle.oxoflow << 'EOF'
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
EOF
oxo-flow validate cycle.oxoflow
# Expected: E006 error for DAG cycle
# Coverage: DAG validation, cycle detection
```

**B28: Undefined config reference (E005)**
```bash
# Scenario: User references undefined config variable
cd "$OXO_TEST_WORKSPACE"
cat > undef-ref.oxoflow << 'EOF'
[workflow]
name = "test"
[[rules]]
name = "step1"
shell = "echo {config.undefined_var}"
EOF
oxo-flow validate undef-ref.oxoflow
# Expected: E005 error for undefined reference
# Coverage: Config validation, variable reference
```

**B29: Depends on non-existent rule (E007)**
```bash
# Scenario: User references wrong rule name
cd "$OXO_TEST_WORKSPACE"
cat > bad-dep.oxoflow << 'EOF'
[workflow]
name = "test"
[[rules]]
name = "step1"
depends_on = ["nonexistent"]
shell = "echo hi"
EOF
oxo-flow validate bad-dep.oxoflow
# Expected: E007 error for missing dependency
# Coverage: Dependency validation, E007
```

**B30: Wildcard mismatch (E003)**
```bash
# Scenario: User has output wildcard not in input
cd "$OXO_TEST_WORKSPACE"
cat > wc-mismatch.oxoflow << 'EOF'
[workflow]
name = "test"
[[rules]]
name = "step1"
input = ["data.txt"]
output = ["{sample}.txt"]
shell = "echo hi"
EOF
oxo-flow validate wc-mismatch.oxoflow
# Expected: E003 error for wildcard mismatch
# Coverage: Wildcard validation, E003
```

---

## Phase 2: Intermediate Tests (40 instances)

### User Profile: Comfortable with basics, exploring advanced features

### Test I01-I10: Workflow Configuration

**I01: Config section usage**
```bash
# Scenario: User defining reusable configuration variables
cd "$OXO_TEST_WORKSPACE/test-I01"
cat > config-test.oxoflow << 'EOF'
[workflow]
name = "config-demo"
version = "1.0"

[config]
reference = "/data/hg38.fa"
threads = 8
samples = ["S1", "S2", "S3"]

[[rules]]
name = "align_{config.samples[0]}"
shell = "bwa mem {config.reference} {config.samples[0]}.fq"
threads = {config.threads}
EOF
oxo-flow validate config-test.oxoflow
# Expected: Config variables recognized
# Coverage: Config system, variable substitution
```

**I02: Defaults section**
```bash
# Scenario: User setting default resource values
cd "$OXO_TEST_WORKSPACE/test-I02"
cat > defaults-test.oxoflow << 'EOF'
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
shell = "echo step2"
EOF
oxo-flow debug defaults-test.oxoflow
# Expected: step1 uses 4 threads, step2 uses 16
# Coverage: Defaults inheritance, override behavior
```

**I03: Resource specifications**
```bash
# Scenario: User specifying per-rule resources
cd "$OXO_TEST_WORKSPACE/test-I03"
cat > resources-test.oxoflow << 'EOF'
[workflow]
name = "resources-demo"

[[rules]]
name = "heavy_task"
threads = 32
memory = "64G"
shell = "echo heavy"

[[rules]]
name = "light_task"
threads = 1
memory = "1G"
shell = "echo light"
EOF
oxo-flow lint resources-test.oxoflow
# Expected: No W005 warning for high threads with memory
# Coverage: Resource validation, W005
```

**I04: GPU resource specification**
```bash
# Scenario: User running GPU-accelerated analysis
cd "$OXO_TEST_WORKSPACE/test-I04"
cat > gpu-test.oxoflow << 'EOF'
[workflow]
name = "gpu-demo"

[[rules]]
name = "deep_learning"
threads = 8
memory = "32G"

[rules.resources]
gpu = 2

shell = "python train.py"
EOF
oxo-flow validate gpu-test.oxoflow
# Expected: GPU specification recognized
# Coverage: GPU resources, hardware acceleration
```

**I05: Time limit specification**
```bash
# Scenario: User setting job time limits
cd "$OXO_TEST_WORKSPACE/test-I05"
cat > time-test.oxoflow << 'EOF'
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
EOF
oxo-flow validate time-test.oxoflow
# Expected: Time limits parsed correctly
# Coverage: Time limit validation, duration parsing
```

**I06: Environment modules**
```bash
# Scenario: User using HPC module system
cd "$OXO_TEST_WORKSPACE/test-I06"
cat > modules-test.oxoflow << 'EOF'
[workflow]
name = "modules-demo"

[[rules]]
name = "gatk_task"
shell = "gatk HaplotypeCaller"

[rules.environment]
modules = ["java/11", "gatk/4.2"]
EOF
oxo-flow validate modules-test.oxoflow
# Expected: Module specification recognized
# Coverage: HPC modules, environment system
```

**I07: Conda environment specification**
```bash
# Scenario: User specifying conda YAML file
cd "$OXO_TEST_WORKSPACE/test-I07"
mkdir -p envs
cat > envs/align.yaml << 'EOF'
name: align-env
channels:
  - bioconda
  - conda-forge
dependencies:
  - bwa=0.7.17
  - samtools=1.15
EOF
cat > conda-test.oxoflow << 'EOF'
[workflow]
name = "conda-demo"

[[rules]]
name = "align"
input = ["reads.fq"]
output = ["aligned.bam"]
shell = "bwa mem ref.fa {input}"

[rules.environment]
conda = "envs/align.yaml"
EOF
oxo-flow validate conda-test.oxoflow
# Expected: Conda environment recognized
# Coverage: Conda integration, environment YAML
```

**I08: Docker container specification**
```bash
# Scenario: User using Docker for reproducibility
cd "$OXO_TEST_WORKSPACE/test-I08"
cat > docker-test.oxoflow << 'EOF'
[workflow]
name = "docker-demo"

[[rules]]
name = "container_task"
shell = "echo in-docker"

[rules.environment]
docker = "biocontainers/bwa:0.7.17"
EOF
oxo-flow validate docker-test.oxoflow
# Expected: Docker container recognized
# Coverage: Docker integration, container runtime
```

**I09: Singularity container specification**
```bash
# Scenario: User on HPC system using Singularity
cd "$OXO_TEST_WORKSPACE/test-I09"
cat > singularity-test.oxoflow << 'EOF'
[workflow]
name = "singularity-demo"

[[rules]]
name = "container_task"
shell = "echo in-singularity"

[rules.environment]
singularity = "docker://biocontainers/bwa:0.7.17"
EOF
oxo-flow validate singularity-test.oxoflow
# Expected: Singularity recognized
# Coverage: Singularity integration, HPC containers
```

**I10: Pixi environment specification**
```bash
# Scenario: User using Pixi for fast environment management
cd "$OXO_TEST_WORKSPACE/test-I10"
cat > pixi-test.oxoflow << 'EOF'
[workflow]
name = "pixi-demo"

[[rules]]
name = "pixi_task"
shell = "echo pixi"

[rules.environment]
pixi = "pixi.toml"
EOF
oxo-flow validate pixi-test.oxoflow
# Expected: Pixi recognized
# Coverage: Pixi integration, modern environment system
```

### Test I11-I20: Advanced Workflow Patterns

**I11: Paired experiment-control workflow**
```bash
# Scenario: Somatic variant calling with tumor-normal pairs
oxo-flow validate examples/paired_experiment_control.oxoflow
# Expected: Experiment-control pairs recognized
# Coverage: Paired analysis, experiment_control expansion
```

**I12: Sample cohort analysis**
```bash
# Scenario: Multi-sample cohort processing
oxo-flow validate examples/cohort_analysis.oxoflow
# Expected: Cohort samples expanded correctly
# Coverage: Cohort processing, sample groups
```

**I13: Conditional execution**
```bash
# Scenario: Rules that execute based on conditions
oxo-flow validate examples/conditional_workflow.oxoflow
# Expected: Conditional rules parsed correctly
# Coverage: Conditional execution, when clauses
```

**I14: Checkpoint workflow**
```bash
# Scenario: Workflow with checkpoint rules
cd "$OXO_TEST_WORKSPACE/test-I14"
cat > checkpoint-test.oxoflow << 'EOF'
[workflow]
name = "checkpoint-demo"

[[rules]]
name = "discover"
checkpoint = true
output = ["samples.txt"]
shell = "echo sample1 sample2 > samples.txt"

[[rules]]
name = "process"
input = ["samples.txt"]
output = ["results.txt"]
shell = "cat {input} > {output}"
EOF
oxo-flow validate checkpoint-test.oxoflow
# Expected: Checkpoint recognized, no W010 error
# Coverage: Checkpoint system, dynamic DAG rebuilding
```

**I15: Wildcard expansion**
```bash
# Scenario: Expanding wildcards for sample processing
cd "$OXO_TEST_WORKSPACE/test-I15"
cat > wildcard-test.oxoflow << 'EOF'
[workflow]
name = "wildcard-demo"

[config]
samples = ["S1", "S2", "S3"]

[[rules]]
name = "process_{sample}"
input = ["{sample}.fq"]
output = ["{sample}.bam"]
scatter = { variable = "sample", values_from = "config.samples" }
shell = "echo process {sample}"
EOF
oxo-flow validate wildcard-test.oxoflow
# Expected: Scatter configuration recognized
# Coverage: Wildcard expansion, scatter pattern
```

**I16: Depends_on specification**
```bash
# Scenario: Explicit dependency declaration
cd "$OXO_TEST_WORKSPACE/test-I16"
cat > deps-test.oxoflow << 'EOF'
[workflow]
name = "deps-demo"

[[rules]]
name = "align"
output = ["aligned.bam"]
shell = "echo align"

[[rules]]
name = "call"
depends_on = ["align"]
input = ["aligned.bam"]
output = ["variants.vcf"]
shell = "echo call"

[[rules]]
name = "annotate"
depends_on = ["call"]
input = ["variants.vcf"]
output = ["annotated.vcf"]
shell = "echo annotate"
EOF
oxo-flow graph -f dot deps-test.oxoflow
# Expected: Shows 3 connected nodes in order
# Coverage: Dependency chains, DAG structure
```

**I17: Rule extension (extends)**
```bash
# Scenario: Extending a base rule configuration
cd "$OXO_TEST_WORKSPACE/test-I17"
cat > extends-test.oxoflow << 'EOF'
[workflow]
name = "extends-demo"

[[rules]]
name = "base_bwa"
threads = 8
memory = "16G"
shell = "bwa mem ref.fa"

[[rules]]
name = "bwa_sample1"
extends = "base_bwa"
input = ["sample1.fq"]
output = ["sample1.bam"]
EOF
oxo-flow validate extends-test.oxoflow
# Expected: Extended rule inherits base properties
# Coverage: Rule inheritance, extends system
```

**I18: Multiple outputs**
```bash
# Scenario: Rule producing multiple output files
cd "$OXO_TEST_WORKSPACE/test-I18"
cat > multi-out.oxoflow << 'EOF'
[workflow]
name = "multi-output"

[[rules]]
name = "split_output"
output = ["part1.txt", "part2.txt", "part3.txt"]
shell = "echo part1 > part1.txt; echo part2 > part2.txt; echo part3 > part3.txt"
EOF
oxo-flow validate multi-out.oxoflow
# Expected: Multiple outputs recognized
# Coverage: Multi-output rules, file tracking
```

**I19: Input function**
```bash
# Scenario: Dynamic input resolution
cd "$OXO_TEST_WORKSPACE/test-I19"
cat > input-func.oxoflow << 'EOF'
[workflow]
name = "input-func-demo"

[[rules]]
name = "dynamic_input"
input_function = "get_input_files"
output = ["processed.txt"]
shell = "cat {input} > {output}"
EOF
oxo-flow validate input-func.oxoflow
# Expected: Input function recognized
# Coverage: Dynamic inputs, function references
```

**I20: Protected and temp outputs**
```bash
# Scenario: Marking outputs for protection or cleanup
cd "$OXO_TEST_WORKSPACE/test-I20"
cat > output-flags.oxoflow << 'EOF'
[workflow]
name = "output-flags"

[[rules]]
name = "pipeline"
input = ["input.bam"]
output = ["final.vcf"]
temp_output = ["intermediate.bam", "unsorted.bam"]
protected_output = ["final.vcf"]
shell = "echo pipeline"
EOF
oxo-flow validate output-flags.oxoflow
# Expected: Output flags recognized
# Coverage: Output lifecycle, cleanup system
```

### Test I21-I30: Cluster Integration

**I21: SLURM cluster submit**
```bash
# Scenario: Submitting workflow to SLURM scheduler
cd "$OXO_TEST_WORKSPACE/test-I21"
oxo-flow cluster submit examples/gallery/02_file_pipeline.oxoflow -b slurm -o ./slurm_scripts
# Expected: SLURM .sh scripts generated
# Coverage: SLURM integration, job script generation
```

**I22: PBS cluster submit**
```bash
# Scenario: Submitting workflow to PBS scheduler
cd "$OXO_TEST_WORKSPACE/test-I22"
oxo-flow cluster submit examples/gallery/02_file_pipeline.oxoflow -b pbs -o ./pbs_scripts
# Expected: PBS .sh scripts generated
# Coverage: PBS integration, job script generation
```

**I23: SGE cluster submit**
```bash
# Scenario: Submitting workflow to SGE scheduler
cd "$OXO_TEST_WORKSPACE/test-I23"
oxo-flow cluster submit examples/gallery/02_file_pipeline.oxoflow -b sge -o ./sge_scripts
# Expected: SGE .sh scripts generated
# Coverage: SGE integration, job script generation
```

**I24: LSF cluster submit**
```bash
# Scenario: Submitting workflow to LSF scheduler
cd "$OXO_TEST_WORKSPACE/test-I24"
oxo-flow cluster submit examples/gallery/02_file_pipeline.oxoflow -b lsf -o ./lsf_scripts
# Expected: LSF .sh scripts generated
# Coverage: LSF integration, job script generation
```

**I25: Cluster status check**
```bash
# Scenario: Checking job status on scheduler
oxo-flow cluster status -b slurm
# Expected: Shows squeue command output
# Coverage: Job monitoring, status command
```

**I26: Profile management**
```bash
# Scenario: Configuring execution profiles
oxo-flow profile list
oxo-flow profile show slurm
# Expected: Shows available profiles and configuration
# Coverage: Profile system, configuration management
```

**I27: GPU cluster script generation**
```bash
# Scenario: Generating GPU-aware cluster scripts
cd "$OXO_TEST_WORKSPACE/test-I27"
cat > gpu-workflow.oxoflow << 'EOF'
[workflow]
name = "gpu-cluster"

[[rules]]
name = "train"
shell = "python train.py"

[rules.resources]
gpu = 4
EOF
oxo-flow cluster submit gpu-workflow.oxoflow -b slurm -o ./gpu_scripts
grep -l "gres=gpu" ./gpu_scripts/*.sh
# Expected: Scripts contain GPU directives
# Coverage: GPU cluster integration, resource directives
```

**I28: Walltime cluster script**
```bash
# Scenario: Generating scripts with time limits
cd "$OXO_TEST_WORKSPACE/test-I28"
cat > time-workflow.oxoflow << 'EOF'
[workflow]
name = "time-cluster"

[[rules]]
name = "long_job"
shell = "echo long"

[rules.resources]
time_limit = "72h"
EOF
oxo-flow cluster submit time-workflow.oxoflow -b slurm -o ./time_scripts
grep "time=" ./time_scripts/*.sh
# Expected: Scripts contain walltime directives
# Coverage: Time limit integration, scheduler directives
```

**I29: Module loading in cluster scripts**
```bash
# Scenario: Generating scripts with module loading
cd "$OXO_TEST_WORKSPACE/test-I29"
cat > module-workflow.oxoflow << 'EOF'
[workflow]
name = "module-cluster"

[[rules]]
name = "gatk_task"
shell = "gatk HaplotypeCaller"

[rules.environment]
modules = ["java/11", "gatk/4.2"]
EOF
oxo-flow cluster submit module-workflow.oxoflow -b slurm -o ./module_scripts
grep "module load" ./module_scripts/*.sh
# Expected: Scripts contain module load commands
# Coverage: Module system, HPC integration
```

**I30: Multi-stage cluster workflow**
```bash
# Scenario: Complex workflow submitted to cluster
oxo-flow cluster submit examples/gallery/07_wgs_germline.oxoflow -b slurm -o ./wgs_scripts
ls ./wgs_scripts/*.sh | wc -l
# Expected: 10+ scripts for WGS pipeline
# Coverage: Complex pipeline cluster submission
```

### Test I31-I40: Container and Packaging

**I31: Dockerfile generation**
```bash
# Scenario: Creating Dockerfile for workflow
oxo-flow package examples/gallery/01_hello_world.oxoflow -f docker
# Expected: Valid Dockerfile with FROM, ENTRYPOINT
# Coverage: Container generation, Docker integration
```

**I32: Singularity definition generation**
```bash
# Scenario: Creating Singularity def file
oxo-flow package examples/gallery/01_hello_world.oxoflow -f singularity
# Expected: Valid def file with Bootstrap, %post
# Coverage: Singularity generation, HPC containers
```

**I33: Multi-stage Dockerfile**
```bash
# Scenario: Creating optimized multi-stage Dockerfile
cd "$OXO_TEST_WORKSPACE/test-I33"
cat > multi-stage.oxoflow << 'EOF'
[workflow]
name = "multi-stage-test"

[[rules]]
name = "build"
shell = "echo build"

[[rules]]
name = "runtime"
depends_on = ["build"]
shell = "echo runtime"
EOF
oxo-flow package multi-stage.oxoflow -f docker --multi-stage
# Expected: Dockerfile with FROM ... AS builder
# Coverage: Multi-stage builds, container optimization
```

**I34: Rootless container**
```bash
# Scenario: Creating security-conscious rootless container
oxo-flow package examples/gallery/01_hello_world.oxoflow -f docker
grep "USER oxoflow" output
# Expected: Dockerfile runs as non-root user
# Coverage: Security, rootless containers
```

**I35: GPU-aware container**
```bash
# Scenario: Creating container with GPU support
cd "$OXO_TEST_WORKSPACE/test-I35"
cat > gpu-container.oxoflow << 'EOF'
[workflow]
name = "gpu-container"

[[rules]]
name = "gpu_task"
shell = "python train.py"

[rules.resources]
gpu = 2
EOF
oxo-flow package gpu-container.oxoflow -f docker
# Expected: NVIDIA CUDA base image selected
# Coverage: GPU containers, CUDA integration
```

**I36: Export workflow to TOML**
```bash
# Scenario: Exporting workflow in canonical format
oxo-flow export examples/gallery/01_hello_world.oxoflow -f toml
# Expected: Canonical TOML output
# Coverage: Export system, canonical format
```

**I37: Conda in container**
```bash
# Scenario: Container with Conda environment
oxo-flow package examples/gallery/05_conda_environments.oxoflow -f docker
# Expected: Dockerfile installs Miniforge, creates conda envs
# Coverage: Conda in containers, environment replication
```

**I38: Pixi in container**
```bash
# Scenario: Container with Pixi environment
cd "$OXO_TEST_WORKSPACE/test-I38"
cat > pixi-container.oxoflow << 'EOF'
[workflow]
name = "pixi-container"

[[rules]]
name = "pixi_task"
shell = "echo pixi"

[rules.environment]
pixi = "pixi.toml"
EOF
oxo-flow package pixi-container.oxoflow -f docker
# Expected: Dockerfile installs Pixi
# Coverage: Pixi in containers, modern environment
```

**I39: Container with extra packages**
```bash
# Scenario: Adding extra apt packages to container
cd "$OXO_TEST_WORKSPACE/test-I39"
cat > extra-pkg.oxoflow << 'EOF'
[workflow]
name = "extra-packages"
EOF
oxo-flow export extra-pkg.oxoflow -f docker --extra-packages samtools bcftools
# Expected: Dockerfile with apt install samtools bcftools
# Coverage: Package customization, container flexibility
```

**I40: Healthcheck in container**
```bash
# Scenario: Container with custom healthcheck
oxo-flow package examples/gallery/01_hello_world.oxoflow -f docker
grep "HEALTHCHECK" output
# Expected: Dockerfile contains HEALTHCHECK directive
# Coverage: Container health monitoring
```

---

## Phase 3: Advanced Tests (30 instances)

### User Profile: Expert user, building complex pipelines

### Test A01-A10: Complex Workflow Patterns

**A01: Multi-omics integration**
```bash
# Scenario: Integrating genomics, transcriptomics, proteomics
oxo-flow dry-run examples/gallery/08_multiomics_integration.oxoflow
# Expected: Shows complex multi-branch DAG
# Coverage: Multi-omics, complex dependencies
```

**A02: Variant calling with VCF merging**
```bash
# Scenario: Calling variants per-chromosome then merging
oxo-flow run examples/gallery/04_scatter_gather.oxoflow -j 8 --dry-run
# Expected: Scatter to chromosomes, then gather_gvcf
# Coverage: Scatter-gather, parallel reduction
```

**A03: Conditional with wildcards**
```bash
# Scenario: Conditional rules with wildcard samples
cd "$OXO_TEST_WORKSPACE/test-A03"
cat > cond-wildcard.oxoflow << 'EOF'
[workflow]
name = "cond-wildcard"

[config]
samples = ["S1", "S2", "S3"]
skip_samples = ["S2"]

[[rules]]
name = "process_{sample}"
input = ["{sample}.fq"]
output = ["{sample}.bam"]
scatter = { variable = "sample", values_from = "config.samples" }
when = "!config.skip_samples.contains(sample)"
shell = "echo process {sample}"
EOF
oxo-flow validate cond-wildcard.oxoflow
# Expected: Conditional scatter recognized
# Coverage: Conditional + wildcard combination
```

**A04: Nested dependencies**
```bash
# Scenario: Deep dependency chain (10 levels)
cd "$OXO_TEST_WORKSPACE/test-A04"
cat > deep-chain.oxoflow << 'EOF'
[workflow]
name = "deep-chain"

[[rules]]
name = "step1"
output = ["out1.txt"]
shell = "echo step1 > out1.txt"

[[rules]]
name = "step2"
depends_on = ["step1"]
input = ["out1.txt"]
output = ["out2.txt"]
shell = "cat {input} > {output}"

# ... repeat for 10 steps
EOF
oxo-flow graph deep-chain.oxoflow
# Expected: Long chain visualization
# Coverage: Deep DAG, dependency tracking
```

**A05: Parallel groups**
```bash
# Scenario: Grouping parallel rules with resource constraints
oxo-flow validate examples/gallery/03_parallel_samples.oxoflow
# Expected: Parallel execution with thread limits
# Coverage: Parallel groups, resource management
```

**A06: Retry and failure handling**
```bash
# Scenario: Rules with retry logic
cd "$OXO_TEST_WORKSPACE/test-A06"
cat > retry-test.oxoflow << 'EOF'
[workflow]
name = "retry-demo"

[[rules]]
name = "flaky_task"
retries = 3
retry_delay = "10s"
on_failure = "echo failed"
shell = "echo flaky"
EOF
oxo-flow validate retry-test.oxoflow
# Expected: Retry configuration recognized
# Coverage: Error handling, retry system
```

**A07: Shadow execution**
```bash
# Scenario: Shadow directories for isolated execution
cd "$OXO_TEST_WORKSPACE/test-A07"
cat > shadow-test.oxoflow << 'EOF'
[workflow]
name = "shadow-demo"

[[rules]]
name = "shadow_task"
input = ["ref.fa"]
output = ["result.txt"]
shadow = "minimal"
shell = "echo shadow"
EOF
oxo-flow validate shadow-test.oxoflow
# Expected: Shadow directive recognized
# Coverage: Shadow execution, isolation
```

**A08: Ancient inputs**
```bash
# Scenario: Ancient files (don't trigger reruns)
cd "$OXO_TEST_WORKSPACE/test-A08"
cat > ancient-test.oxoflow << 'EOF'
[workflow]
name = "ancient-demo"

[[rules]]
name = "ancient_task"
ancient = ["reference.fa"]
input = ["data.fq", "reference.fa"]
output = ["aligned.bam"]
shell = "echo ancient"
EOF
oxo-flow validate ancient-test.oxoflow
# Expected: Ancient inputs recognized
# Coverage: Ancient file handling, rerun control
```

**A09: Local rules**
```bash
# Scenario: Rules that run locally on scheduler nodes
cd "$OXO_TEST_WORKSPACE/test-A09"
cat > local-test.oxoflow << 'EOF'
[workflow]
name = "local-demo"

[[rules]]
name = "local_task"
localrule = true
shell = "echo local"
EOF
oxo-flow validate local-test.oxoflow
# Expected: Local rule recognized
# Coverage: Local execution, scheduler bypass
```

**A10: Subworkflow includes**
```bash
# Scenario: Including subworkflows
cd "$OXO_TEST_WORKSPACE/test-A10"
cat > subwf.oxoflow << 'EOF'
[workflow]
name = "sub-workflow"
[[rules]]
name = "sub_task"
shell = "echo sub"
EOF
cat > mainwf.oxoflow << 'EOF'
[workflow]
name = "main-workflow"

[[include]]
path = "subwf.oxoflow"

[[rules]]
name = "main_task"
depends_on = ["sub_task"]
shell = "echo main"
EOF
oxo-flow validate mainwf.oxoflow
# Expected: Include recognized
# Coverage: Subworkflow, modular pipelines
```

### Test A11-A20: Report Generation

**A11: HTML report generation**
```bash
# Scenario: Generating execution report
cd "$OXO_TEST_WORKSPACE/test-A11"
oxo-flow run examples/gallery/01_hello_world.oxoflow
oxo-flow report examples/gallery/01_hello_world.oxoflow -f html -o report.html
# Expected: Valid HTML with dark mode support
# Coverage: HTML reports, execution documentation
```

**A12: JSON report generation**
```bash
# Scenario: Generating machine-readable report
oxo-flow report examples/gallery/01_hello_world.oxoflow -f json -o report.json
# Expected: Valid JSON with all metadata
# Coverage: JSON reports, data export
```

**A13: Report with provenance**
```bash
# Scenario: Report including execution provenance
cat report.html | grep -i provenance
# Expected: Provenance section in report
# Coverage: Provenance documentation, audit trail
```

**A14: Clinical disclaimer**
```bash
# Scenario: Report with clinical disclaimer
cat report.html | grep -i "clinical"
# Expected: Clinical disclaimer section
# Coverage: Clinical compliance, regulatory
```

**A15: QC metrics report**
```bash
# Scenario: Report with QC metrics
cd "$OXO_TEST_WORKSPACE/test-A15"
cat > qc-workflow.oxoflow << 'EOF'
[workflow]
name = "qc-demo"

[[rules]]
name = "qc"
output = ["qc.json"]
shell = "echo {\"total_reads\": 1000, \"mapped\": 950} > qc.json"
EOF
oxo-flow report qc-workflow.oxoflow -f html -o qc-report.html
# Expected: QC metrics section
# Coverage: QC reporting, quality documentation
```

**A16: Sample information report**
```bash
# Scenario: Report with sample metadata
cat report.html | grep -i "sample"
# Expected: Sample information section
# Coverage: Sample tracking, metadata
```

**A17: Execution time chart**
```bash
# Scenario: Report with timing visualization
cat report.html | grep -i "chart\|svg"
# Expected: SVG timing chart
# Coverage: Visualization, timing analysis
```

**A18: Report with variants**
```bash
# Scenario: Report showing variant summary
oxo-flow report examples/simple_variant_calling.oxoflow -f html
# Expected: Variant summary section
# Coverage: Variant reporting, clinical output
```

**A19: Table of contents**
```bash
# Scenario: Report with navigation TOC
cat report.html | grep -i "toc\|contents"
# Expected: Table of contents
# Coverage: Navigation, report structure
```

**A20: Multi-rule report**
```bash
# Scenario: Report for complex workflow
oxo-flow report examples/gallery/07_wgs_germline.oxoflow -f html -o wgs-report.html
wc -l wgs-report.html
# Expected: Large report with many sections
# Coverage: Complex workflow reporting
```

### Test A21-A30: Venus Clinical Pipeline

**A21: Venus pipeline generation**
```bash
# Scenario: Generating clinical somatic pipeline
cd "$OXO_TEST_WORKSPACE/test-A21"
cat > venus_config.toml << 'EOF'
mode = "ExperimentOnly"
seq_type = "WGS"
genome_build = "GRCh38"
reference_fasta = "/ref/hg38.fa"
threads = 16
output_dir = "output"
annotate = true
report = true

[[experiment_samples]]
name = "TUMOR_01"
r1_fastq = "raw/TUMOR_01_R1.fq.gz"
r2_fastq = "raw/TUMOR_01_R2.fq.gz"
is_experiment = true
EOF
venus generate venus_config.toml -o somatic_pipeline.oxoflow
# Expected: Generated somatic calling workflow
# Coverage: Venus pipeline generation, clinical
```

**A22: Venus validate config**
```bash
# Scenario: Validating Venus configuration
venus validate venus_config.toml
# Expected: Config validation passes
# Coverage: Venus config validation
```

**A23: Venus list steps**
```bash
# Scenario: Listing available Venus pipeline steps
venus list-steps
# Expected: Shows Fastp, BwaMem2, Mutect2, Strelka2, etc.
# Coverage: Venus pipeline steps, clinical tools
```

**A24: Experiment-Control mode**
```bash
# Scenario: Tumor-normal paired analysis
cd "$OXO_TEST_WORKSPACE/test-A24"
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
venus generate paired_config.toml -o paired_pipeline.oxoflow
# Expected: Paired somatic calling workflow
# Coverage: Paired analysis, tumor-normal
```

**A25: WES mode**
```bash
# Scenario: Whole exome sequencing pipeline
cd "$OXO_TEST_WORKSPACE/test-A25"
cat > wes_config.toml << 'EOF'
mode = "ExperimentOnly"
seq_type = "WES"
target_bed = "/ref/exome_targets.bed"
EOF
venus generate wes_config.toml
# Expected: WES-specific pipeline steps
# Coverage: WES pipeline, targeted sequencing
```

**A26: RNA-seq mode**
```bash
# Scenario: RNA expression pipeline
cd "$OXO_TEST_WORKSPACE/test-A26"
cat > rnaseq_config.toml << 'EOF'
mode = "ExperimentOnly"
seq_type = "RNA-seq"
EOF
venus generate rnaseq_config.toml
# Expected: RNA-seq pipeline steps
# Coverage: RNA-seq, expression analysis
```

**A27: Multiple samples**
```bash
# Scenario: Batch sample processing
cd "$OXO_TEST_WORKSPACE/test-A27"
cat > batch_config.toml << 'EOF'
mode = "ExperimentOnly"

[[experiment_samples]]
name = "SAMPLE_01"

[[experiment_samples]]
name = "SAMPLE_02"

[[experiment_samples]]
name = "SAMPLE_03"
EOF
venus validate batch_config.toml
# Expected: Multiple samples recognized
# Coverage: Batch processing, sample management
```

**A28: Venus with CNV calling**
```bash
# Scenario: Pipeline with CNVkit integration
grep -i cnvkit somatic_pipeline.oxoflow
# Expected: CNV calling steps present
# Coverage: CNV calling, copy number analysis
```

**A29: Venus with MSI detection**
```bash
# Scenario: Pipeline with MSI analysis
grep -i msisensor somatic_pipeline.oxoflow
# Expected: MSI detection steps present
# Coverage: MSI analysis, biomarker detection
```

**A30: Venus clinical report**
```bash
# Scenario: Pipeline generating clinical report
grep -i "clinical_report\|ClinicalReport" somatic_pipeline.oxoflow
# Expected: Clinical report generation step
# Coverage: Clinical reporting, regulatory output
```

---

## Phase 4: Expert Tests (20 instances)

### User Profile: Production deployment, clinical compliance

### Test E01-E10: Production Features

**E01: SHA-256 checksum verification**
```bash
# Scenario: Verifying file integrity with SHA-256
cd "$OXO_TEST_WORKSPACE/test-E01"
cat > test.txt
echo "hello world"
sha256sum test.txt
oxo-flow run examples/gallery/01_hello_world.oxoflow
cat .oxo-flow/provenance.json | jq '.output_checksums'
# Expected: SHA-256 checksums in provenance
# Coverage: SHA-256, clinical integrity
```

**E02: Provenance persistence**
```bash
# Scenario: Automatic provenance saving
cat .oxo-flow/provenance.json
# Expected: Valid provenance JSON
# Coverage: Provenance persistence, audit trail
```

**E03: Checkpoint recovery**
```bash
# Scenario: Resuming from checkpoint
oxo-flow run examples/gallery/02_file_pipeline.oxoflow
# Interrupt execution
oxo-flow status .oxo-flow/checkpoint.json
# Expected: Shows completed rules
# Coverage: Checkpoint system, resumable execution
```

**E04: Clean operation**
```bash
# Scenario: Cleaning workflow outputs
oxo-flow clean examples/gallery/01_hello_world.oxoflow -n
# Expected: Shows files to clean (dry-run)
# Coverage: Clean system, output management
```

**E05: Touch operation**
```bash
# Scenario: Marking outputs as complete without execution
oxo-flow touch examples/gallery/01_hello_world.oxoflow
# Expected: Outputs marked as up-to-date
# Coverage: Touch system, rerun control
```

**E06: Diff workflows**
```bash
# Scenario: Comparing workflow versions
cd "$OXO_TEST_WORKSPACE/test-E06"
cat > v1.oxoflow << 'EOF'
[workflow]
name = "test"
version = "1.0"
[[rules]]
name = "step1"
shell = "echo v1"
EOF
cat > v2.oxoflow << 'EOF'
[workflow]
name = "test"
version = "2.0"
[[rules]]
name = "step1"
shell = "echo v2"
[[rules]]
name = "step2"
shell = "echo added"
EOF
oxo-flow diff v1.oxoflow v2.oxoflow
# Expected: Shows added rule, changed shell
# Coverage: Diff system, version comparison
```

**E07: Config inspection**
```bash
# Scenario: Inspecting workflow configuration
oxo-flow config show examples/gallery/07_wgs_germline.oxoflow
# Expected: Shows workflow config details
# Coverage: Config inspection, debugging
```

**E08: Config statistics**
```bash
# Scenario: Workflow statistics
oxo-flow config stats examples/gallery/07_wgs_germline.oxoflow
# Expected: Shows rule count, resource summary
# Coverage: Statistics, workflow analysis
```

**E09: Secret scanning integration**
```bash
# Scenario: Lint with secret detection
cd "$OXO_TEST_WORKSPACE/test-E09"
cat > secret-test.oxoflow << 'EOF'
[workflow]
name = "test"
[config]
password = "secret123"
api_key = "AKIAIOSFODNN7EXAMPLE"
EOF
oxo-flow lint secret-test.oxoflow
# Expected: S008 warnings for secrets
# Coverage: Secret scanning, security
```

**E10: Validation error codes**
```bash
# Scenario: Testing all validation error codes
# Create workflows with E001-E009 errors
# Verify each produces correct error code
# Coverage: Error codes E001-E009
```

### Test E11-E20: Web Interface

**E11: Web server startup**
```bash
# Scenario: Starting web interface
oxo-flow-web --port 8080 &
sleep 2
curl http://localhost:8080/health
# Expected: Health endpoint responds
# Coverage: Web server, API health
```

**E12: Workflow upload**
```bash
# Scenario: Uploading workflow via API
curl -X POST -F "workflow=@examples/gallery/01_hello_world.oxoflow" http://localhost:8080/api/workflows
# Expected: Workflow uploaded successfully
# Coverage: Web API, workflow management
```

**E13: Workflow listing**
```bash
# Scenario: Listing workflows via API
curl http://localhost:8080/api/workflows
# Expected: JSON list of workflows
# Coverage: Web API, workflow listing
```

**E14: Workflow validation via API**
```bash
# Scenario: Remote validation
curl -X POST -F "workflow=@test.oxoflow" http://localhost:8080/api/validate
# Expected: Validation result in JSON
# Coverage: Web API, remote validation
```

**E15: Job submission via API**
```bash
# Scenario: Submitting execution via API
curl -X POST -d '{"workflow_id": "123"}' http://localhost:8080/api/run
# Expected: Execution started
# Coverage: Web API, remote execution
```

**E16: Status monitoring**
```bash
# Scenario: Monitoring execution status
curl http://localhost:8080/api/status/123
# Expected: Current execution status
# Coverage: Web API, monitoring
```

**E17: Report download**
```bash
# Scenario: Downloading report from web
curl http://localhost:8080/api/report/123 -o report.html
# Expected: HTML report downloaded
# Coverage: Web API, report delivery
```

**E18: Authentication**
```bash
# Scenario: Testing API authentication
curl -H "Authorization: Bearer token" http://localhost:8080/api/workflows
# Expected: Authenticated access
# Coverage: Web security, authentication
```

**E19: Web shutdown**
```bash
# Scenario: Graceful server shutdown
kill $(pgrep oxo-flow-web)
# Expected: Clean shutdown
# Coverage: Server lifecycle
```

**E20: Web audit logging**
```bash
# Scenario: Checking audit logs
# Coverage: Audit system, compliance logging
```

---

## Test Execution Order

1. **Phase 1: Beginner Tests** (B01-B30) - Basic discovery and simple workflows
2. **Phase 2: Intermediate Tests** (I01-I40) - Configuration and patterns
3. **Phase 3: Advanced Tests** (A01-A30) - Complex workflows and reports
4. **Phase 4: Expert Tests** (E01-E20) - Production and clinical

## Cleanup

```bash
# After all tests complete
cd /Users/wsx/Documents/GitHub/oxo-flow
rm -rf "$OXO_TEST_WORKSPACE"
conda deactivate
conda env remove -n oxo-test-env -y
```

## Success Criteria

- All 120+ tests execute without errors
- Coverage includes:
  - 50+ CLI command variations
  - 15+ Web interface tests
  - 15+ Venus pipeline tests
  - 25+ Gallery workflow tests
  - 20+ Bioinformatics pattern tests
- All error codes (E001-E009) properly detected
- All warning codes (W001-W018) properly flagged
- GPU, cluster, container features verified
- Clinical compliance features verified