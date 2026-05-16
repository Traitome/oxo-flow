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
| `--force` | — | — | Skip the confirmation prompt |
| `--verbose` | `-v` | — | Enable debug-level logging |

---

## Examples

### Preview what would be cleaned

```bash
oxo-flow clean pipeline.oxoflow -n
```

### Clean with confirmation prompt

```bash
oxo-flow clean pipeline.oxoflow
```

### Clean without confirmation

```bash
oxo-flow clean pipeline.oxoflow --force
```

---

## Output

### Dry-run output

```
oxo-flow 0.4.1 — Bioinformatics Pipeline Engine
Would clean (dry-run):
  results/trimmed/sample1_R1.fastq.gz (exists)
  results/trimmed/sample1_R2.fastq.gz (exists)
  results/aligned/{sample}.bam (wildcard, skipped)
  results/report.html (not found)

Total: 4 output patterns
```

### Clean output

```
oxo-flow 0.4.1 — Bioinformatics Pipeline Engine
Clean: 2 file(s) will be deleted. Continue? [y/N]
y
  ✓ results/trimmed/sample1_R1.fastq.gz
  ✓ results/trimmed/sample1_R2.fastq.gz

Done: 2 deleted, 0 failed, 1 not found, 1 wildcard skipped, 0 rejected
```

---

## Notes

- **Wildcard patterns** (containing `{` and `}`) are skipped because they cannot be resolved to specific files without runtime context
- **Path Traversal Protection** strictly rejects paths that begin with `/`, `~`, or contain `..`, marking them as `rejected` and preventing arbitrary file deletion
- **Non-existent files** are silently skipped (not counted as errors)
- Without `--force`, a confirmation prompt is shown before deleting any files
- Use `--dry-run` to preview the list of files that would be affected before committing to a clean
- Only files declared as rule `output` are targeted — input files, scripts, and environment specs are never deleted
