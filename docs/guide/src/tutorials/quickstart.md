# Quick Start

Get from zero to a running pipeline in under five minutes. This tutorial assumes you have already [installed oxo-flow](./installation.md).

---

## 1. Initialize a project

```bash
oxo-flow init my-pipeline
cd my-pipeline
```

This creates:

```
my-pipeline/
├── my-pipeline.oxoflow    # Workflow definition
├── envs/                  # Environment specs
├── scripts/               # Helper scripts
└── .gitignore             # Bioinformatics-aware ignore file
```

---

## 2. Define a simple workflow

Open `my-pipeline.oxoflow` and replace its contents:

```toml
[workflow]
name = "my-pipeline"
version = "0.1.0"
description = "A simple two-step demo"

[defaults]
threads = 2

[[rules]]
name = "create_data"
input = []
output = ["data/greeting.txt"]
shell = "mkdir -p data && echo 'Hello from oxo-flow!' > data/greeting.txt"

[[rules]]
name = "transform"
input = ["data/greeting.txt"]
output = ["results/uppercase.txt"]
shell = "mkdir -p results && tr '[:lower:]' '[:upper:]' < data/greeting.txt > results/uppercase.txt"
```

This workflow has two rules:

1. **create_data** — writes a text file (no input files required)
2. **transform** — converts the file to uppercase (depends on `create_data`'s output)

oxo-flow infers the dependency automatically because `transform`'s input matches `create_data`'s output.

---

## 3. Validate

```bash
oxo-flow validate my-pipeline.oxoflow
```

```
✓ my-pipeline.oxoflow — 2 rules, 1 dependencies
```

---

## 4. Dry-run

Preview the execution plan without running anything:

```bash
oxo-flow dry-run my-pipeline.oxoflow
```

```
oxo-flow 0.10.2 — Bioinformatics Pipeline Engine
DAG: (dry-run) 2 rules would execute
  1. create_data
     threads=2
     outputs: ["data/greeting.txt"]
     command: mkdir -p data && echo 'Hello from oxo-flow!' > data/greeting.txt
  2. transform
     threads=2
     outputs: ["results/uppercase.txt"]
     command: mkdir -p results && tr '[:lower:]' '[:upper:]' < data/greeting.txt > results/uppercase.txt
     input ✗: data/greeting.txt

Summary: 2 rules, total 4 threads declared, max 2 threads/rule

To execute:  oxo-flow run my-pipeline.oxoflow -j 10
```

---

## 5. Execute

```bash
oxo-flow run my-pipeline.oxoflow
```

```
oxo-flow 0.10.2 — Bioinformatics Pipeline Engine
DAG: 2 rules in execution order
  1. create_data
  2. transform
  Running: create_data
  ✓ create_data (0.0s)
  Running: transform
  ✓ transform (0.0s)

Done: 2 succeeded, 0 skipped, 0 failed
✓ 2 output files verified (42B total)
```

---

## 6. Check the results

Ensure you are in the `my-pipeline` directory:

```bash
cd my-pipeline
cat results/uppercase.txt
# HELLO FROM OXO-FLOW!
```

!!! success "What to verify"
    After running your first workflow, check these to confirm success:

    1. **Output files exist**: `ls results/` shows `uppercase.txt`
    2. **Content is correct**: The file contains the expected uppercase text
    3. **No error files**: Check `.oxo-flow/` for any error logs

If the output doesn't match expectations, see the [Troubleshooting Guide](../how-to/troubleshooting.md).

---

## 7. Visualize the DAG

oxo-flow provides multiple ways to visualize your workflow's structure.

### Terminal View (Default)

The default `graph` command prints a stylized ASCII or tree representation directly to your terminal:

```bash
oxo-flow graph my-pipeline.oxoflow
```

```text
oxo-flow 0.10.2 — Bioinformatics Pipeline Engine
┌──────────────────────────────────────────────┐
│  Workflow DAG: 2 rules, 1 dependencies       │
│  Depth: 2, Width: 1, Critical path: 2 steps  │
└──────────────────────────────────────────────┘

Level 0 (sequential)
     create_data
     │
     ▼
Level 1 (sequential)
     transform [depends: create_data]

Critical path: create_data → transform
```

### Graphviz (DOT) Export

For complex pipelines, you can export to Graphviz DOT format for high-resolution rendering:

```bash
oxo-flow graph my-pipeline.oxoflow --format dot
```

```dot
oxo-flow 0.10.2 — Bioinformatics Pipeline Engine
digraph {
    0 [ label = "create_data"]
    1 [ label = "transform"]
    0 -> 1 [ ]
}
```

If you have Graphviz installed, render it (macOS: `brew install graphviz`):

```bash
oxo-flow graph my-pipeline.oxoflow -f dot | dot -Tpng -o dag.png
```

---

## What Just Happened?

1. oxo-flow parsed the `.oxoflow` TOML file into a `WorkflowConfig`
2. The DAG engine analyzed input/output dependencies and built a directed acyclic graph
3. Topological sorting determined that `create_data` must run before `transform`
4. The local executor ran each rule's shell command in order
5. Success/failure was reported for each step

---

## Web Interface

Start the web server for a browser-based workflow experience:

```bash
# Personal mode (localhost, no auth)
oxo-flow serve

# Team mode (multi-user, OAuth2)
oxo-flow serve --mode team

# HPC mode (cluster submit panel, scheduler auto-detected)
oxo-flow serve --mode hpc
```

Open `http://localhost:8080` to access the web UI with:

- DAG visualization with live status
- Pipeline validation and execution monitoring
- AI-powered pipeline generation from natural language
- Real-time resource metrics and diagnostics

See [Deployment Modes](../how-to/deploy-modes.md) for detailed configuration.

---

## Next Steps

- [Your First Workflow](./first-workflow.md) — build a real bioinformatics pipeline with environments
- [Variant Calling Pipeline](./variant-calling.md) — complete NGS analysis tutorial
- [Create a Workflow](../how-to/create-workflow.md) — reference guide for `.oxoflow` authoring
- [Command Reference](../commands/run.md) — explore all CLI options
