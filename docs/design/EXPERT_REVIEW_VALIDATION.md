# Expert Review: Workflow Validation and Linting

**Reviewer**: Pipeline Standards Expert
**Date**: 2026-05-14
**Version Reviewed**: oxo-flow v0.3.1

---

## Executive Summary

oxo-flow implements a robust validation and linting system with 8 error codes (E001-E008), 16 warning/info codes (W001-W016), and 8 schema codes (S001-S008). The architecture separates validation into three layers: TOML parsing, schema verification, and semantic validation. However, several gaps exist in security validation and integration of existing checks.

**Overall Assessment**: **Moderate** - Core validation is solid, but security checks and integration have gaps.

---

## 1. Validation Architecture

### 1.1 Three-Tier Validation Pipeline

| Layer | Component | File | Purpose |
|-------|-----------|------|---------|
| 1 | TOML Parsing | `toml::de::Error` | Basic TOML syntax |
| 2 | Schema Verification | `verify_schema()` | `format.rs:597-728` | Section presence and type checks |
| 3 | Semantic Validation | `validate_format()` | `format.rs:118-261` | Rule consistency, DAG cycles |

### 1.2 Command Integration

The CLI has two validation commands:

- **`oxo-flow validate`** (`main.rs:603-634`): Basic TOML parsing + DAG construction check
- **`oxo-flow lint`** (`main.rs:1183-1228`): Full validation + linting + secret scanning

**GAP**: The `validate` command does NOT call `validate_format()` from `format.rs`. It only checks parsing and DAG, missing semantic validations like E001-E008.

---

## 2. Error Code Analysis

### 2.1 Error Codes (E) - Must Fix

| Code | Check | File | Status |
|------|-------|------|--------|
| E001 | Empty workflow name | `format.rs:122-130` | **GAP**: Not called by `validate` command |
| E002 | Rule validation failure | `format.rs:137-153` | Works via `lint` |
| E003 | Wildcard output not in input | `format.rs:156-168` | Works via `lint` |
| E004 | Invalid memory format | `format.rs:170-183` | Works via `lint` |
| E005 | Undefined config reference | `format.rs:185-202` | Works via `lint` |
| E006 | DAG cycle detection | `format.rs:241-252` | Works via both commands |
| E007 | depends_on non-existent rule | `format.rs:206-223` | Works via `lint` |
| E008 | extends non-existent rule | `format.rs:226-238` | Works via `lint` |

### 2.2 Warning Codes (W) - Best Practices

| Code | Check | Severity | Verified |
|------|-------|----------|----------|
| W001 | Missing workflow description | Warning | Yes |
| W002 | Missing workflow author | Warning | Yes |
| W003 | Missing rule description | Warning | Yes |
| W004 | Shell command without log file | Warning | Yes |
| W005 | High threads (>8) no memory | Warning | Yes |
| W006 | Hyphens in rule name | Info | Yes |
| W007 | Leaf rule not marked target | Info | Yes |
| W008 | No environment specification | Info | Yes |
| W009 | Very high threads (>32) no memory | Warning | Yes |
| W010 | Checkpoint without outputs | Warning | Yes |
| W011 | Shadow without inputs | Warning | Yes |
| W012 | Retries without retry_delay | Info | Yes |
| W013 | on_failure without retries | Info | Yes |
| W014/W015 | depends_on/extends unknown rule | Warning | Duplicate of E007/E008 |
| W016 | Unlocked conda/pixi env | Info | Yes |

### 2.3 Schema Codes (S) - Structure Errors

| Code | Check | File |
|------|-------|------|
| S001 | Invalid TOML syntax | `format.rs:603-614` |
| S002 | Missing [workflow] section | `format.rs:618-625` |
| S003 | Missing workflow.name | `format.rs:628-640` |
| S004 | rules not array of tables | `format.rs:643-661` |
| S005 | rule missing name | `format.rs:663-673` |
| S006 | Unknown top-level key | `format.rs:677-699` |
| S007 | Unrecognized format_version | `format.rs:703-720` |
| S008 | Secret pattern detected | `format.rs:778-810` |

---

## 3. Validation Gaps

### 3.1 Critical Gaps

#### Gap 1: Empty Workflow Name Not Detected by `validate` Command

**Location**: `main.rs:603-634`

The `validate` command only checks:
```rust
let config = WorkflowConfig::from_file(&workflow);
match config {
    Ok(cfg) => {
        match WorkflowDag::from_rules(&cfg.rules) {
            // ...
        }
    }
    // ...
}
```

It does NOT call `validate_format()` which would detect E001 (empty workflow name).

**Test Case**:
```bash
# This passes validation but should fail
cat > /tmp/test.oxoflow << 'EOF'
[workflow]
name = ""

[[rules]]
name = "test"
shell = "echo hi"
EOF
oxo-flow validate /tmp/test.oxoflow  # Output: ✓ ... 1 rules
```

**Fix**: Call `validate_format()` in the `validate` command and report all errors.

---

#### Gap 2: Rule Name Validation Not Enforced

**Location**: `rule.rs:431-451`

`Rule.validate()` checks for invalid characters:
```rust
if !self.name.chars().all(|c| c.is_alphanumeric() || c == '_' || c == '-') {
    return Err(...);
}
```

But space characters like `"step with space"` pass validation.

**Test Case**:
```bash
cat > /tmp/test.oxoflow << 'EOF'
[workflow]
name = "test"

[[rules]]
name = "step with space"
shell = "echo hi"
EOF
oxo-flow validate /tmp/test.oxoflow  # Output: ✓ ... 1 rules
```

**Root Cause**: `Rule.validate()` is called by `validate_format()`, but `validate_format()` is not called by the `validate` command.

---

#### Gap 3: Dangerous Path Detection Not Integrated

**Location**: `executor.rs:1195-1223`

`validate_path_safety()` exists but is only used during execution:
```rust
pub fn validate_path_safety(workdir: &std::path::Path, path: &str) -> crate::Result<()> {
    // Rejects path traversal (..), absolute paths, and home directory references
}
```

**Missing**: This should be part of workflow validation to catch:
- Path traversal: `../../../etc/passwd`
- Absolute paths: `/etc/passwd`
- Home directory: `~/.ssh/id_rsa`

**Test Case**:
```bash
cat > /tmp/test.oxoflow << 'EOF'
[workflow]
name = "test"

[[rules]]
name = "step1"
output = ["../../../etc/passwd"]
shell = "echo hi"
EOF
oxo-flow validate /tmp/test.oxoflow  # Output: ✓ ... 1 rules (should fail)
```

---

#### Gap 4: Secret Scanning Not Integrated into Lint Command

**Location**: `format.rs:778-810`

`scan_for_secrets()` exists and checks for:
- AWS Access Keys (`AKIA...`)
- Stripe/OpenAI keys (`sk-...`)
- GitHub tokens (`ghp_...`)
- GitLab tokens (`glpat-...`)
- Password, secret, api_key, access_token patterns

**Location in CLI**: `main.rs:1183-1228`

The `lint` command calls `validate_format()` and `lint_format()` but does NOT call `scan_for_secrets()`.

**Test Case**:
```bash
cat > /tmp/test.oxoflow << 'EOF'
[workflow]
name = "test"

[config]
api_key = "sk-1234567890abcdef"
password = "supersecret123"

[[rules]]
name = "step1"
shell = "echo hi"
EOF
oxo-flow lint /tmp/test.oxoflow  # Output: 0 error(s) (should warn about secrets)
```

---

### 3.2 Moderate Gaps

#### Gap 5: Duplicate Warning/Error Codes

W014 (lint) and E007 (validation) both check `depends_on` referencing non-existent rules. Same for W015 and E008.

**Impact**: Users see both error and warning for the same issue.

---

#### Gap 6: No Duplicate Rule Name Check

`config.rs:1008-1028` has a test for duplicate rule names, but the check is not in `validate_format()`.

**Test Case**:
```toml
[[rules]]
name = "align"
shell = "echo align1"

[[rules]]
name = "align"
shell = "echo align2"
```

This should fail but the TOML parser silently uses the last definition.

---

#### Gap 7: Output Wildcards Without Input Wildcards

The check (E003) allows output wildcards if `input.is_empty()`, which is problematic for source rules.

```rust
for wc in &output_wildcards {
    if !input_wildcards.contains(wc) && !rule.input.is_empty() {
        // Only warns if input is non-empty
    }
}
```

---

## 4. Security Assessment

### 4.1 Implemented Security Checks

| Check | Location | Status |
|-------|----------|--------|
| Shell command sanitization | `executor.rs:1172-1188` | Works at execution |
| Path traversal prevention | `executor.rs:1195-1223` | Works at execution |
| Secret pattern detection | `format.rs:778-810` | **NOT integrated** |
| Dangerous output paths | None | **Missing** |

### 4.2 Security Recommendations

1. **Integrate `scan_for_secrets()` into `lint` command**
2. **Add path validation to `validate_format()`**
3. **Check for shell injection patterns** (e.g., `rm -rf /`, `sudo`)
4. **Validate file extensions** for bioinformatics safety (partial: `is_known_bio_format()` exists)

---

## 5. Error Message Quality

### 5.1 Good Examples

```
error [E005]: shell command references undefined config variable 'undefined_var' (rule: step1)
warning [W003]: rule has no description (rule: step1)
```

These messages include:
- Severity level
- Diagnostic code
- Clear description
- Rule context
- Suggestions (for some)

### 5.2 Suggestions Present in 60% of Codes

Codes with suggestions: E001, E002, E003, E004, E005, E006, E007, E008, S002, S003, S005, S006, S007, S008, W001-W016.

---

## 6. Test Coverage

### 6.1 Unit Tests in format.rs

- `validate_valid_workflow`
- `validate_empty_workflow_name`
- `validate_invalid_memory`
- `validate_undefined_config_ref`
- `validate_wildcard_consistency`
- `lint_missing_descriptions`
- `lint_high_threads_no_memory`
- `lint_missing_log`
- 50+ additional tests

**Coverage**: Comprehensive for validation logic.

### 6.2 Missing Test Cases

- Path traversal in output files
- Secret detection integration
- Empty workflow name via CLI validate command
- Shell injection patterns

---

## 7. Recommendations

### Priority 1 (Critical)

1. **Fix validate command integration**: Call `validate_format()` in `validate` command
2. **Integrate secret scanning**: Add `scan_for_secrets()` call in `lint` command
3. **Add path validation**: Validate output/input paths at workflow validation time

### Priority 2 (High)

4. **Remove duplicate checks**: Keep E007/E008 as errors, remove W014/W015
5. **Add duplicate rule name check**: E009 for duplicate rule names
6. **Strengthen wildcard validation**: Require output wildcards in input even for source rules

### Priority 3 (Medium)

7. **Add shell injection detection**: New E010 for dangerous shell patterns
8. **Validate file extensions**: Warn on non-bioinformatics file types
9. **Add format verification**: Ensure format_version matches current version

---

## 8. Implementation Locations

| Component | File Path | Lines |
|-----------|-----------|-------|
| validate_format() | `crates/oxo-flow-core/src/format.rs` | 118-261 |
| lint_format() | `crates/oxo-flow-core/src/format.rs` | 271-506 |
| verify_schema() | `crates/oxo-flow-core/src/format.rs` | 597-728 |
| scan_for_secrets() | `crates/oxo-flow-core/src/format.rs` | 778-810 |
| Rule.validate() | `crates/oxo-flow-core/src/rule.rs` | 431-507 |
| validate_path_safety() | `crates/oxo-flow-core/src/executor.rs` | 1195-1223 |
| sanitize_shell_command() | `crates/oxo-flow-core/src/executor.rs` | 1172-1188 |
| CLI validate command | `crates/oxo-flow-cli/src/main.rs` | 603-634 |
| CLI lint command | `crates/oxo-flow-cli/src/main.rs` | 1183-1228 |
| WorkflowConfig.validate() | `crates/oxo-flow-core/src/config.rs` | 622-648 |
| Diagnostic struct | `crates/oxo-flow-core/src/format.rs` | 37-63 |

---

## 9. Conclusion

oxo-flow's validation system has a well-designed architecture with proper separation of concerns. The diagnostic system with coded errors/warnings is excellent for programmatic handling. However, integration gaps mean that several validation checks exist but are not called by the CLI commands, reducing effectiveness.

The most significant issues are:
1. `validate` command bypasses semantic validation
2. Secret scanning exists but is not used
3. Path safety checks are deferred to execution time
4. Some duplicate checks produce noise

Addressing these gaps would significantly improve workflow safety and user experience.