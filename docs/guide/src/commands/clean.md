# `oxo-flow clean`

Clean workflow outputs and temporary files. Removes files declared as outputs in the workflow's rules.

---

## Usage

```
oxo-flow clean [OPTIONS] <WORKFLOW>
```

---

## Arguments

| Argument | Description |
|---|---|
| `<WORKFLOW>` | Path to the `.oxoflow` workflow file |

---

## Options

| Option | Short | Default | Description |
|---|---|---|---|
| `--dry-run` | `-n` | — | Show what would be cleaned without deleting |
| `--force` | — | — | Actually delete files (without it, clean defaults to dry-run) |
| `--orphans` | — | — | Clean orphaned temporary files (chunks from interrupted transforms) |
| `--workdir` | — | — | Working directory for `.oxo-flow` artifacts (default: the workflow file's directory) |
| `--verbose` | `-v` | — | Enable debug-level logging |

---

## Examples

### Preview what would be cleaned

```bash
oxo-flow clean pipeline.oxoflow -n
```

### Clean (defaults to dry-run)

```bash
# Without --force, clean only previews what would be deleted
oxo-flow clean pipeline.oxoflow
```

### Clean with confirmation

```bash
# With --force, files are deleted (an interactive confirmation
# prompt is shown when run in a terminal)
oxo-flow clean pipeline.oxoflow --force
```

### Clean orphaned chunks from interrupted transforms

```bash
# When a transform operation is interrupted (Ctrl+C), chunk files
# may remain in .oxo-flow/chunks/. Use --orphans to clean them.
oxo-flow clean pipeline.oxoflow --orphans
```

---

## Output

### Dry-run output

```
oxo-flow 0.10.2 — Bioinformatics Pipeline Engine
Would clean (dry-run):
  results/trimmed/sample1_R1.fastq.gz (exists)
  results/trimmed/sample1_R2.fastq.gz (exists)
  results/aligned/{sample}.bam (no files matched)
  results/report.html (not found)

Total: 4 patterns → 2 files (+ 1 unresolved wildcards)

Run with --force to actually delete these files.
```

### Clean output

```
oxo-flow 0.10.2 — Bioinformatics Pipeline Engine
⚠ 2 file(s) will be deleted. Continue? [y/N]
y
  ✓ results/trimmed/sample1_R1.fastq.gz
  ✓ results/trimmed/sample1_R2.fastq.gz

Done: 2 deleted, 0 failed, 1 not found, 1 wildcard skipped, 0 rejected
```

---

## Notes

- **Wildcard patterns** (containing `{` and `}`) are resolved against the filesystem as glob patterns; patterns that match no files are reported as `(no files matched)` and skipped during deletion
- **Path Traversal Protection** — during deletion, paths that begin with `/`, `~`, or contain `..` are strictly rejected, marked as `rejected`, and never deleted
- **Non-existent files** are silently skipped (not counted as errors)
- Without `--force`, no files are deleted — clean defaults to dry-run
- With `--force`, files are deleted after an interactive confirmation prompt (skipped when stdin is not a terminal)
- Use `--dry-run` to preview the list of files that would be affected before committing to a clean
- Only files declared as rule `output` are targeted — input files, scripts, and environment specs are never deleted
