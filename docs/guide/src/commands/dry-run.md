# `oxo-flow dry-run`

Simulate execution without running any commands. Shows the execution plan, rule order, and expanded shell commands.

---

## Usage

```
oxo-flow dry-run [OPTIONS] [WORKFLOW]
```

---

## Arguments

| Argument | Description |
|---|---|
| `[WORKFLOW]` | Path to the `.oxoflow` workflow file. **Optional** — if not specified, auto-discovery searches for: (1) `main.oxoflow` in current directory, (2) alphabetically first `*.oxoflow` file in current directory. |

---

## Options

| Option | Short | Description |
|---|---|---|
| `--target` | `-t` | Preview only specific target rules and their dependencies (repeatable) |
| `--verbose` | `-v` | Enable debug-level logging |

---

## Examples

### Preview with auto-discovery

```bash
# Auto-discover workflow in current directory
oxo-flow dry-run
```

### Preview a specific workflow

```bash
oxo-flow dry-run pipeline.oxoflow
```

### Preview a specific target rule and its dependencies

```bash
oxo-flow dry-run pipeline.oxoflow -t align
```

### Preview multiple target rules

```bash
oxo-flow dry-run pipeline.oxoflow -t align -t sort_bam
```

### With verbose output

```bash
oxo-flow dry-run pipeline.oxoflow -v
```

---

## Output

```
oxo-flow 0.3.0 — Bioinformatics Pipeline Engine
Dry-run: 3 rules would execute:
  1. trim_reads [threads=4, env=conda]
     $ fastp --in1 raw/sample1_R1.fastq.gz --in2 raw/sample1_R2.fastq.gz --out1
  2. align [threads=16, env=docker]
     $ bwa mem -t 16 /data/ref/hg38.fa trimmed/sample1_R1.fastq.gz | samtools so
  3. sort_bam [threads=4, env=conda]
     $ samtools sort -@ 4 -o sorted/sample1.bam aligned/sample1.bam
```

---

## Notes

- The workflow file is optional; if not specified, auto-discovery searches for `main.oxoflow` first, then any `*.oxoflow` file alphabetically
- If no `.oxoflow` file is found, an error message suggests running `oxo-flow init` to create one
- No shell commands are executed — the dry-run is read-only
- Shell command previews are truncated to 80 characters
- The environment type (conda, docker, etc.) is shown for each rule
- Thread and resource settings are displayed per rule
- Use dry-run to verify your workflow before committing compute resources
- When `--target` is specified, only the named rules and all rules they depend on
  (transitively) are shown — downstream rules are excluded
