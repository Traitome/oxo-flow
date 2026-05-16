# `oxo-flow batch`

Execute a command template in parallel across multiple items.

---

## Usage

```
oxo-flow batch [OPTIONS] <TEMPLATE> [ITEMS...]
```

---

## Arguments

| Argument | Description |
|---|---|
| `<TEMPLATE>` | Command template with placeholders (`{item}`, `{stem}`, etc.) |
| `[ITEMS...]` | Items to process (files, globs, or from stdin/file) |

---

## Options

| Option | Short | Default | Description |
|---|---|---|---|
| `--jobs` | `-j` | 1 | Number of parallel workers |
| `--stop-on-error` | `-x` | — | Stop after first failure |
| `--file` | `-f` | — | Read items from file |
| `--json` | — | — | Output results as JSON |
| `--dry-run` | `-n` | — | Preview without executing |
| `--workdir` | `-d` | . | Working directory |
| `--environment` | `-e` | — | Environment spec |
| `--checksum` | — | — | Compute output checksums |
| `--generate-workflow` | — | — | Generate .oxoflow file |
| `--output` | `-o` | batch.oxoflow | Output workflow file |

---

## Placeholders

| Placeholder | Example | Description |
|---|---|---|
| `{}` / `{item}` | `s1.bam` | Current item |
| `{nr}` | 1, 2, 3... | 1-based index |
| `{basename}` | `s1.bam` | Filename (no path) |
| `{dir}` | `data/` | Directory |
| `{stem}` | `s1` | Filename without extension |
| `{ext}` | `bam` | Extension |

---

## Examples

### Basic batch execution

```bash
oxo-flow batch "samtools flagstat {item}" *.bam
```

### Parallel with 8 workers

```bash
oxo-flow batch -j 8 "fastqc {item}" *.fastq.gz
```

### Stop on first error

```bash
oxo-flow batch -x "bwa mem ref.fa {item}" samples.txt
```

### From stdin (pipeline)

```bash
ls *.bam | oxo-flow batch "samtools flagstat {item}"
```

### With environment

```bash
oxo-flow batch -e "conda: bwa_env" "bwa mem ref.fa {item}" *.fastq.gz
```

### Generate workflow

```bash
oxo-flow batch --generate-workflow "bwa mem ref.fa {item}" *.bam -o align.oxoflow
```

### JSON output

```bash
oxo-flow batch --json "samtools flagstat {item}" *.bam > results.json
```

### Dry-run preview

```bash
oxo-flow batch -n "fastqc {item}" *.fastq.gz
```

---

## Notes

- Items can be files, globs (`*.bam`), or plain strings
- Input file format: one item per line, blank lines and `#` comments ignored
- When no items provided and stdin has data, reads from pipe
- `{}` is rush-compatible shorthand for `{item}`
- Environment spec format: `conda: env.yaml` or `docker: image:tag`
- Generated workflows use wildcard patterns when items follow a naming pattern