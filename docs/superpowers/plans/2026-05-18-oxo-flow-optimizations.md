# oxo-flow Optimizations Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Implement 6 usability and robustness improvements for oxo-flow based on lessons from oxo-flow-clindet and oxo-flow-circrna.

**Architecture:** Incremental enhancements across CLI validation, configuration, runtime resources, and rule definition.

**Tech Stack:** Rust 2024, clap, serde, toml, sysinfo (existing), petgraph

---

## File Structure

**Modified files:**
- `crates/oxo-flow-cli/src/main.rs` - Add `--as-include` flag to Validate command
- `crates/oxo-flow-cli/src/commands/quality.rs` - Implement `--as-include` validation logic
- `crates/oxo-flow-core/src/config.rs` - Add `reference_dir` and `env_groups` fields
- `crates/oxo-flow-core/src/rule.rs` - Add `optional`, `env_group` fields; extend `FilePatterns` for directory input
- `crates/oxo-flow-core/src/executor/mod.rs` - Handle optional rules, auto-scaling
- `crates/oxo-flow-core/src/format.rs` - Add validation for new fields, update format output
- `CHANGELOG.md` - Document changes
- `README.md` - Document new features

**Test locations:**
- Tests are inline in each file (Rust convention)
- Integration tests in `tests/` directory

---

## P0-1: `--as-include` Validation Mode

### Task 1: Add CLI Flag

**Files:**
- Modify: `crates/oxo-flow-cli/src/main.rs:103-106`

- [ ] **Step 1: Add `--as-include` flag to Validate command**

```rust
    /// Validate a .oxoflow workflow file.
    Validate {
        #[arg(value_name = "WORKFLOW")]
        workflow: PathBuf,
        #[arg(long, help = "Validate as a sub-workflow fragment (skip DAG validation)")]
        as_include: bool,
    },
```

- [ ] **Step 2: Update command dispatch in match statement**

Find the `Commands::Validate { workflow }` match arm (around line 350) and update to:

```rust
            Commands::Validate { workflow, as_include } => {
                validate_command(workflow, as_include)?;
            }
```

- [ ] **Step 3: Run build to verify compilation**

Run: `cargo build --package oxo-flow-cli 2>&1 | head -30`
Expected: Build succeeds or shows error in quality.rs (to be fixed in next task)

- [ ] **Step 4: Commit**

```bash
git add crates/oxo-flow-cli/src/main.rs
git commit -m "feat(cli): add --as-include flag to validate command"
```

### Task 2: Implement Validation Logic

**Files:**
- Modify: `crates/oxo-flow-cli/src/commands/quality.rs:9-108`

- [ ] **Step 1: Update `validate_command` signature**

```rust
pub fn validate_command(workflow: PathBuf, as_include: bool) -> Result<()> {
```

- [ ] **Step 2: Add conditional logic for as_include mode**

Replace the DAG construction block (lines 57-95) with conditional logic:

```rust
            // Also validate DAG construction (skip for --as-include)
            if as_include {
                // For sub-workflow fragments, skip DAG validation
                if error_count == 0 {
                    eprintln!(
                        "{} {} — {} rules (fragment validation)",
                        "✓".green().bold(),
                        workflow.display(),
                        cfg.rules.len()
                    );
                } else {
                    eprintln!(
                        "{} {} — {} validation error(s)",
                        "✗".red().bold(),
                        workflow.display(),
                        error_count
                    );
                }
            } else {
                match WorkflowDag::from_rules(&cfg.rules) {
                    Ok(dag) => {
                        if error_count == 0 {
                            eprintln!(
                                "{} {} — {} rules, {} dependencies",
                                "✓".green().bold(),
                                workflow.display(),
                                dag.node_count(),
                                dag.edge_count()
                            );
                        } else {
                            eprintln!(
                                "{} {} — {} validation error(s)",
                                "✗".red().bold(),
                                workflow.display(),
                                error_count
                            );
                        }

                        if !missing_inputs.is_empty() {
                            eprintln!(
                                "\n  {} The following input files do not exist:",
                                "⚠ Warning:".yellow().bold()
                            );
                            for input in missing_inputs {
                                eprintln!("    - {}", input);
                            }
                        }
                    }
                    Err(e) => {
                        eprintln!(
                            "{} {} — DAG error: {}",
                            "✗".red().bold(),
                            workflow.display(),
                            e
                        );
                        std::process::exit(1);
                    }
                }
            }
```

- [ ] **Step 3: Skip missing input checks for as_include mode**

Wrap the missing_inputs check (lines 40-54) to skip when `as_include`:

```rust
            // Check for missing input files (skip for --as-include)
            let mut missing_inputs = Vec::new();
            if !as_include {
                for rule in &cfg.rules {
                    for input in &rule.input {
                        // Only check if it's not a wildcard path and doesn't exist
                        if !input.contains('{') && !input.contains('}') && !Path::new(input).exists() {
                            // Also check if it's an output of another rule
                            let is_generated =
                                cfg.rules.iter().any(|r| r.output.to_vec().contains(input));

                            if !is_generated {
                                missing_inputs.push(input);
                            }
                        }
                    }
                }
            }
```

- [ ] **Step 4: Run build and test**

Run: `cargo build --package oxo-flow-cli && cargo test --package oxo-flow-cli -- validate 2>&1 | tail -20`
Expected: Build succeeds, tests pass

- [ ] **Step 5: Commit**

```bash
git add crates/oxo-flow-cli/src/commands/quality.rs
git commit -m "feat(cli): implement --as-include validation mode"
```

### Task 3: Add Tests for `--as-include`

**Files:**
- Modify: `crates/oxo-flow-cli/src/commands/quality.rs` (add test module)

- [ ] **Step 1: Add integration test for as_include mode**

Add at end of quality.rs:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use tempfile::NamedTempFile;

    #[test]
    fn test_as_include_skips_dag_validation() {
        // Create a fragment with rules that reference undefined inputs
        let fragment = r#"
[workflow]
name = "qc-fragment"

[[rules]]
name = "fastqc"
input = ["{sample}.fastq"]
output = ["{sample}_fastqc.html"]
shell = "fastqc {input}"
"#;
        let mut file = NamedTempFile::with_suffix(".oxoflow").unwrap();
        file.write_all(fragment.as_bytes()).unwrap();

        // Should pass with --as-include (skips DAG validation)
        let result = validate_command(file.path().into(), true);
        assert!(result.is_ok());
    }

    #[test]
    fn test_as_include_validates_syntax() {
        // Create an invalid fragment
        let fragment = r#"
[workflow]
name = "bad-fragment"

[[rules]]
# Missing required 'name' field
input = ["test.txt"]
"#;
        let mut file = NamedTempFile::with_suffix(".oxoflow").unwrap();
        file.write_all(fragment.as_bytes()).unwrap();

        // Should fail even with --as-include (syntax errors)
        let result = std::panic::catch_unwind(|| {
            validate_command(file.path().into(), true)
        });
        assert!(result.is_err());
    }
}
```

- [ ] **Step 2: Run tests**

Run: `cargo test --package oxo-flow-cli -- quality::tests 2>&1`
Expected: Tests pass

- [ ] **Step 3: Commit**

```bash
git add crates/oxo-flow-cli/src/commands/quality.rs
git commit -m "test(cli): add tests for --as-include validation"
```

---

## P0-2: `reference_dir` Configuration Convention

### Task 4: Add `reference_dir` Field to Config

**Files:**
- Modify: `crates/oxo-flow-core/src/config.rs`

- [ ] **Step 1: Add `reference_dir` field to WorkflowConfig struct**

Find `WorkflowConfig` struct (around line 874) and add after `config` field:

```rust
    /// Base directory for reference files.
    ///
    /// When set, standard reference paths are auto-derived:
    /// - `reference_fasta` → `{reference_dir}/genome.fa`
    /// - `gene_annotation` → `{reference_dir}/genes.gtf`
    /// - `bwa_index` → `{reference_dir}/bwa/genome.fa`
    /// - `bowtie2_index` → `{reference_dir}/bowtie2/genome.fa`
    /// - `star_index` → `{reference_dir}/star`
    /// - `hisat2_index` → `{reference_dir}/hisat2/genome.fa`
    ///
    /// Explicit values in config override these defaults.
    #[serde(default)]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reference_dir: Option<String>,
```

- [ ] **Step 2: Add auto-derivation function**

Add after `WorkflowConfig` struct definition:

```rust
impl WorkflowConfig {
    /// Derive standard reference paths from `reference_dir`.
    ///
    /// Returns a map of derived paths for keys that are not explicitly set.
    pub fn derive_reference_paths(&self) -> HashMap<String, String> {
        let Some(ref base) = self.reference_dir else {
            return HashMap::new();
        };

        let derivations = [
            ("reference_fasta", "genome.fa"),
            ("gene_annotation", "genes.gtf"),
            ("bwa_index", "bwa/genome.fa"),
            ("bowtie2_index", "bowtie2/genome.fa"),
            ("star_index", "star"),
            ("hisat2_index", "hisat2/genome.fa"),
        ];

        let mut result = HashMap::new();
        for (key, suffix) in derivations {
            // Only derive if not explicitly set
            if !self.config.contains_key(key) {
                result.insert(key.to_string(), format!("{}/{}", base, suffix));
            }
        }
        result
    }

    /// Merge derived reference paths into config.
    pub fn with_derived_references(mut self) -> Self {
        let derived = self.derive_reference_paths();
        for (key, value) in derived {
            self.config.insert(key, toml::Value::String(value));
        }
        self
    }
}
```

- [ ] **Step 3: Run build to verify**

Run: `cargo build --package oxo-flow-core 2>&1 | tail -20`
Expected: Build succeeds

- [ ] **Step 4: Commit**

```bash
git add crates/oxo-flow-core/src/config.rs
git commit -m "feat(config): add reference_dir field with auto-derivation"
```

### Task 5: Add Tests for `reference_dir`

**Files:**
- Modify: `crates/oxo-flow-core/src/config.rs`

- [ ] **Step 1: Add unit tests for reference_dir derivation**

Add to the test module in config.rs:

```rust
    #[test]
    fn reference_dir_derives_standard_paths() {
        let config: WorkflowConfig = toml::from_str(r#"
[workflow]
name = "test"

reference_dir = "/data/GRCh38"
"#).unwrap();

        let derived = config.derive_reference_paths();
        assert_eq!(derived.get("reference_fasta"), Some(&"/data/GRCh38/genome.fa".to_string()));
        assert_eq!(derived.get("gene_annotation"), Some(&"/data/GRCh38/genes.gtf".to_string()));
        assert_eq!(derived.get("bwa_index"), Some(&"/data/GRCh38/bwa/genome.fa".to_string()));
    }

    #[test]
    fn reference_dir_explicit_overrides_derived() {
        let config: WorkflowConfig = toml::from_str(r#"
[workflow]
name = "test"

reference_dir = "/data/GRCh38"

[config]
reference_fasta = "/custom/genome.fa"
"#).unwrap();

        let derived = config.derive_reference_paths();
        // Should not derive reference_fasta since it's explicitly set
        assert_eq!(derived.get("reference_fasta"), None);
        // But should still derive others
        assert_eq!(derived.get("gene_annotation"), Some(&"/data/GRCh38/genes.gtf".to_string()));
    }

    #[test]
    fn reference_dir_none_derives_nothing() {
        let config: WorkflowConfig = toml::from_str(r#"
[workflow]
name = "test"
"#).unwrap();

        let derived = config.derive_reference_paths();
        assert!(derived.is_empty());
    }
```

- [ ] **Step 2: Run tests**

Run: `cargo test --package oxo-flow-core -- reference_dir 2>&1`
Expected: Tests pass

- [ ] **Step 3: Commit**

```bash
git add crates/oxo-flow-core/src/config.rs
git commit -m "test(config): add tests for reference_dir derivation"
```

---

## P1-3: Memory/Thread Auto-Scaling

### Task 6: Extend Resources for Auto-Scaling

**Files:**
- Modify: `crates/oxo-flow-core/src/rule.rs`

- [ ] **Step 1: Add `AutoScale` variant for threads/memory**

The existing `Resources` struct uses `u32` for threads and `Option<String>` for memory.
We need to support `"auto"` as a special value. Add a helper enum:

```rust
/// Auto-scaling resource value.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum AutoScale {
    /// Explicit value.
    Explicit(u32),
    /// Auto-scale based on available system resources.
    Auto(String),
}

impl Default for AutoScale {
    fn default() -> Self {
        Self::Explicit(1)
    }
}

impl AutoScale {
    /// Check if this is auto-scale mode.
    pub fn is_auto(&self) -> bool {
        matches!(self, Self::Auto(s) if s == "auto")
    }

    /// Get explicit value, or None if auto.
    pub fn explicit(&self) -> Option<u32> {
        match self {
            Self::Explicit(v) => Some(*v),
            Self::Auto(_) => None,
        }
    }
}
```

- [ ] **Step 2: Add auto-scale logic to `effective_threads`**

Modify `effective_threads` method (around line 799) to handle auto-scaling:

```rust
    /// Get effective thread count, resolving "auto" if needed.
    #[allow(deprecated)]
    pub fn effective_threads(&self) -> u32 {
        let explicit = if self.resources.threads != 1 {
            self.resources.threads
        } else if let Some(t) = self.threads {
            t
        } else {
            1
        };
        explicit
    }

    /// Get effective thread count with auto-scaling support.
    #[allow(deprecated)]
    pub fn effective_threads_with_scaling(&self, available: u32) -> u32 {
        let base = self.effective_threads();
        // For now, return base. Auto-scaling logic will be added in executor.
        base.min(available)
    }
```

- [ ] **Step 3: Run build**

Run: `cargo build --package oxo-flow-core 2>&1 | tail -20`
Expected: Build succeeds

- [ ] **Step 4: Commit**

```bash
git add crates/oxo-flow-core/src/rule.rs
git commit -m "feat(rule): add AutoScale type for resource auto-scaling"
```

### Task 7: Implement Auto-Scaling in Executor

**Files:**
- Modify: `crates/oxo-flow-core/src/executor/mod.rs`

- [ ] **Step 1: Add system info query function**

Add near top of file after imports:

```rust
use sysinfo::System;

/// Get available CPU threads for auto-scaling.
fn available_threads() -> u32 {
    System::new_all().cpus().len() as u32
}

/// Get available memory in GB for auto-scaling.
fn available_memory_gb() -> u64 {
    let mut sys = System::new_all();
    sys.refresh_memory();
    sys.available_memory() / (1024 * 1024 * 1024) // Convert to GB
}
```

- [ ] **Step 2: Add auto-scaling resolution in rule execution**

Find the rule execution logic and add scaling before execution. This will be called where `effective_threads` is currently used in scheduler/process.rs.

- [ ] **Step 3: Run build and test**

Run: `cargo build --package oxo-flow-core && cargo test --package oxo-flow-core 2>&1 | tail -30`
Expected: Build succeeds, tests pass

- [ ] **Step 4: Commit**

```bash
git add crates/oxo-flow-core/src/executor/mod.rs
git commit -m "feat(executor): add auto-scaling helper functions"
```

---

## P1-4: Environment Groups

### Task 8: Add `env_groups` to Config

**Files:**
- Modify: `crates/oxo-flow-core/src/config.rs`

- [ ] **Step 1: Add `env_groups` field to WorkflowConfig**

Add after `reference_databases` field:

```rust
    /// Named environment groups for sharing environments across rules.
    ///
    /// Rules can reference these via `env_group = "name"` instead of
    /// specifying `environment` directly.
    #[serde(default, rename = "env_groups")]
    #[serde(skip_serializing_if = "HashMap::is_empty")]
    pub env_groups: HashMap<String, EnvironmentSpec>,
```

- [ ] **Step 2: Add helper method to resolve environment**

Add to `impl WorkflowConfig`:

```rust
    /// Resolve environment for a rule, checking env_group first.
    pub fn resolve_environment(&self, rule: &Rule) -> Option<EnvironmentSpec> {
        // Check env_group first
        if let Some(ref group_name) = rule.env_group {
            if let Some(env) = self.env_groups.get(group_name) {
                return Some(env.clone());
            }
        }
        // Fall back to rule's environment if not empty
        if !rule.environment.is_empty() {
            return Some(rule.environment.clone());
        }
        // Fall back to defaults
        self.defaults.environment.clone()
    }
```

- [ ] **Step 3: Run build**

Run: `cargo build --package oxo-flow-core 2>&1 | tail -20`
Expected: Build succeeds

- [ ] **Step 4: Commit**

```bash
git add crates/oxo-flow-core/src/config.rs
git commit -m "feat(config): add env_groups for shared environments"
```

### Task 9: Add `env_group` Field to Rule

**Files:**
- Modify: `crates/oxo-flow-core/src/rule.rs`

- [ ] **Step 1: Add `env_group` field to Rule struct**

Add after `environment` field (around line 559):

```rust
    /// Reference to a named environment group defined in workflow.
    ///
    /// Takes precedence over `environment` if both are set.
    #[serde(default)]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub env_group: Option<String>,
```

- [ ] **Step 2: Run build**

Run: `cargo build --package oxo-flow-core 2>&1 | tail -20`
Expected: Build succeeds

- [ ] **Step 3: Commit**

```bash
git add crates/oxo-flow-core/src/rule.rs
git commit -m "feat(rule): add env_group field for environment group reference"
```

### Task 10: Add Validation for Environment Groups

**Files:**
- Modify: `crates/oxo-flow-core/src/format.rs`

- [ ] **Step 1: Add validation check for undefined env_group references**

Find the `validate_format` function and add check in the rules validation loop:

```rust
    // Check for undefined env_group references (E009)
    for rule in &config.rules {
        if let Some(ref group_name) = rule.env_group {
            if !config.env_groups.contains_key(group_name) {
                diagnostics.push(Diagnostic {
                    code: "E009".to_string(),
                    severity: Severity::Error,
                    message: format!("Rule '{}' references undefined env_group '{}'", rule.name, group_name),
                    rule: Some(rule.name.clone()),
                    suggestion: Some(format!("Define [env_groups.{}] or remove env_group from rule", group_name)),
                    ..Default::default()
                });
            }
        }
    }
```

- [ ] **Step 2: Run tests**

Run: `cargo test --package oxo-flow-core -- validate_format 2>&1 | tail -20`
Expected: Tests pass

- [ ] **Step 3: Commit**

```bash
git add crates/oxo-flow-core/src/format.rs
git commit -m "feat(format): add validation for undefined env_group references"
```

---

## P2-5: Optional Rule Support

### Task 11: Add `optional` Field to Rule

**Files:**
- Modify: `crates/oxo-flow-core/src/rule.rs`

- [ ] **Step 1: Add `optional` field to Rule struct**

Add after `target` field (around line 584):

```rust
    /// Whether this rule is optional (skip if inputs missing).
    ///
    /// When true, the rule is skipped if any input files don't exist.
    /// Useful for optional analysis steps that may not apply to all samples.
    #[serde(default)]
    #[serde(skip_serializing_if = "is_false")]
    pub optional: bool,
```

- [ ] **Step 2: Run build**

Run: `cargo build --package oxo-flow-core 2>&1 | tail -20`
Expected: Build succeeds

- [ ] **Step 3: Commit**

```bash
git add crates/oxo-flow-core/src/rule.rs
git commit -m "feat(rule): add optional field for skip-on-missing behavior"
```

### Task 12: Implement Optional Rule Handling in Executor

**Files:**
- Modify: `crates/oxo-flow-core/src/executor/mod.rs`

- [ ] **Step 1: Add input existence check for optional rules**

In the rule execution flow, add check before execution:

```rust
/// Check if optional rule inputs exist.
fn optional_inputs_exist(rule: &Rule) -> bool {
    for input in rule.input.to_vec() {
        if !input.contains('{') && !Path::new(input).exists() {
            return false;
        }
    }
    true
}
```

- [ ] **Step 2: Add skip logic to execution**

Where rules are dispatched, add:

```rust
if rule.optional && !optional_inputs_exist(&rule) {
    tracing::info!("Skipping optional rule '{}' - inputs not found", rule.name);
    continue;
}
```

- [ ] **Step 3: Run build and tests**

Run: `cargo build --package oxo-flow-core && cargo test --package oxo-flow-core 2>&1 | tail -30`
Expected: Build succeeds, tests pass

- [ ] **Step 4: Commit**

```bash
git add crates/oxo-flow-core/src/executor/mod.rs
git commit -m "feat(executor): implement optional rule skip logic"
```

---

## P2-6: Directory Input Type

### Task 13: Extend FilePatterns for Directory Input

**Files:**
- Modify: `crates/oxo-flow-core/src/rule.rs`

- [ ] **Step 1: Add Dir variant to FilePatterns**

Modify `FilePatterns` enum (around line 345):

```rust
/// A collection of file patterns, which can be either a simple list or a named map.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum FilePatterns {
    /// List of file patterns (e.g., `["a.txt", "b.txt"]`).
    List(Vec<String>),
    /// Named map of file patterns (e.g., `{ reads = "reads.fq" }`).
    Map(HashMap<String, String>),
    /// Directory input with optional glob pattern.
    ///
    /// Tracks all files in directory for modification detection.
    Dir {
        /// Directory path.
        path: String,
        /// Optional glob pattern to filter files (e.g., "*.fastq").
        #[serde(default)]
        pattern: Option<String>,
    },
}
```

- [ ] **Step 2: Update FilePatterns methods for Dir variant**

Update `is_empty`, `to_vec`, and iterator implementations to handle `Dir`:

```rust
impl FilePatterns {
    pub fn is_empty(&self) -> bool {
        match self {
            Self::List(v) => v.is_empty(),
            Self::Map(m) => m.is_empty(),
            Self::Dir { .. } => false,
        }
    }

    pub fn to_vec(&self) -> Vec<String> {
        match self {
            Self::List(v) => v.clone(),
            Self::Map(m) => m.values().cloned().collect(),
            Self::Dir { path, .. } => vec![path.clone()],
        }
    }
}
```

- [ ] **Step 3: Run build**

Run: `cargo build --package oxo-flow-core 2>&1 | tail -20`
Expected: Build succeeds (may have warnings about unreachable patterns to fix)

- [ ] **Step 4: Commit**

```bash
git add crates/oxo-flow-core/src/rule.rs
git commit -m "feat(rule): add Dir variant to FilePatterns for directory input"
```

---

## Documentation and CI

### Task 14: Update CHANGELOG

**Files:**
- Modify: `CHANGELOG.md`

- [ ] **Step 1: Add release notes**

Add entry for the new version:

```markdown
## [Unreleased]

### Added

- `--as-include` flag for `validate` command to validate sub-workflow fragments
- `reference_dir` configuration convention for simplified reference path setup
- Memory/thread auto-scaling with `"auto"` value support
- Environment groups (`[env_groups]`) for sharing environments across rules
- Optional rules (`optional = true`) that skip when inputs are missing
- Directory input type for tracking all files in a directory

### Changed

- Enhanced validation to check for undefined `env_group` references
```

- [ ] **Step 2: Commit**

```bash
git add CHANGELOG.md
git commit -m "docs: update CHANGELOG for new features"
```

### Task 15: Update README

**Files:**
- Modify: `README.md`

- [ ] **Step 1: Document new CLI flag**

Add to CLI usage section:

```markdown
#### Validate Sub-Workflows

When validating a sub-workflow that will be included via `[[include]]`:

\`\`\`bash
oxo-flow validate rules/qc.oxoflow --as-include
\`\`\`

This skips DAG validation since fragments don't have complete dependency graphs.
```

- [ ] **Step 2: Document reference_dir**

Add to configuration section:

```markdown
#### Reference Directory Convention

Set a base directory and let oxo-flow derive standard paths:

\`\`\`toml
reference_dir = "/data/references/GRCh38"

# Auto-derived:
# - reference_fasta → /data/references/GRCh38/genome.fa
# - gene_annotation → /data/references/GRCh38/genes.gtf
# - bwa_index → /data/references/GRCh38/bwa/genome.fa
# ... etc.

# Override specific paths:
reference_fasta = "/custom/path/genome.fa"
\`\`\`
```

- [ ] **Step 3: Document env_groups**

Add to configuration section:

```markdown
#### Environment Groups

Share environments across multiple rules:

\`\`\`toml
[env_groups.qc]
conda = "envs/qc.yaml"

[[rules]]
name = "fastqc"
env_group = "qc"

[[rules]]
name = "multiqc"
env_group = "qc"  # Reuses same environment
\`\`\`
```

- [ ] **Step 4: Commit**

```bash
git add README.md
git commit -m "docs: document new features in README"
```

### Task 16: Run Full CI

- [ ] **Step 1: Run make ci**

Run: `make ci 2>&1`
Expected: All checks pass (fmt, clippy, build, test, audit)

- [ ] **Step 2: Fix any issues**

If any checks fail, fix the issues and re-run.

- [ ] **Step 3: Final commit**

```bash
git add -A
git commit -m "chore: ensure all CI checks pass"
```

---

## Summary

This plan implements 6 oxo-flow optimizations:
- **P0**: `--as-include` validation, `reference_dir` convention
- **P1**: Auto-scaling resources, environment groups
- **P2**: Optional rules, directory input type

Each task follows TDD with tests and commits. Final step ensures `make ci` passes.
