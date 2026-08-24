# Non-Shell Language Support and Documentation Completeness

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Implement `envvars` injection, `script` field with auto-interpreter detection, and complete workflow-format.md documentation for all missing Rule fields.

**Architecture:** Minimal enhancements to executor.rs for envvars injection and script handling; documentation updates to workflow-format.md.

**Tech Stack:** Rust, existing Rule struct fields, TOML serde.

---

## 1. Current State Analysis

### Implemented but Unused Fields

| Field | Code Exists | Executor Uses | Documented |
|-------|-------------|---------------|------------|
| `script` | ✓ (rule.rs:369) | ❌ | ❌ |
| `envvars` | ✓ (rule.rs:505) | ❌ | ❌ |
| `on_success` | ✓ (rule.rs:559) | ✓ (executor.rs:793) | ❌ |
| `on_failure` | ✓ (rule.rs:566) | ✓ (executor.rs:849) | ❌ |
| `retries` | ✓ (rule.rs:477) | ✓ | ❌ |
| `retry_delay` | ✓ (rule.rs:543) | ✓ | ❌ |
| `temp_output` | ✓ (rule.rs:459) | ✓ (executor.rs:478) | ❌ |

### Documentation Gaps

workflow-format.md documents ~15 Rule fields but omits ~20+ additional fields that are implemented and functional.

---

## 2. Proposed Solutions

### 2.1 `envvars` Injection

**Problem:** `envvars` field defined in Rule but never injected into command execution.

**Solution:** Inject environment variables before shell command:

```toml
[[rules]]
name = "gpu_task"
shell = "python train.py"

[rules.envvars]
CUDA_VISIBLE_DEVICES = "0,1"
PYTHONPATH = "./src"
```

Executes as:
```bash
CUDA_VISIBLE_DEVICES=0,1 PYTHONPATH=./src python train.py
```

**Implementation:**
- In `executor.rs`, before `Command::new("sh")`, build env prefix from `rule.envvars`
- Support both inline (`envvars = { A = "1" }`) and table (`[rules.envvars]`) formats
- Support placeholder expansion in envvar values: `{config.reference}` → actual value

### 2.2 `script` Field with Auto-Interpreter Detection

**Problem:** `script` field defined but executor only uses `shell`.

**Solution:**
1. When both `shell` and `script` exist: execute shell first, then script (sequential)
2. When only `script` exists: auto-detect interpreter from file extension
3. Optional `interpreter` field for explicit override

**Auto-Detection Rules:**

| Extension | Interpreter | Notes |
|-----------|-------------|-------|
| `.py` | `python` | Python script |
| `.R` / `.r` | `Rscript` | R script |
| `.jl` | `julia` | Julia script |
| `.sh` / `.bash` | `bash` | Shell script |
| `.pl` | `perl` | Perl script |
| `.rb` | `ruby` | Ruby script |
| `.qmd` | `quarto render` | Quarto document |
| `.Rmd` / `.rmd` | `quarto render` | R Markdown |
| `.ipynb` | `jupyter nbconvert --to notebook --execute` | Jupyter notebook |
| `.smk` | `snakemake` | Snakemake workflow |
| `.nextflow` | `nextflow run` | Nextflow script |
| `.wdl` | `miniwdl run` | WDL workflow |
| other/none | `interpreter` field required | Explicit override |

**User-Configurable Interpreter Map:**

```toml
[workflow]
name = "pipeline"

[workflow.interpreter_map]
".m" = "octave"        # MATLAB/Octave
".sas" = "sas"         # SAS
".do" = "stata-mp"     # Stata
".stan" = "cmdstan"    # Stan
".py3" = "python3"     # Custom extension
```

### 2.3 Sequential Shell + Script Execution

When both `shell` and `script` are defined:

```toml
[[rules]]
name = "qc_and_report"
shell = "fastqc {input} -o qc/"
script = "reports/qc_report.qmd"
```

Execution order:
1. `fastqc data.fastq -o qc/` (shell)
2. `quarto render reports/qc_report.qmd` (script)

Combined with envvars:
```bash
# Step 1
CUDA_VISIBLE_DEVICES=0,1 fastqc data.fastq -o qc/

# Step 2
CUDA_VISIBLE_DEVICES=0,1 quarto render reports/qc_report.qmd
```

### 2.4 Documentation Completeness

Add to `workflow-format.md`:

**New Rule Fields Section:**

```markdown
### Script Execution

| Field | Type | Description |
|-------|------|-------------|
| `script` | String | Script file path (auto-detects interpreter from extension) |
| `interpreter` | String | Explicit interpreter override (e.g., `"/opt/python3.11/bin/python") |

When both `shell` and `script` are defined, they execute sequentially: shell first, then script.

### Environment Variables

| Field | Type | Description |
|-------|------|-------------|
| `envvars` | Table | Rule-level environment variables injected before execution |

Inline format:
```toml
envvars = { CUDA_VISIBLE_DEVICES = "0,1" }
```

Table format:
```toml
[rules.envvars]
CUDA_VISIBLE_DEVICES = "0,1"
PYTHONPATH = "./src"
```

### Lifecycle Hooks

| Field | Type | Description |
|-------|------|-------------|
| `on_success` | String | Shell command executed after successful completion |
| `on_failure` | String | Shell command executed after all retries exhausted |

### Retry Configuration

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `retries` | Integer | 0 | Number of automatic retry attempts |
| `retry_delay` | String | — | Delay between retries (`"5s"`, `"30s"`, `"2m"`) |

### Output Management

| Field | Type | Description |
|-------|------|-------------|
| `temp_output` | Array | Temporary outputs cleaned after downstream rules complete |
| `protected_output` | Array | Protected outputs never overwritten or deleted |

### Execution Control

| Field | Type | Description |
|-------|------|-------------|
| `depends_on` | Array | Explicit rule dependencies (not inferred from files) |
| `localrule` | Boolean | Always run locally, never submit to cluster |
| `workdir` | String | Per-rule working directory override |
| `checkpoint` | Boolean | Enable dynamic DAG modification |
| `shadow` | String | Atomic execution mode: `"minimal"`, `"shallow"`, `"full"` |

### Input/Output Hints

| Field | Type | Description |
|-------|------|-------------|
| `ancient` | Array | Inputs that never trigger re-execution |
| `format_hint` | Array | File format hints for I/O optimization |
| `pipe` | Boolean | Enable FIFO streaming mode for inputs |
| `checksum` | String | Output checksum algorithm (`"md5"`, `"sha256"`) |

### Organization

| Field | Type | Description |
|-------|------|-------------|
| `tags` | Array | Categorization tags (`["qc", "alignment"]`) |
| `extends` | String | Base rule to inherit settings from |
```

**New Workflow-Level Configuration:**

```markdown
### Interpreter Configuration

Configure custom interpreter mappings:

```toml
[workflow.interpreter_map]
".m" = "octave"
".sas" = "sas"
".do" = "stata-mp"
```

Overrides default auto-detection for specified file extensions.
```

---

## 3. Files to Modify

| File | Changes |
|------|---------|
| `crates/oxo-flow-core/src/config.rs` | Add `interpreter_map` field to WorkflowConfig |
| `crates/oxo-flow-core/src/executor.rs` | Implement envvars injection, script execution, interpreter detection |
| `crates/oxo-flow-core/src/rule.rs` | Add `interpreter` field to Rule (optional) |
| `docs/guide/src/reference/workflow-format.md` | Add all missing Rule fields documentation |

---

## 4. Testing Requirements

### Unit Tests

| Test | Purpose |
|------|---------|
| `envvars_injection_inline` | Inline format envvars injected correctly |
| `envvars_injection_table` | Table format envvars injected correctly |
| `envvars_placeholder_expansion` | `{config.X}` expanded in envvar values |
| `script_py_detection` | `.py` → `python` |
| `script_R_detection` | `.R` → `Rscript` |
| `script_qmd_detection` | `.qmd` → `quarto render` |
| `script_interpreter_override` | `interpreter` field overrides auto-detection |
| `shell_then_script_execution` | Both defined: sequential execution |
| `custom_interpreter_map` | Workflow-level mapping overrides default |

### Integration Tests

| Test | Purpose |
|------|---------|
| `envvars_with_conda_env` | Envvars + conda environment work together |
| `script_with_envvars` | Script execution receives envvars |
| `quarto_render_rule` | Full `.qmd` execution flow |

---

## 5. Implementation Priority

**Phase 1: Core Features (Critical)**
1. `envvars` injection in executor
2. `script` field execution with auto-detection
3. `interpreter` field and workflow-level `interpreter_map`

**Phase 2: Documentation (Important)**
4. Complete workflow-format.md with all missing fields

**Phase 3: Extended Interpreters (Optional)**
5. Additional bioinformatics formats (`.smk`, `.nextflow`, `.wdl`)
6. Quarto/Jupyter integration testing

---

## 6. Self-Review Checklist

- [x] No placeholders (TBD, TODO)
- [x] No contradictions between sections
- [x] Focused scope - single implementation plan
- [x] All solutions specified with concrete code/command examples
- [x] Backward compatible - existing workflows continue to work
- [x] Tests specified for each feature
- [x] Documentation updates defined