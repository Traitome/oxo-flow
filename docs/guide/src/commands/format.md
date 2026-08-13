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
| `--verbose` | `-v` | — | Enable verbose (debug-level) logging |
| `--quiet` | — | — | Suppress non-essential output (errors only) |
| `--no-color` | — | — | Disable colored output |
| `--json` | — | — | Output machine-readable JSON to stdout |

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
- The file is re-serialized from the parsed configuration, so comments are not preserved
