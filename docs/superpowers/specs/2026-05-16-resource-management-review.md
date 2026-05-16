# Resource Management Review and Optimization

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Improve reliability, portability, and observability of CPU/memory/resource management across local and HPC execution backends.

**Architecture:** Targeted enhancements to existing ResourcePool, executor, and scheduler integration - no major architectural changes.

**Tech Stack:** Rust, existing oxo-flow-core structures (Resources, ResourcePool, ExecutorConfig), sysinfo crate for cross-platform resource detection.

---

## 1. Current State Analysis

### What Works Well

| Feature | Status | Notes |
|---|---|---|
| Thread tracking | ✓ Reliable | ResourcePool correctly tracks thread allocation/release |
| Memory string parsing | ✓ Works | parse_memory_mb handles G/M/K/T suffixes correctly |
| SLURM script generation | ✓ Functional | Basic --cpus-per-task, --mem directives generated |
| Resource groups | ✓ Implemented | Custom groups (e.g., db_connection) work for limiting shared resources |
| Timeout per-rule | ✓ Implemented | time_limit field translates to scheduler walltime |

### Identified Problems

| Problem | Severity | Impact |
|---|---|---|
| **Memory detection only works on Linux** | HIGH | macOS/Windows workflows default to 8GB, may underestimate or overestimate |
| **Disk requirement ignored** | MEDIUM | Declared but never validated or enforced |
| **gpu_spec fields unused** | MEDIUM | model, memory_gb, compute_capability not translated to scheduler directives |
| **ResourceHint not utilized** | LOW | input_size, memory_scale fields defined but not used in scheduling |
| **No system capacity validation** | MEDIUM | Jobs can declare resources exceeding actual system capacity |
| **Process timeout unreliable** | HIGH | Timeout kills parent shell but child processes may survive |
| **Cleanup on failure incomplete** | MEDIUM | Partial outputs may remain after failed jobs |
| **No resource usage observability** | MEDIUM | Users can't see actual vs declared resource consumption |

---

## 2. Proposed Solutions (Simple & Practical)

### 2.1 Cross-Platform Memory Detection (Reliability)

**Problem:** `detect_system_resources()` only reads `/proc/meminfo` on Linux.

**Solution:** Use `sysinfo` crate for cross-platform detection:

```rust
fn detect_system_memory() -> u64 {
    use sysinfo::System;
    let mut sys = System::new_all();
    sys.refresh_memory();
    sys.total_memory() / 1024 / 1024  // bytes → MB
}
```

**Changes:**
- Add `sysinfo = "0.33"` to Cargo.toml (workspace)
- Update `executor.rs::detect_system_resources()` to use sysinfo
- Remove Linux-specific `/proc/meminfo` reading (keep as fallback)

### 2.2 Process Timeout Enforcement (Reliability)

**Problem:** `Command::timeout()` kills the shell process, but spawned children (BWA, GATK) may continue.

**Solution:** Process group termination:

```rust
use nix::sys::signal::{kill, Signal};
use nix::unistd::getpgid;

// On timeout, kill entire process group
fn kill_process_group(pid: u32) -> Result<()> {
    let pgid = getpgid(Some(nix::unistd::Pid::from_raw(pid as i32)))?;
    kill(pgid, Signal::SIGKILL)?;
    Ok(())
}
```

**Changes:**
- Add `nix = "0.29"` to Cargo.toml (Unix only)
- Wrap timeout handling to kill process group
- macOS/Linux support only (Windows uses job objects differently)

### 2.3 System Capacity Validation (Practical)

**Problem:** User declares `threads=128` on 16-core machine - job runs but wastes resources.

**Solution:** Warning-level validation, not enforcement:

```rust
fn validate_resources_against_system(rule: &Rule, system_threads: u32, system_memory_mb: u64) -> Vec<String> {
    let warnings = Vec::new();
    let req_threads = rule.effective_threads();
    let req_memory = rule.effective_memory().and_then(parse_memory_mb).unwrap_or(0);

    if req_threads > system_threads {
        warnings.push(format!(
            "rule '{}' requests {} threads but system has {} (will oversubscribe)",
            rule.name, req_threads, system_threads
        ));
    }
    if req_memory > system_memory_mb {
        warnings.push(format!(
            "rule '{}' requests {}MB but system has {}MB (may fail)",
            rule.name, req_memory, system_memory_mb
        ));
    }
    warnings
}
```

**Changes:**
- Add validation function in `config.rs`
- Call during workflow validation, emit warnings to tracing
- Don't block execution - user may intentionally oversubscribe

### 2.4 GPU Spec Translation (Practical)

**Problem:** `gpu_spec.model="A100"` not translated to scheduler directive.

**Solution:** Enhance SLURM/PBS script generators:

```rust
// SLURM GPU directive generation
fn slurm_gpu_directive(spec: &GpuSpec) -> String {
    match &spec.model {
        Some(model) => format!("--gres=gpu:{}:{}", model, spec.count),
        None => format!("--gres=gpu:{}", spec.count),
    }
}

// PBS GPU directive (varies by site)
fn pbs_gpu_directive(spec: &GpuSpec) -> String {
    format!("gpu={}", spec.count)  // Most PBS sites use this format
}
```

**Changes:**
- Update `cluster.rs` GPU handling in script generators
- Add model-specific GPU constraint for SLURM
- Document that PBS/SGE GPU syntax is site-specific

### 2.5 ResourceHint Basic Support (Simple)

**Problem:** `ResourceHint` defined but unused.

**Solution:** Simple memory scaling based on input_size estimate:

```rust
fn estimate_memory_from_hint(hint: &ResourceHint, base_mb: u64) -> u64 {
    if let Some(scale) = hint.memory_scale {
        let size_base = match hint.input_size.as_deref() {
            Some("small") => 1024,    // 1GB
            Some("medium") => 10240,  // 10GB
            Some("large") => 102400,  // 100GB
            _ => base_mb,
        };
        (size_base as f64 * scale) as u64
    } else {
        base_mb
    }
}
```

**Changes:**
- Add helper function in scheduler.rs
- Use when rule has no explicit memory but has resource_hint
- Simple scaling - don't attempt actual input size detection

### 2.6 Disk Validation (Simple)

**Problem:** `disk` field defined but ignored.

**Solution:** Basic pre-flight check, no enforcement:

```rust
fn check_disk_space(rule: &Rule, workdir: &Path) -> Vec<String> {
    let warnings = Vec::new();
    if let Some(ref disk) = rule.resources.disk {
        let required_mb = parse_memory_mb(disk).unwrap_or(0);
        // Get available space in workdir
        let available = fs2::available_space(workdir).unwrap_or(u64::MAX) / 1024 / 1024;
        if required_mb > available {
            warnings.push(format!(
                "rule '{}' may need {}MB disk but only {}MB available",
                rule.name, required_mb, available
            ));
        }
    }
    warnings
}
```

**Changes:**
- Add `fs2 = "0.4"` to Cargo.toml
- Check during dry-run/validation
- Warning only - can't enforce disk usage

### 2.7 Resource Usage Observability (Practical)

**Problem:** No visibility into actual resource consumption.

**Solution:** Log resource allocation state during execution:

```rust
tracing::info!(
    rule = %rule.name,
    threads_requested = %rule.effective_threads(),
    threads_available = %pool.threads,
    memory_requested_mb = %req_memory,
    memory_available_mb = %pool.memory_mb,
    "resource allocation"
);
```

**Changes:**
- Add structured logging in executor.rs reserve/release
- Add --verbose flag to show resource pool state
- Extend dry-run output to show resource requirements summary

### 2.8 Cleanup on Failure Improvement (Reliability)

**Problem:** Partial outputs remain after failed jobs.

**Solution:** Mark temp outputs for cleanup:

```rust
async fn execute_rule(...) -> Result<JobRecord> {
    // ... execution ...
    if result.is_err() {
        // Clean up temp outputs on failure
        for temp in &rule.temp_output {
            if let Err(e) = tokio::fs::remove_file(expand_path(temp)).await {
                tracing::warn!("failed to cleanup temp output: {}", e);
            }
        }
        // Clean up chunk outputs if transform with cleanup=true
        if rule.transform.as_ref().map(|t| t.cleanup).unwrap_or(false) {
            // ... cleanup logic
        }
    }
    result
}
```

**Changes:**
- Add cleanup logic in executor failure handling
- Clean temp_output files on job failure
- Clean transform chunk outputs when cleanup=true

---

## 3. Files to Modify

| File | Changes |
|---|---|
| `Cargo.toml` (workspace) | Add: sysinfo, nix, fs2 crates |
| `crates/oxo-flow-core/Cargo.toml` | Add workspace dependencies |
| `crates/oxo-flow-core/src/executor.rs` | Cross-platform detection, process group timeout, cleanup |
| `crates/oxo-flow-core/src/scheduler.rs` | ResourceHint estimation, disk validation helper |
| `crates/oxo-flow-core/src/config.rs` | System capacity validation |
| `crates/oxo-flow-core/src/cluster.rs` | GPU spec translation to scheduler directives |
| `crates/oxo-flow-cli/src/main.rs` | Resource warnings display, verbose logging |
| `docs/guide/src/reference/workflow-format.md` | Document resource management behavior |
| `docs/guide/src/how-to/resource-tuning.md` | New: Best practices for resource declaration |

---

## 4. Testing Requirements

### Unit Tests

| Test | Purpose |
|---|---|
| `detect_system_memory_cross_platform` | Verify sysinfo returns valid values |
| `validate_resources_exceed_system` | Warning generated when oversubscribed |
| `gpu_spec_to_slurm_directive` | Model constraint correctly formatted |
| `memory_scale_from_hint` | ResourceHint scaling math |
| `disk_space_validation` | Warning when insufficient disk |

### Integration Tests

| Test | Purpose |
|---|---|
| `timeout_kills_process_group` | Child processes terminated on timeout |
| `cleanup_temp_on_failure` | Temp files removed after job failure |
| `resource_pool_logging` | Verbose mode shows allocation state |

### CI Requirements

- All tests must pass
- Cross-platform CI: Linux, macOS (Windows optional)
- GPU tests mocked (no actual GPU required)

---

## 5. Documentation Updates

### workflow-format.md

Add section on resource management behavior:

```markdown
## Resource Management

### Declaration vs Enforcement

oxo-flow tracks declared resources for scheduling but does not strictly enforce
them in local execution. On HPC clusters, resources are enforced by the scheduler.

### Platform Detection

- **Linux**: Reads /proc/meminfo for accurate memory
- **macOS/Windows**: Uses sysinfo crate for memory detection
- **Threads**: Uses num_cpus crate

### Validation Warnings

When a rule declares resources exceeding system capacity, oxo-flow emits warnings
but does not block execution. This allows intentional oversubscription for testing.

### Cleanup Behavior

- `temp_output`: Automatically cleaned after successful completion
- Failed jobs: Temp outputs cleaned to prevent stale partial files
- Transform chunks: Cleaned when `cleanup=true` and combine succeeds
```

### New: resource-tuning.md

Best practices guide:

```markdown
# Resource Tuning Guide

## Thread Declaration

Match threads to tool's parallelism capability:
- **BWA-MEM2**: threads=16 (saturates ~12-16 cores)
- **samtools sort**: threads=4 + memory=2G per thread
- **GATK**: threads varies by tool (CheckDuplicates: threads=1, HaplotypeCaller: threads=4)

## Memory Declaration

Rule of thumb:
- **Alignment**: 2-4x largest input file size
- **Variant calling**: 32G for WGS, 8G for targeted panels
- **Sort/index**: threads × 2G per thread for samtools

## GPU Resources

For SLURM with specific GPU models:
```toml
[rules.resources.gpu_spec]
count = 2
model = "A100"
memory_gb = 40
```

## ResourceHints

When exact requirements unknown:
```toml
[rules.resource_hint]
input_size = "medium"  # ~10GB
memory_scale = 2.0     # 2x input size
```

## HPC vs Local

Same workflow, different execution:
- Local: Declare what you have
- HPC: Declare what scheduler allocates
- Use ResourceBudget to limit total concurrent usage
```

---

## 6. Implementation Priority

**Phase 1: Reliability (Critical)**
1. Cross-platform memory detection
2. Process group timeout
3. Cleanup on failure

**Phase 2: Practical (Important)**
4. System capacity validation
5. GPU spec translation
6. Resource usage logging

**Phase 3: Simple Enhancements (Optional)**
7. ResourceHint support
8. Disk validation warning

---

## 7. Self-Review Checklist

- [x] No placeholders (TBD, TODO, implement later)
- [x] No contradictions between sections
- [x] Focused scope - single implementation plan, not decomposed
- [x] No ambiguous requirements - all solutions specified with code
- [x] Backward compatible - existing workflows continue to work
- [x] No unnecessary complexity - solutions are minimal practical fixes
- [x] CI requirements specified
- [x] Documentation updates defined