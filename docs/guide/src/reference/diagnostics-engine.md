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
against a curated library of 30 error patterns (tool, resource, data,
system, and config classes — see
`crates/oxo-flow-web/src/domains/execution/diagnostics.rs` for the full
list).

## Error Pattern Library

30 patterns total across five categories — Tool (6), Resource (5), Data (8),
System (6), Config (7). The tables below cover the most common; the
authoritative list is
`crates/oxo-flow-web/src/domains/execution/diagnostics.rs`.

### Tool Errors

| Pattern | Signature | Auto-Fix |
|---------|-----------|----------|
| Command not found | exit 127, "command not found" | ✅ Suggest `conda install` |
| Version incompatibility | "version", "incompatible", "unsupported" | ❌ Suggest version switch |
| Tool crash (SIGSEGV) | exit 139, "segmentation fault" | ❌ Suggest tool update |
| Tool crash (SIGILL) | exit 132, "illegal instruction" | ❌ Check CPU compatibility |
| Tool crash (SIGBUS) | exit 138, "bus error" | ❌ Check filesystem integrity |

### Resource Errors

| Pattern | Signature | Auto-Fix |
|---------|-----------|----------|
| Out of memory | exit 137/9, "out of memory", "cannot allocate memory" | ✅ Increase memory limit |
| Timeout | "timed out", exit 124 | ✅ Increase time_limit |
| Disk full | ENOSPC, "no space left on device" | ❌ Suggest cleanup |
| Too many open files | "too many open files", EMFILE | ✅ Raise `ulimit -n` before starting |
| CPU limit exceeded | "cpu time", "cpu limit", EAGAIN | ✅ Increase thread/CPU allocation |

### Data Errors

| Pattern | Signature | Auto-Fix |
|---------|-----------|----------|
| Input file missing | "no such file or directory", "cannot open", exit 1 | ❌ Point to missing path |
| File truncated | "truncated", "unexpected end", "premature" | ❌ Suggest re-download |
| Corrupt gzip file | "not in gzip format", "corrupt input", exit 1 | ❌ Verify file is valid gzip |
| FASTQ quality low | "per base sequence quality.*fail", "low quality", "poor quality" | ✅ Suggest fastp insertion |
| Empty file | "empty file", "zero length", "no data" | ❌ Check upstream rule |
| BAM truncated / index | "truncated file", "bam index", "EOF marker", exit 1 | ✅ Suggest `samtools index` |
| BAM index missing | "failed to open index", "no index available", exit 1 | ✅ Suggest `samtools index` |

### System Errors

| Pattern | Signature | Auto-Fix |
|---------|-----------|----------|
| Permission denied | exit 126/13, "permission denied", EACCES | ❌ Fix file permissions |
| Network error | "connection refused", "cannot resolve", "timeout" | ❌ Check network |
| Shared library missing | "error while loading shared libraries", "cannot open shared object" (exit 127/1) | ✅ Suggest conda install |
| Broken pipe (SIGPIPE) | exit 141/13, "broken pipe" | ❌ Check pipeline commands |
| Signal kill (SIGTERM) | exit 143/15, "terminated", "killed" | ❌ Check system logs |
| Signal interrupt (SIGINT) | exit 130/2, "interrupt", "cancelled" | ❌ Re-run when ready |

### Config Errors

| Pattern | Signature | Auto-Fix |
|---------|-----------|----------|
| Invalid parameter | "invalid option", "unrecognized", "unknown option", exit 1 | ❌ Check tool documentation |
| Missing required parameter | "required", "must specify", "missing", exit 1 | ❌ Add the missing parameter |
| Wildcard expanded empty | "no matches", "no files", "wildcard", "empty" | ❌ Check naming |
| Conda environment failed | "conda", "environment", "create failed", exit 1 | ❌ Verify conda env name |
| Docker failed | exit 125/126, "docker", "cannot connect", "daemon" | ❌ Check Docker daemon |
| Singularity/Apptainer failed | exit 255, "singularity", "apptainer", "image not found" | ❌ Pull the image |
| Shell syntax error | exit 2, "syntax error", "unexpected token", "parse error" | ❌ Review the command |

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
  missed. Cluster-executed rules carry the scheduler accounting store's
  peak RSS when it reports one (SLURM `sacct`'s max `MaxRSS` across task
  rows); the value is `None` only for schedulers without accounting
  (e.g. LSF) or a missing accounting row. Legacy checkpoints degrade to
  an empty list.

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
