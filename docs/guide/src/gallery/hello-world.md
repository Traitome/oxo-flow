# 01 — Hello World

The simplest possible oxo-flow workflow: a single rule that writes a greeting to a file.

!!! info "Concepts Covered"
    - Minimal workflow structure (`[workflow]` + `[[rules]]`)
    - Shell commands in rules
    - Output file declarations

## Workflow Definition

```toml
# examples/gallery/01_hello_world.oxoflow

[workflow]
name = "hello-world"
version = "1.0.0"
description = "A minimal workflow that writes a greeting to a file"
author = "oxo-flow examples"

[[rules]]
name = "greet"
output = ["hello.txt"]
shell = "echo 'Hello from oxo-flow!' > {output[0]}"
```

## Key Concepts

### Workflow Metadata

Every `.oxoflow` file begins with a `[workflow]` section that declares the pipeline's identity:

- **`name`** — unique identifier for the workflow
- **`version`** — semantic version (recommended)
- **`description`** — human-readable summary

### Rules

A `[[rules]]` entry defines a single step. The double brackets (`[[...]]`) indicate an array of tables in TOML — you can have as many rules as you need.

Each rule needs:

- **`name`** — unique identifier within the workflow
- **`output`** — list of files this rule produces
- **`shell`** — the command to execute

### Output Substitution

`{output[0]}` in the shell command is replaced with the first element of the `output` array at execution time. This ensures the command always writes to the declared output path.

## Running the Workflow

### Validate

```bash
$ oxo-flow validate examples/gallery/01_hello_world.oxoflow
✓ examples/gallery/01_hello_world.oxoflow — 1 rules, 0 dependencies
```

### Dry-Run

```bash
$ oxo-flow dry-run examples/gallery/01_hello_world.oxoflow
oxo-flow 0.3.0 — Bioinformatics Pipeline Engine
Dry-run: 1 rules would execute:
  1. greet [threads=1, env=system]
     $ echo 'Hello from oxo-flow!' > {output[0]}
```

### Execute

```bash
$ oxo-flow run examples/gallery/01_hello_world.oxoflow
```

### DAG Visualization

Since this is a single rule, the DAG is trivial:

```bash
$ oxo-flow graph examples/gallery/01_hello_world.oxoflow
digraph {
    0 [ label = "greet" ]
}
```

## What's Next?

Move on to [File Pipeline](file-pipeline.md) to learn how multiple rules chain together through input/output dependencies.
