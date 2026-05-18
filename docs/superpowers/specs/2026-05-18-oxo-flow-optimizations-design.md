# oxo-flow Optimizations Design

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Improve oxo-flow usability and robustness based on lessons from oxo-flow-clindet and oxo-flow-circrna development.

**Architecture:** Incremental enhancements across 4 subsystems: CLI validation, Configuration, Runtime, and Rule definition.

**Tech Stack:** Rust 2024 edition, existing dependencies (clap, serde, toml, sysinfo, petgraph)

---

## P0: Essential Optimizations

### 1. `--as-include` Validation Mode

**Purpose:** Validate sub-workflow fragments without requiring parent context.

**CLI interface:**
```bash
oxo-flow validate rules/qc.oxoflow --as-include
```

**Behavior:**
- Parse the file as TOML
- Validate rule syntax (name, input, output, shell/script)
- Check for required fields per rule
- Report errors with line numbers
- Exit 0 if valid, non-zero if errors
- Skip DAG dependency validation (fragments don't have complete DAG)
- Skip missing input file checks (will be checked in parent context)

**Files to modify:**
- `crates/oxo-flow-cli/src/commands/quality.rs` - Add `--as-include` flag
- `crates/oxo-flow-cli/src/main.rs` - Update CLI definition

**Acceptance criteria:**
- `oxo-flow validate sub.oxoflow --as-include` passes for valid fragment
- `oxo-flow validate sub.oxoflow --as-include` fails with clear error for invalid fragment
- DAG validation is skipped when flag is set
- Missing input file checks are skipped when flag is set

---

### 2. `reference_dir` Configuration Convention

**Purpose:** Simplify reference file configuration with a single base path.

**Configuration format:**
```toml
[config]
reference_dir = "/data/references/GRCh38"

# Auto-derived (implicit):
# - reference_fasta → {reference_dir}/genome.fa
# - gene_annotation → {reference_dir}/genes.gtf
# - bwa_index → {reference_dir}/bwa/genome.fa
# - bowtie2_index → {reference_dir}/bowtie2/genome.fa
# - star_index → {reference_dir}/star
# - hisat2_index → {reference_dir}/hisat2/genome.fa

# Explicit overrides allowed:
reference_fasta = "/custom/path/genome.fa"  # overrides auto-derivation
```

**Auto-derivation rules:**
| Config Key | Derived Path |
|------------|--------------|
| `reference_fasta` | `{reference_dir}/genome.fa` |
| `gene_annotation` | `{reference_dir}/genes.gtf` |
| `bwa_index` | `{reference_dir}/bwa/genome.fa` |
| `bowtie2_index` | `{reference_dir}/bowtie2/genome.fa` |
| `star_index` | `{reference_dir}/star` |
| `hisat2_index` | `{reference_dir}/hisat2/genome.fa` |

**Files to modify:**
- `crates/oxo-flow-core/src/config.rs` - Add `reference_dir` field and auto-derivation
- `crates/oxo-flow-core/src/format.rs` - Document convention in comments

**Acceptance criteria:**
- Setting `reference_dir` auto-populates standard paths
- Explicit values override auto-derived ones
- Works with existing config parsing

---

## P1: Important Optimizations

### 3. Memory/Thread Auto-Scaling

**Purpose:** Automatically adjust resources based on available system capacity.

**Configuration:**
```toml
[defaults]
threads = "auto"   # or explicit number like 8
memory = "auto"    # or explicit like "16G"

[[rules]]
name = "heavy_task"
threads = "auto"   # uses up to available CPUs (capped by --max-threads)
memory = "32G"     # explicit override
```

**Behavior:**
- If rule has `threads = "auto"` → scale to `min(default_threads, available_cpus)`
- If rule has `memory = "auto"` → scale to `min(default_memory, 80% of available_memory)`
- CLI flags `--max-threads` and `--max-memory` set global caps (already implemented)

**Files to modify:**
- `crates/oxo-flow-core/src/rule.rs` - Extend threads/memory parsing for "auto"
- `crates/oxo-flow-core/src/executor.rs` - Implement auto-scaling logic

**Acceptance criteria:**
- `"auto"` value is accepted for threads and memory
- System resources are queried using sysinfo crate
- Values are capped at available resources

---

### 4. Environment Groups

**Purpose:** Share environments across rules to reduce setup overhead.

**Configuration:**
```toml
[env_groups.qc]
conda = "envs/qc.yaml"

[env_groups.align]
conda = "envs/align.yaml"

[[rules]]
name = "fastqc"
env_group = "qc"  # references env_groups.qc

[[rules]]
name = "trim_galore"
env_group = "qc"  # reuses same environment
```

**Behavior:**
- `env_group` takes precedence over rule-level `environment`
- Environment is resolved at DAG construction time
- Same environment instance is shared across all rules in the group (cached)
- Validation error if `env_group` references non-existent group

**Files to modify:**
- `crates/oxo-flow-core/src/config.rs` - Add `env_groups` to config, `env_group` to rule
- `crates/oxo-flow-core/src/rule.rs` - Add `env_group` field
- `crates/oxo-flow-core/src/executor.rs` - Modify environment resolution logic
- `crates/oxo-flow-core/src/format.rs` - Add validation for undefined group references

**Acceptance criteria:**
- `env_groups` section is parsed from workflow config
- Rules can reference environment groups by name
- Undefined group references cause validation error
- Shared environments are only set up once

---

## P2: Enhancement Optimizations

### 5. Optional Rule Support

**Purpose:** Allow rules to be skipped if inputs are missing.

**Configuration:**
```toml
[[rules]]
name = "optional_qc"
input = "{sample}_extra.fastq"  # may not exist for all samples
output = "{sample}_extra_qc.html"
shell = "fastqc {input}"
optional = true  # skip if input missing, no error
```

**Behavior:**
- When `optional = true` and input files don't exist, the rule is skipped
- Skipped rules don't block dependent rules (outputs treated as "not produced")
- Validation warns if optional rule has no wildcards in input
- Default is `optional = false` (current behavior)

**Files to modify:**
- `crates/oxo-flow-core/src/rule.rs` - Add `optional` field
- `crates/oxo-flow-core/src/executor.rs` - Check input existence for optional rules
- `crates/oxo-flow-core/src/dag.rs` - Handle missing outputs from skipped rules

**Acceptance criteria:**
- Optional rules are skipped when inputs missing
- Non-optional rules still fail on missing inputs
- Skipped optional rules are logged at info level

---

### 6. Directory Input Type

**Purpose:** Support directory inputs with change detection.

**Configuration:**
```toml
[[rules]]
name = "process_dir"
input = { dir = "data/raw/" }
output = "results/processed/"
shell = "process-dir {input} -o {output}"
```

**Behavior:**
- Directory input tracks all files recursively
- Rule re-runs if any file in directory is newer than output
- Supports glob patterns: `{ dir = "data/*/", pattern = "*.fastq" }`

**Files to modify:**
- `crates/oxo-flow-core/src/rule.rs` - Extend `FilePatterns` for directory type
- `crates/oxo-flow-core/src/executor.rs` - Handle directory input checking

**Acceptance criteria:**
- Directory inputs are tracked recursively
- Modification time is max of all contained files
- Pattern filtering works for directory inputs

---

## Documentation Updates

All changes require:
1. Inline documentation comments in modified code
2. README.md updates for new CLI flags and config options
3. CHANGELOG.md entry for the release
4. Format specification updates in `format.rs` comments

## Testing Requirements

- All existing tests must pass (`cargo test`)
- New unit tests for each feature
- Integration tests for `--as-include` validation
- `make ci` must pass (fmt + clippy + build + test + audit)
