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

## Matching semantics

- `stderr_patterns` are **case-insensitive regular expressions**, compiled
  once per engine and shared process-wide. Entries must stay specific
  (loader lines, tool error prefixes) — near-miss words must not fire
  (issue #708).
- A pattern **without exit codes** fires on any non-zero or unknown exit
  code — never on exit 0. A rule that exited 0 has nothing to diagnose.
- The same pattern table backs `monitor_agent::is_known_error_pattern`, so
  the AI monitor and the diagnostics service never disagree on a
  classification.

## Error Pattern Library

30 patterns total across five categories — Tool (5), Resource (5), Data (7),
System (6), Config (7). The tables below cover the most common; the
authoritative list is
`crates/oxo-flow-web/src/domains/execution/diagnostics.rs`.

### Tool Errors

| Pattern | Signature | Auto-Fix |
|---------|-----------|----------|
| Command not found | exit 127, "command not found" | ✅ Suggest `conda install` |
| Version incompatibility | "version mismatch/conflict", "incompatible … version", "unsupported version" | ❌ Suggest version switch |
| Tool crash (SIGSEGV) | exit 139, "segmentation fault" | ❌ Suggest tool update |
| Tool crash (SIGILL) | exit 132, "illegal instruction" | ❌ Check CPU compatibility |
| Tool crash (SIGBUS) | exit 138, "bus error" | ❌ Check filesystem integrity |

### Resource Errors

| Pattern | Signature | Auto-Fix |
|---------|-----------|----------|
| Out of memory | exit 137/9, "out of memory", "bad_alloc", `\boom\b` | ✅ Increase memory limit |
| Timeout | "timed out", exit 124 | ✅ Increase time_limit |
| Disk full | ENOSPC, "no space left on device" | ❌ Suggest cleanup |
| Too many open files | "too many open files", EMFILE, "ulimit" + failure word | ✅ Raise `ulimit -n` before starting |
| CPU limit exceeded | "cpu time limit", "cpu limit", EAGAIN | ✅ Increase thread/CPU allocation |

### Data Errors

| Pattern | Signature | Auto-Fix |
|---------|-----------|----------|
| Input file missing | "no such file or directory", "cannot open", "file not found", exit 1 | ❌ Point to missing path |
| File truncated | "truncated", "unexpected end", "premature" | ❌ Suggest re-download |
| Corrupt gzip file | "not in gzip format", "invalid compressed data", exit 1 | ❌ Verify file is valid gzip |
| FASTQ quality low | "per base sequence quality … fail/warn", "low quality", "poor quality" | ✅ Suggest fastp insertion |
| Empty file | "empty file", "zero length", "is empty" | ❌ Check upstream rule |
| BAM truncated | "truncated file", "EOF marker", "bgzf", exit 1 | ✅ Suggest `samtools index` |
| BAM index missing | "could not open/retrieve index file", "no index available", exit 1 | ✅ Suggest `samtools index` |

### System Errors

| Pattern | Signature | Auto-Fix |
|---------|-----------|----------|
| Permission denied | exit 126/13, "permission denied", EACCES | ❌ Fix file permissions |
| Network error | "connection refused", "could not resolve host", "network is unreachable" | ❌ Check network |
| Shared library missing | "error while loading shared libraries", "cannot open shared object", "undefined symbol" (exit 127/1) | ✅ Suggest conda install |
| Broken pipe (SIGPIPE) | exit 141/13, "broken pipe" | ❌ Check pipeline commands |
| Signal kill (SIGTERM) | exit 143/15, "terminated", `\bkilled\b` | ❌ Check system logs |
| Signal interrupt (SIGINT) | exit 130/2, "interrupt", "cancelled" | ❌ Re-run when ready |

### Config Errors

| Pattern | Signature | Auto-Fix |
|---------|-----------|----------|
| Invalid parameter | "invalid option", "unrecognized", "unknown option", exit 1 | ❌ Check tool documentation |
| Missing required parameter | "arguments are required", "missing required <arg>", "must specify", exit 1 | ❌ Add the missing parameter |
| Wildcard expanded empty | "no files/matches", "wildcard … did not match" | ❌ Check naming |
| Conda environment failed | "CondaError", "Solving environment: failed", "environment.yml", exit 1 | ❌ Verify conda env name |
| Docker failed | exit 125/126, "docker daemon", "error response from daemon" | ❌ Check Docker daemon |
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
    // Case-insensitive regexes — keep them specific enough that benign
    // output cannot match ("tool version 1.2" must not fire).
    stderr_patterns: compile(&["specific error text"]),
    likely_cause: "Description of the likely cause",
    auto_fixable: false,
    fix_desc: Some("Suggested fix"),
    fix_config_path: None,
}
```

Every new pattern must ship with two test rows in the module's tests
(issue #708):

- a canonical trigger line in `CANONICAL_TRIGGERS` (proves the pattern is
  not dead — the test fails if a pattern has no row, or a row no pattern),
- and, when near-miss words exist, a benign line in `BENIGN_LOG_LINES`
  that mentions the vocabulary without being a real error.
