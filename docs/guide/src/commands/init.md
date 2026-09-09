# `oxo-flow init`

Initialize a new workflow project with a starter `.oxoflow` file, directory structure, and `.gitignore`.

---

## Usage

```
oxo-flow init [OPTIONS] <NAME>
```

---

## Arguments

| Argument | Description |
|---|---|
| `<NAME>` | Project name (also used as the default directory name) |

---

## Options

| Option | Short | Default | Description |
|---|---|---|---|
| `--dir` | `-d` | `./<NAME>` | Output directory (defaults to the project name) |
| `--verbose` | `-v` | — | Enable verbose (debug-level) logging |
| `--quiet` | — | — | Suppress non-essential output (errors only) |
| `--no-color` | — | — | Disable colored output |


> The global `--json` flag is **not supported** by this command — passing it fails fast instead of being silently ignored. Machine-readable output is available from: `run`, `dry-run`, `validate`, `lint`, `test`, `status`, `batch`, `info`, `schema`, `license`, `ai`, and `provenance verify`.

---

## Examples

### Create a new project

```bash
oxo-flow init my-pipeline
```

### Create in a specific directory

```bash
oxo-flow init my-pipeline -d /projects/genomics/my-pipeline
```

---

## Output

```
oxo-flow v0.18.0 — Rust-native bioinformatics pipeline engine
✓ Created new project at my-pipeline
  my-pipeline/my-pipeline.oxoflow
  my-pipeline/envs/example.yaml
  my-pipeline/scripts/example.sh
  my-pipeline/.gitignore

  Next steps: To run your first workflow:
    cd my-pipeline
    oxo-flow run my-pipeline.oxoflow
```

### Generated files

**`<name>.oxoflow`** — Starter workflow file:

```toml
[workflow]
name = "my-pipeline"
version = "0.1.0"
description = "A new oxo-flow pipeline"
author = ""

[config]
# Variables defined here are used in shell commands as {config.key}
sample_name = "example"
greeting = "Hello from oxo-flow!"

[defaults]
threads = 1
memory = "1G"

[[rules]]
name = "hello_world"
description = "A minimal rule that writes a greeting"
output = ["results/{config.sample_name}_output.txt"]
# The greeting is passed via the environment, not spliced into the shell —
# an apostrophe or quote in the config value cannot break the command
# (`--arg greeting=...` overrides the default).
envvars = { GREETING = "{config.greeting}" }
shell = "echo \"$GREETING\" > {output[0]}"
```

**`envs/example.yaml`** — Starter conda environment specification.

**`scripts/example.sh`** — Starter helper script.

**`data/`** — Pre-populated with an `input.txt` sample to allow immediate execution.

**`results/`** — Empty directory created for workflow outputs.

**`.gitignore`** — Pre-configured with bioinformatics patterns (BAM, VCF, index files, workflow outputs).

---

## Notes

- The output directory is created if it does not exist
- If the directory already exists, oxo-flow prints a warning and files may be overwritten
- The generated `.gitignore` includes common bioinformatics file types and oxo-flow internal directories
