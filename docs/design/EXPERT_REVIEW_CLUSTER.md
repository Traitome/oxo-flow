# Expert Review: HPC Cluster Job Submission and Resource Management

**Reviewer**: HPC Cluster Administrator (SLURM/PBS/SGE/LSF clusters, 1000+ users)
**Date**: 2026-05-14
**Files Reviewed**:
- `/Users/wsx/Documents/GitHub/oxo-flow/docs/guide/src/commands/cluster.md`
- `/Users/wsx/Documents/GitHub/oxo-flow/crates/oxo-flow-core/src/cluster.rs`
- `/Users/wsx/Documents/GitHub/oxo-flow/crates/oxo-flow-core/src/environment.rs`
- `/Users/wsx/Documents/GitHub/oxo-flow/crates/oxo-flow-core/src/rule.rs`
- `/Users/wsx/Documents/GitHub/oxo-flow/crates/oxo-flow-cli/src/main.rs`

---

## Summary

oxo-flow provides basic cluster script generation for SLURM, PBS, SGE, and LSF schedulers with environment wrapping support. However, several critical features for production HPC workflows are missing or incomplete. Users with 100+ job submissions will face significant friction.

---

## Issues by Severity

### CRITICAL (Must Fix Before Production Use)

#### 1. GPU Resources Not Translated to Scheduler Directives

**Location**: `crates/oxo-flow-core/src/cluster.rs` (lines 175-289)

**Problem**: The Rule struct defines GPU requirements (`resources.gpu`, `resources.gpu_spec`) but these are never translated to scheduler-specific GPU directives. Users must manually add `--gres=gpu:N` via `extra_args`.

**Evidence**:
```rust
// rule.rs defines GPU fields:
pub gpu: Option<u32>,
pub gpu_spec: Option<GpuSpec>,

// But cluster.rs never uses them:
fn generate_slurm_script(rule: &Rule, shell_cmd: &str, config: &ClusterJobConfig) -> String {
    // No GPU directive generated from rule.resources.gpu
}
```

**Impact**: GPU workflows (deep learning, GATK GPU acceleration, molecular dynamics) require manual extra_args for every GPU rule. Confusing for users who define `gpu = 2` in their workflow but jobs fail on GPU partitions.

**Fix Required**:
```rust
// Add GPU directive translation:
if let Some(gpu_count) = rule.resources.gpu {
    lines.push(format!("#SBATCH --gres=gpu:{}", gpu_count));
}
if let Some(ref spec) = rule.resources.gpu_spec {
    if let Some(ref model) = spec.model {
        lines.push(format!("#SBATCH --gres=gpu:{}:{}", model, spec.count));
    }
}
```

Per-backend mappings:
- SLURM: `--gres=gpu:N` or `--gres=gpu:MODEL:N`
- PBS: `-l gpu=N`
- SGE: Complex, depends on cluster config (typically `-l gpu=N`)
- LSF: `-gpu N`

---

#### 2. No Job Array Support for Batch Processing

**Location**: `crates/oxo-flow-core/src/cluster.rs`, ROADMAP.md line 426

**Problem**: HPC workflows commonly process 100-1000 samples. oxo-flow generates individual scripts for each rule, but has no mechanism for SLURM job arrays (`#SBATCH --array=1-100`). The `scatter` configuration in rules exists but is not translated to job arrays.

**Evidence**:
```rust
// rule.rs defines scatter:
pub scatter: Option<ScatterConfig>,

// But cluster.rs ignores it - generates one script per rule
```

**Impact**: A workflow with 500 samples generates 500+ individual submit scripts. User must submit each manually or write wrapper scripts. Scheduler queue becomes flooded. No efficient resource sharing.

**Fix Required**:
1. Add `--array` directive support when `scatter` is defined
2. Generate array index substitution in shell command (`$SLURM_ARRAY_TASK_ID`)
3. Allow users to submit one array job instead of N individual jobs

Example output:
```bash
#!/bin/bash
#SBATCH --job-name=process_samples
#SBATCH --array=1-500
#SBATCH --output=logs/process_samples_%a.out

SAMPLE=$(sed -n "${SLURM_ARRAY_TASK_ID}p" samples.txt)
bwa mem -t 16 ref.fa ${SAMPLE}.fq > ${SAMPLE}.bam
```

---

#### 3. Log Directory Not Created in Generated Scripts

**Location**: `crates/oxo-flow-core/src/cluster.rs` (lines 195-196, 228-229, etc.)

**Problem**: Generated scripts write to `logs/{rule_name}.out` and `logs/{rule_name}.err` but never create the `logs/` directory. Jobs fail immediately on first execution.

**Evidence**:
```rust
lines.push(format!("#SBATCH --output=logs/{}.out", rule.name));
lines.push(format!("#SBATCH --error=logs/{}.err", rule.name));
// No mkdir command added
```

**Impact**: First-time users get confusing failures. Scheduler reports "output file creation failed".

**Fix Required**:
```rust
// Add before shell command:
lines.push(String::new());
lines.push("mkdir -p logs".to_string());
lines.push(shell_cmd.to_string());
```

---

### HIGH (Should Fix Soon)

#### 4. Per-Rule Walltime Not Translated from `resources.time_limit`

**Location**: `crates/oxo-flow-core/src/cluster.rs` (line 61)

**Problem**: Rules can define `resources.time_limit` but cluster script generation only uses `ClusterJobConfig.walltime` (global setting). Per-rule timeout is ignored.

**Evidence**:
```rust
// rule.rs:
pub time_limit: Option<String>,  // e.g., "24h", "30m"

// cluster.rs uses only config.walltime:
if let Some(ref wt) = config.walltime {
    lines.push(format!("#SBATCH --time={wt}"));
}
// rule.resources.time_limit is never checked
```

**Impact**: Short jobs get unnecessarily long walltime allocations. Long jobs may get insufficient time and timeout. Resource allocation inefficient.

**Fix Required**:
```rust
let walltime = rule.resources.time_limit
    .as_ref()
    .or(&config.walltime);
if let Some(ref wt) = walltime {
    // Convert format (rule uses "24h", SLURM expects "24:00:00")
    let formatted = convert_time_format(wt);
    lines.push(format!("#SBATCH --time={formatted}"));
}
```

---

#### 5. Environment Module Loading Not Implemented

**Location**: `crates/oxo-flow-core/src/environment.rs`

**Problem**: Rules can define `environment.modules = ["gcc/11.2", "cuda/11.7"]` but the EnvironmentResolver has no handler for module loading. Modules field is ignored.

**Evidence**:
```rust
// rule.rs:
pub modules: Vec<String>,

// environment.rs:
impl EnvironmentBackend for SystemBackend {
    fn wrap_command(&self, command: &str, _spec: &str) -> Result<String> {
        Ok(command.to_string())  // No module loading
    }
}
```

**Impact**: HPC users who rely on `module load` (standard on most clusters) cannot use this feature. Must manually add module commands to shell templates.

**Fix Required**:
Add module loading in generated script:
```bash
module purge
module load gcc/11.2
module load cuda/11.7
```

Or create a ModulesBackend in environment.rs.

---

#### 6. No Per-Rule Partition/Queue Override

**Location**: `crates/oxo-flow-cli/src/main.rs` (ClusterAction::Submit)

**Problem**: Users can only specify one global queue/partition via CLI `--queue`. Some rules need different queues (e.g., GPU rules on `gpu` partition, long jobs on `long` partition).

**Impact**: Users cannot optimize queue selection per rule type. GPU jobs may land on wrong partition.

**Fix Required**:
Add per-rule partition field:
```toml
[[rules]]
name = "gpu_align"
resources.partition = "gpu"  # New field
```

---

#### 7. No DAG Dependency Handling for Cluster Jobs

**Location**: `crates/oxo-flow-cli/src/main.rs` (ClusterAction::Submit, lines 1325-1410)

**Problem**: The CLI generates individual scripts for each rule but does not capture DAG dependencies. Users must manually submit jobs in correct order or use `--dependency` flags.

**Evidence**:
```rust
for rule_name in &order {
    // Generates script but no dependency tracking
    let script = generate_submit_script_with_env(...);
    std::fs::write(&script_path, &script)?;
}
// No dependency file or submit order guidance
```

**Impact**: 100-rule workflow generates 100 scripts with no dependency metadata. Users must figure out submission order themselves or jobs fail due to missing inputs.

**Fix Required**:
1. Generate a submit manifest with dependency order
2. Optionally add SLURM `--dependency=afterok:JOBID` to scripts
3. Provide a `--submit-all` mode that submits in order with dependencies

---

### MEDIUM (Improvements)

#### 8. Disk Space Requirements Not Translated

**Location**: `crates/oxo-flow-core/src/rule.rs` (line 90)

**Problem**: Rules can define `resources.disk` but this is never translated to scheduler directives.

**Fix Required**:
- SLURM: Some clusters support `--tmp=N` for local scratch
- PBS: `-l file=N`

---

#### 9. No Account Inheritance from Workflow Config

**Location**: `crates/oxo-flow-core/src/config.rs` (ClusterProfile)

**Problem**: Workflow config can define cluster.account but CLI submit requires explicit `--account` flag.

**Fix Required**:
CLI should default to `config.cluster.account` if not specified.

---

#### 10. SGE PE (Parallel Environment) Hardcoded

**Location**: `crates/oxo-flow-core/src/cluster.rs` (line 242)

**Problem**: SGE script generation hardcodes `#$ -pe smp N`. Many clusters use different PE names (`orte`, `mpi`, `shared`).

**Evidence**:
```rust
lines.push(format!("#$ -pe smp {}", rule.effective_threads()));
```

**Fix Required**:
Allow PE name configuration via ClusterProfile:
```toml
[cluster]
backend = "sge"
parallel_environment = "orte"
```

---

#### 11. Job Name Length Not Validated

**Problem**: SLURM limits job names to ~15-30 characters (depends on cluster). Rule names like `bwa_mem_align_samples_for_variant_calling` exceed limits.

**Fix Required**:
Truncate or hash long names. Warn user.

---

#### 12. No `localrule` Handling in Cluster Submit

**Location**: `crates/oxo-flow-core/src/rule.rs` (line 314)

**Problem**: Rules can be marked `localrule = true` but cluster submit generates scripts for all rules including local ones.

**Fix Required**:
Skip localrules in cluster submit, execute locally after cluster jobs complete.

---

### LOW (Minor Issues)

#### 13. Conda Environment Name Derivation

**Location**: `crates/oxo-flow-core/src/environment.rs` (line 60-63)

**Problem**: Environment name derived from YAML filename stem. Users may have YAML in nested paths like `envs/alignment/bwa.yaml` but conda env name is just `bwa`.

**Consideration**: Works for simple cases but may conflict if multiple `bwa.yaml` exist in different directories.

---

#### 14. Singularity/Apptainer Detection

**Location**: `crates/oxo-flow-core/src/environment.rs` (lines 143-150)

**Good**: Properly checks both `singularity` and `apptainer` commands. Modern clusters transitioning to Apptainer.

---

## Positive Findings

### 1. Environment Wrapping is Properly Implemented

The `generate_submit_script_with_env` function correctly wraps commands through the EnvironmentResolver. Conda, docker, singularity, pixi, and venv environments are properly handled.

```rust
// Correct implementation exists:
pub fn generate_submit_script_with_env(
    backend: &ClusterBackend,
    rule: &Rule,
    shell_cmd: &str,
    cluster_config: &ClusterJobConfig,
    env_resolver: &EnvironmentResolver,
) -> Result<String, String>
```

### 2. Multiple Backend Support

SLURM, PBS, SGE, and LSF all have dedicated script generators with correct directive syntax.

### 3. Extra Args Flexibility

The `extra_args` field in ClusterJobConfig allows users to add custom scheduler directives, providing workaround for missing features.

---

## User Workflow Assessment

### Current Experience (Pain Points)

1. **GPU User**: Defines `gpu = 2` in workflow, submits to cluster, job fails because no `--gres` directive. Must read docs, add `extra_args = ["--gres=gpu:2"]` to every GPU rule.

2. **Batch Processing**: Has 300 samples. Generates 300 scripts, manually submits all. Scheduler queue flooded. No efficient execution.

3. **First-Time User**: Runs `oxo-flow cluster submit`, submits job, immediately fails because `logs/` directory doesn't exist.

4. **Complex Workflow**: 50 rules with dependencies. Generates 50 scripts. Must manually figure out submission order from DAG output.

### Ideal Experience (After Fixes)

1. Define `resources.gpu = 2` - automatically gets `--gres=gpu:2`
2. Define `scatter.variable = "sample"` with 300 values - generates single array job
3. Scripts include `mkdir -p logs` - works immediately
4. Generate submit manifest with dependency order, or `--submit-with-dependencies` auto-submits in order

---

## Recommended Fix Priority

1. **Phase 1 (Critical)**:
   - GPU directive translation (Issue #1)
   - Log directory creation (Issue #3)
   - Per-rule walltime (Issue #4)

2. **Phase 2 (High)**:
   - Job array support (Issue #2) - significant work, high value
   - Module loading (Issue #5)
   - DAG dependency tracking (Issue #7)

3. **Phase 3 (Medium)**:
   - Per-rule partition (Issue #6)
   - Disk space translation (Issue #8)
   - Account inheritance (Issue #9)

---

## Concrete Code Changes Required

### cluster.rs - GPU Translation

```rust
fn generate_slurm_script(rule: &Rule, shell_cmd: &str, config: &ClusterJobConfig) -> String {
    let mut lines = vec!["#!/bin/bash".to_string()];
    lines.push(format!("#SBATCH --job-name={}", rule.name));
    lines.push(format!("#SBATCH --cpus-per-task={}", rule.effective_threads()));

    // NEW: GPU handling
    if let Some(gpu) = rule.resources.gpu {
        lines.push(format!("#SBATCH --gres=gpu:{}", gpu));
    }
    if let Some(ref spec) = rule.resources.gpu_spec {
        let gpu_str = match &spec.model {
            Some(model) => format!("gpu:{}:{}", model, spec.count),
            None => format!("gpu:{}", spec.count),
        };
        lines.push(format!("#SBATCH --gres={}", gpu_str));
    }

    // Memory handling (existing)
    if let Some(mem) = rule.effective_memory() {
        lines.push(format!("#SBATCH --mem={mem}"));
    }

    // NEW: Per-rule walltime
    let walltime = rule.resources.time_limit.as_ref().or(&config.walltime);
    if let Some(ref wt) = walltime {
        let formatted = format_walltime_for_slurm(wt);
        lines.push(format!("#SBATCH --time={formatted}"));
    }

    // ... rest of existing code ...

    // NEW: Create logs directory
    lines.push(String::new());
    lines.push("mkdir -p logs".to_string());

    // NEW: Module loading (if defined)
    if !rule.environment.modules.is_empty() {
        lines.push("module purge".to_string());
        for module in &rule.environment.modules {
            lines.push(format!("module load {}", module));
        }
    }

    lines.push(shell_cmd.to_string());
    lines.join("\n")
}
```

### New function for walltime format conversion

```rust
fn format_walltime_for_slurm(time_str: &str) -> String {
    // Convert "24h", "30m", "2d" to SLURM format "DD:HH:MM:SS"
    let time_str = time_str.trim();
    if time_str.contains(':') {
        return time_str.to_string(); // Already in correct format
    }

    let total_secs = parse_duration_secs(time_str).unwrap_or(3600);
    let days = total_secs / 86400;
    let hours = (total_secs % 86400) / 3600;
    let mins = (total_secs % 3600) / 60;
    let secs = total_secs % 60;

    if days > 0 {
        format!("{}-{:02}:{:02}:{:02}", days, hours, mins, secs)
    } else {
        format!("{:02}:{:02}:{:02}", hours, mins, secs)
    }
}
```

---

## Conclusion

oxo-flow's cluster support is functional for basic workflows but lacks critical features for production HPC use. GPU users, batch processing workflows, and users with complex DAGs will face significant friction. The fixes are straightforward to implement and would dramatically improve the user experience for cluster submission.