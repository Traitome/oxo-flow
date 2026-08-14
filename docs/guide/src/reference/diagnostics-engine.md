# Diagnostics Engine

> Deterministic failure analysis — no AI, pure pattern matching.
> Every diagnosis is reproducible given the same inputs.

## Overview

The Diagnostics Engine analyzes failed pipeline runs and returns:
- **Error pattern** identified (e.g., OOM, command not found, file missing)
- **Likely cause** with evidence
- **Fix suggestions** (auto-fixable or manual)
- **Relevant log lines** for context

It does NOT use AI. It matches error signatures (log keywords, exit codes)
against a curated library of 30+ error patterns.

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
| Out of memory | exit 137/9, "out of memory", "cannot allocate memory" | ✅ Increase memory limit |
| Timeout | "timed out", exit 124 | ✅ Increase time_limit |
| Disk full | ENOSPC, "No space left" | ❌ Suggest cleanup |
| Too many open files | EMFILE, "Too many open files" | ✅ Run `ulimit -n 65536` before starting |

### Data Errors

| Pattern | Signature | Auto-Fix |
|---------|-----------|----------|
| Input file missing | "No such file", exit 1 | ❌ Point to missing path |
| File truncated | "truncated", "unexpected end" | ❌ Suggest re-download |
| Corrupt gzip file | "not in gzip format", "corrupt input" | ❌ Verify file is valid gzip |
| FASTQ quality low | "low quality", "poor quality" | ✅ Suggest fastp insertion |
| Empty file | "empty file", "zero length" | ❌ Check upstream rule |

### System Errors

| Pattern | Signature | Auto-Fix |
|---------|-----------|----------|
| Permission denied | exit 126, "Permission denied" | ❌ Fix file permissions |
| Network error | "Connection refused", timeout | ❌ Check network |
| Shared library missing | "error while loading shared libraries", "cannot open shared object" | ✅ Suggest conda install |

### Config Errors

| Pattern | Signature | Auto-Fix |
|---------|-----------|----------|
| Invalid parameter | "invalid option", "unknown option" | ❌ Check tool documentation |
| Missing required parameter | "required", "must specify" | ❌ Add the missing parameter |
| Wildcard expanded empty | "no matches", "no files", "empty" | ❌ Check naming |
| Conda environment failed | "conda", "environment", "create failed" | ❌ Verify conda env name |

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
    suggestion: "This rule was skipped due to upstream failure."
  }],
  resource_bottlenecks: []
}
```

`resource_bottlenecks` lists rules whose measured memory use pressed against
their declared limit (issue #67 §4):

```json
resource_bottlenecks: [{
  "rule": "markdup",
  "metric": "max_memory_mb",
  "actual": 31.0,
  "limit": 32.0
}]
```

- `actual` is the rule's sampled peak RSS in MiB (recorded by the local
  executor into the checkpoint's benchmark records) and `limit` is its
  declared memory limit (`memory` / `resources.memory`, resolved at
  execution time).
- A rule is flagged when `actual ≥ 80% × limit` — a conservative threshold
  because the peak is **sampled every 200 ms** across the rule's process
  subtree, not an exact `getrusage` maximum; sub-interval spikes can be
  missed. Cluster-executed rules carry no measurement (`None`), and legacy
  checkpoints degrade to an empty list.

## Extending the Pattern Library

Add new patterns in `crates/oxo-flow-web/src/domains/execution/diagnostics.rs`
(inside `DiagnosticsEngine::new()`):

```rust
Pattern {
    id: "new_pattern_id",
    category: ErrorCategory::Tool,
    exit_codes: vec![134],
    stderr_patterns: vec!["specific error text"],
    likely_cause: "Description of the likely cause",
    auto_fixable: false,
    fix_desc: Some("Suggested fix"),
    fix_config_path: None,
}
```
