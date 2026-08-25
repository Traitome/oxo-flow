# `oxo-flow graph`

Output the workflow DAG for visualization.

---

## Usage

```
oxo-flow graph [OPTIONS] <WORKFLOW>
```

---

## Arguments

| Argument | Description |
|---|---|
| `<WORKFLOW>` | Path to the `.oxoflow` workflow file |

---

## Options

| Option | Short | Description |
|---|---|---|
| `--format <FORMAT>` | `-f` | Output format: `ascii` (terminal), `dot` (Graphviz), `dot-clustered` (level-grouped), `tree` (indented tree), `mermaid` (Mermaid `graph LR`), `metro` (nf-metro metro map). Default: `ascii` |
| `--output <FILE>` | `-o` | Save output to a file (useful for dot/svg generation) |
| `--expanded` | | Show the DAG after wildcard/sample/scatter expansion (the actual runtime DAG) |
| `--verbose` | `-v` | Enable debug-level logging |
| `--quiet` | | Suppress non-essential output (errors only) |
| `--no-color` | | Disable colored output |
| `--json` | | Output machine-readable JSON to stdout |

---

## Examples

### Print ASCII graph to terminal (default)

```bash
oxo-flow graph pipeline.oxoflow
```

### Print DOT format

```bash
oxo-flow graph pipeline.oxoflow -f dot
```

### Render to PNG with Graphviz

The graph command prints log output (e.g. resource warnings) to stdout before the DOT body, so piping stdout into `dot` does not work reliably. Write the DOT to a file first, then render it:

```bash
oxo-flow graph -f dot -o graph.dot pipeline.oxoflow
dot -Tpng -o dag.png graph.dot
```

### Save DOT to file

```bash
oxo-flow graph pipeline.oxoflow -f dot -o graph.dot
```

### Render clustered view

```bash
oxo-flow graph pipeline.oxoflow -f dot-clustered -o clustered.dot
```

### Export a Mermaid diagram

`mermaid` emits standard Mermaid `graph LR` — no `%%metro` directives — so it renders directly on GitHub, in VS Code, and in any Mermaid renderer:

```bash
oxo-flow graph pipeline.oxoflow -f mermaid -o pipeline.mmd
```

### Export an nf-metro metro map

`metro` emits an [nf-metro](https://github.com/seqeralabs/nf-metro) definition — Mermaid `graph LR` extended with `%%metro` line/section directives — that renders as a transit-map-style SVG. Rules are grouped into colored "lines" by their analysis stage (inferred from shell keywords, or set explicitly via each rule's `tags`):

```bash
oxo-flow graph pipeline.oxoflow -f metro -o pipeline.mmd
# Render locally (requires nf-metro) or paste the .mmd content into the
# online playground at https://seqeralabs.github.io/nf-metro/latest/playground/
pip install nf-metro
nf-metro render pipeline.mmd -o pipeline.svg
```

### View the expanded runtime DAG

By default, `graph` shows the template DAG — one node per `[[rules]]` block, with every declared dataflow edge included (`input` paths and `expand_inputs` patterns alike). Use `--expanded` to show the actual runtime DAG after wildcard, sample, and scatter expansion: each generated task becomes its own node (rule names get a `_<group>_<sample>` or `_<pair_id>` suffix). Catalog pages render the template DAG — it is the stable overview; the expanded view is the full runtime truth.

```bash
oxo-flow graph pipeline.oxoflow --expanded
```

For example, a workflow with a `cohort` sample group of three samples shows the template DAG as 12 rules, while the expanded view shows 22 rules — one per (rule, sample) task:

```
┌────────────────────────────────────────────────┐
│  Workflow DAG: 22 rules, 28 dependencies       │
│  Depth: 12, Width: 3, Critical path: 12 steps  │
└────────────────────────────────────────────────┘

Level 0 (parallel: 3 rules)
┌─── fastp_qc_cohort_NA12878
│─── fastp_qc_cohort_NA12879
└─── fastp_qc_cohort_NA12880
     │
     ▼
Level 1 (parallel: 3 rules)
┌─── bwa_mem2_align_cohort_NA12878 [depends: fastp_qc_cohort_NA12878]
│─── bwa_mem2_align_cohort_NA12879 [depends: fastp_qc_cohort_NA12879]
└─── bwa_mem2_align_cohort_NA12880 [depends: fastp_qc_cohort_NA12880]
```

---

## Output Formats

### ASCII (default)

```
┌──────────────────────────────────────────────┐
│  Workflow DAG: 3 rules, 2 dependencies       │
│  Depth: 3, Width: 1, Critical path: 3 steps  │
└──────────────────────────────────────────────┘

Level 0 (sequential)
     generate_data
     │
     ▼
Level 1 (sequential)
     transform [depends: generate_data]
     │
     ▼
Level 2 (sequential)
     summarize [depends: transform]

Critical path: generate_data → transform → summarize
```

### DOT

```dot
digraph {
    0 [ label = "generate_data"]
    1 [ label = "transform"]
    2 [ label = "summarize"]
    0 -> 1 [ ]
    1 -> 2 [ ]
}
```

The `dot-clustered` format adds level-based clusters, `rankdir = TB` (top-to-bottom), and node/edge styling:

```dot
digraph workflow {
  rankdir=TB;
  node [shape=box, style="rounded,filled", fillcolor="#e8f0fe", fontname="Helvetica"];
  edge [color="#666666"];

  subgraph cluster_0 {
    label = "Level 0";
    style = dashed;
    color = "#cccccc";
    "generate_data";
  }

  subgraph cluster_1 {
    label = "Level 1";
    style = dashed;
    color = "#cccccc";
    "transform";
  }

  subgraph cluster_2 {
    label = "Level 2";
    style = dashed;
    color = "#cccccc";
    "summarize";
  }

  "generate_data" -> "transform";
  "transform" -> "summarize";
}
```

### Mermaid

The `mermaid` format emits standard Mermaid `graph LR` — a node per rule, an
edge per dependency:

```mermaid
graph LR
    n0["generate_data"]
    n1["transform"]
    n2["summarize"]
    n0 --> n1
    n1 --> n2
```

This renders directly in any Mermaid renderer (GitHub, VS Code, MkDocs) with no
extra tooling.

### Metro map (nf-metro)

The `metro` format emits an
[nf-metro](https://github.com/seqeralabs/nf-metro) definition — Mermaid
`graph LR` extended with `%%metro` directives — that renders as a
transit-map-style SVG. You can render it locally with `nf-metro render`
or paste the `.mmd` content into the
[nf-metro online playground](https://seqeralabs.github.io/nf-metro/latest/playground/)
to preview it without installing anything:

```mmd
%%metro line: generic | Analysis | #79706E

graph LR
    n0["generate_data"]
    n1["transform"]
    n2["summarize"]
    n0 -->|generic| n1
    n1 -->|generic| n2
```

Each rule is assigned a *stage* that becomes a colored "metro line":

- **Explicit**: the rule's first `tags` entry (e.g. `tags = ["align"]`),
  normalized through a small synonym table (`alignment` → `align`, etc.).
  Unknown tags become their own custom line.
- **Inferred**: keyword matching against the rule's `shell`/`script`
  commands — `fastqc`/`fastp` → QC/trim, `bwa`/`STAR` → align,
  `featureCounts`/`salmon` → quantify, `gatk`/`bcftools call` → variant,
  `multiqc` → report, and so on — with no match falling back to `generic`.

With more than one stage, stations are grouped into `subgraph` sections (one
per stage) and cross-stage edges are placed outside the sections as nf-metro
requires.

---

## Notes

- Default output is ASCII for terminal viewing
- DOT format requires Graphviz (`dot` command) to render images. Install with:
    - **macOS**: `brew install graphviz`
    - **Linux**: `apt install graphviz` or `yum install graphviz`
    - **Conda**: `conda install graphviz`
- Nodes represent rules, edges represent dependencies
- The `dot-clustered` format is laid out top-to-bottom (`rankdir = TB`); the plain `dot` format does not set a direction

### Understanding Metrics

The header shows key workflow metrics:

| Metric | Meaning |
|---|---|
| **Rules** | Total workflow rules (DAG nodes) |
| **Dependencies** | Total edges connecting rules |
| **Depth** | Critical path length (longest chain) |
| **Width** | Maximum parallelism (rules at same level) |

**Dependencies count:** The total number of edges in the DAG. When a rule has multiple input files from different upstream rules, each creates a separate edge. For example, a merge rule combining outputs from 3 parallel branches contributes 3 dependencies.
