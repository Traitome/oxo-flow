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
| `--verbose` | `-v` | — | Enable debug-level logging |

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
oxo-flow 0.3.0 — Bioinformatics Pipeline Engine
✓ Created new project at my-pipeline
  my-pipeline/my-pipeline.oxoflow
  my-pipeline/envs/
  my-pipeline/scripts/
  my-pipeline/.gitignore

  Edit my-pipeline/my-pipeline.oxoflow to define your pipeline.
```

### Generated files

**`<name>.oxoflow`** — Starter workflow file:

```toml
[workflow]
name = "my-pipeline"
version = "0.1.0"
description = "A new oxo-flow pipeline"

[config]
# Add your configuration variables here

[defaults]
threads = 4
memory = "8G"

# Define your pipeline rules below:
# [[rules]]
# name = "step1"
# input = ["input.txt"]
# output = ["output.txt"]
# shell = "cat input.txt > output.txt"
```

**`envs/`** — Directory for conda YAML / Docker / Pixi environment specs.

**`scripts/`** — Directory for helper scripts.

**`.gitignore`** — Pre-configured with bioinformatics patterns (BAM, VCF, index files, workflow outputs).

---

## Notes

- The output directory is created if it does not exist
- If the directory already exists, files are written into it without overwriting existing files
- The generated `.gitignore` includes common bioinformatics file types and oxo-flow internal directories
