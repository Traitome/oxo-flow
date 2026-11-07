# `oxo-flow format`

Reformat a `.oxoflow` file into canonical TOML form (in place by default).

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
| `--output` | `-o` | in place | Write formatted output to this path instead of rewriting the input file |
| `--check` | — | — | Check if the file is already formatted (exit non-zero if not) |
| `--verbose` | `-v` | — | Enable verbose (debug-level) logging |
| `--quiet` | — | — | Suppress non-essential output (errors only) |
| `--no-color` | — | — | Disable colored output |


> The global `--json` flag is **not supported** by this command — passing it fails fast instead of being silently ignored. Machine-readable output is available from: `run`, `dry-run`, `validate`, `lint`, `test`, `status`, `batch`, `info`, `knowledge`, `schema`, `verify-data`, `license`, `ai`, and `provenance verify`.

---

## Examples

### Format in place

```bash
oxo-flow format pipeline.oxoflow
```

### Save formatted output to a new file (input untouched)

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
[[rules]]
name = "step1"
input = ["input.txt"]
output = ["output.txt"]
shell = "cat input.txt > output.txt"

[workflow]
name = "my-pipeline"
version = "0.1.0"
```

---

## Notes

- The formatter ensures consistent indentation and key ordering: map-backed
  sections (`[config]`, `[metadata]`, include `params`, …) are emitted with
  keys in **sorted** order, so the same file always formats to the same
  bytes — the property `--check` relies on
- Internal parse state is never emitted: declarative `[config]` entries
  (`key = { default = …, … }`) roundtrip as themselves, not as a
  `[config_meta]` section
- The always-derived fan-out keys `config.paired_pairs_list` /
  `config.paired_pairs_count` (injected by the parser from `[[pairs]]`)
  are **not** emitted — they are re-derived at every parse, so
  keeping them out is what makes `format` converge with `format --check`.
  User-declared `samples_list` / `pairs_list` entries are preserved verbatim
- Using `--check` is recommended for CI/CD pipelines to enforce style consistency
- The file is re-serialized from the parsed configuration, so comments are not preserved
