# Diagnostics Engine

> Deterministic failure analysis — no AI, pure pattern matching.
> Every diagnosis is reproducible given the same inputs.

## Overview

The Diagnostics Engine analyzes failed pipeline runs and returns:
- **Error pattern** identified (e.g., OOM, command not found, file missing)
- **Likely cause** with evidence
- **Fix suggestions** (auto-fixable or manual)
- **Relevant log lines** for context

It does NOT use AI. It matches error signatures (regex patterns, exit codes,
log keywords) against a curated library of 30+ error patterns.

## Error Pattern Library

### Tool Errors

| Pattern | Signature | Auto-Fix |
|---------|-----------|----------|
| Command not found | exit 127, "command not found" | ✅ Suggest `conda install` |
| Version incompatibility | "version X required, found Y" | ❌ Suggest version switch |
| Tool crash (SIGSEGV) | exit 139, "Segmentation fault" | ❌ Suggest tool update |
| Tool crash (SIGILL) | exit 132, "Illegal instruction" | ❌ Check CPU compatibility |

### Resource Errors

| Pattern | Signature | Auto-Fix |
|---------|-----------|----------|
| Out of memory | exit 137/9, "OOM", "out of memory" | ✅ Increase memory limit |
| Timeout | "timed out", exit 124 | ✅ Increase time_limit |
| Disk full | ENOSPC, "No space left" | ❌ Suggest cleanup |
| Too many open files | EMFILE, "Too many open files" | ❌ Increase ulimit |

### Data Errors

| Pattern | Signature | Auto-Fix |
|---------|-----------|----------|
| Input file missing | "No such file", exit 1 | ❌ Point to missing path |
| File truncated | "truncated", "unexpected EOF" | ❌ Suggest re-download |
| Checksum mismatch | "checksum", "hash mismatch" | ❌ Verify file integrity |
| FASTQ quality low | "quality score" warnings | ✅ Suggest fastp insertion |
| Empty file | "empty file", "zero length" | ❌ Check upstream rule |

### System Errors

| Pattern | Signature | Auto-Fix |
|---------|-----------|----------|
| Permission denied | exit 126, "Permission denied" | ❌ Fix file permissions |
| Network error | "Connection refused", timeout | ❌ Check network |
| Lock file conflict | "already locked", "lock" | ✅ Remove stale lock |

### Config Errors

| Pattern | Signature | Auto-Fix |
|---------|-----------|----------|
| Incompatible params | "incompatible", "conflict" | ❌ Point to conflict |
| Wildcard expanded empty | "no files matching" | ❌ Check naming |
| Invalid reference | "reference not found" | ❌ Check genome install |
| Missing dependency rule | "rule not found", "depends on" | ❌ Add missing rule |

## API

```
GET /api/runs/{run_id}/diagnostics

Response:
{
  failed_nodes: [{
    rule: "star_align",
    error_pattern: "oom_killed",
    likely_cause: "STAR alignment needs ~32GB; currently 16GB",
    suggestions: ["Increase memory to 32GB", "Use --limitBAMsortRAM"],
    relevant_log_lines: ["FATAL: out of memory", "EXITING: 137"]
  }],
  warnings: [{
    rule: "qualimap",
    pattern: "skipped",
    suggestion: "Skipped due to upstream failure"
  }],
  resource_bottlenecks: [{
    rule: "star_align",
    metric: "memory",
    actual: 16384,
    limit: 32768
  }]
}
```

## Extending the Pattern Library

Add new patterns in `domains/execution/diagnostics.rs`:

```rust
ErrorPattern {
    id: "new_pattern_id",
    category: ErrorCategory::Tool,
    signatures: vec![
        ErrorSignature::ExitCode(134),
        ErrorSignature::StderrContains("specific error text"),
    ],
    likely_cause: "Description of the likely cause",
    auto_fixable: false,
    fix_action: Some(FixAction {
        description: "Suggested fix",
        config_change: None,
    }),
}
```
