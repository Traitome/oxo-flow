# `oxo-flow format`

Reformat a `.oxoflow` file into canonical TOML form.

---

## Usage

```
oxo-flow format [OPTIONS] <WORKFLOW>
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
| `--output` | `-o` | stdout | Write formatted output to a file |
| `--check` | — | — | Check if the file is already formatted (exit non-zero if not) |

---

## Examples

### Format and print to stdout

```bash
oxo-flow format pipeline.oxoflow
```

### Save formatted output to a new file

```bash
oxo-flow format pipeline.oxoflow -o formatted.oxoflow
```

### Check formatting in CI

```bash
oxo-flow format pipeline.oxoflow --check
```

---

## Output

```
oxo-flow 0.5.1 — Bioinformatics Pipeline Engine
[workflow]
name = "my-pipeline"
version = "0.1.0"

[[rules]]
name = "step1"
input = ["input.txt"]
output = ["output.txt"]
shell = "cat input.txt > output.txt"
```

---

## Notes

- The formatter ensures consistent indentation and key ordering
- Using `--check` is recommended for CI/CD pipelines to enforce style consistency
- Comments are preserved during reformatting
