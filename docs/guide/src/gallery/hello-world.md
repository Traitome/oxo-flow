# 01 — Hello World

The simplest possible oxo-flow workflow: a single rule that writes a greeting to a file.

!!! info "Concepts Covered"
    - Minimal workflow structure (`[workflow]` + `[[rules]]`)
    - Shell commands in rules
    - Output file declarations

## Workflow Definition

```toml
# examples/gallery/01_hello_world.oxoflow
--8<-- "examples/gallery/01_hello_world.oxoflow"
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

!!! note "`input` and `output` can be omitted"
    Both are optional when they don't apply:
    - Omit `input` when a rule reads no files (like `greet` above)
    - Omit `output` for setup-only rules that produce no files (e.g. `mkdir`)
    Declare them whenever files flow between rules — that's what the DAG engine uses to infer dependencies.

### Output Substitution

`{output[0]}` in the shell command is replaced with the first element of the `output` array at execution time. This ensures the command always writes to the declared output path.

All built-in placeholders (`{input}`, `{output}`, `{threads}`, `{memory}`, `{config.x}`…) are listed in the [wildcards reference](../reference/wildcards.md#built-in-placeholders).

## Running the Workflow

### Validate

```bash
$ oxo-flow validate examples/gallery/01_hello_world.oxoflow
✓ examples/gallery/01_hello_world.oxoflow — 1 rules, 0 dependencies
```

### Dry-Run

```bash
$ oxo-flow dry-run examples/gallery/01_hello_world.oxoflow
oxo-flow 0.11.0 — Bioinformatics Pipeline Engine
DAG: (dry-run) 1 rules would execute
  1. greet
     threads=1
     outputs: ["hello.txt"]
     command: echo 'Hello from oxo-flow!' > hello.txt

Summary: 1 rules, total 1 threads declared, max 1 threads/rule

To execute:  oxo-flow run examples/gallery/01_hello_world.oxoflow -j 1
```

### Execute

```bash
$ oxo-flow run examples/gallery/01_hello_world.oxoflow
```

### DAG Visualization

Since this is a single rule, the DAG is trivial. The default `graph` output is an ASCII level-based tree; use `-f dot` to export Graphviz DOT format:

```bash
$ oxo-flow graph -f dot examples/gallery/01_hello_world.oxoflow
oxo-flow 0.11.0 — Bioinformatics Pipeline Engine
digraph {
    0 [ label = "greet"]
}
```

## What's Next?

Move on to [File Pipeline](file-pipeline.md) to learn how multiple rules chain together through input/output dependencies.
