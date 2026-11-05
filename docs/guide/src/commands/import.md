# oxo-flow import

Transpile external Nextflow DSL2 and Snakemake pipelines into native `.oxoflow` workflows.

## Usage

```
oxo-flow import [OPTIONS] <SOURCE>
```

## Description

The `import` command automatically transpiles Nextflow DSL2 process definitions
and Snakemake rules into typed `.oxoflow` pipelines.

It extracts rule names, CPU threads, memory allocations, walltime limits, conda/docker
environments, inputs, outputs, and shell commands. In addition, it performs biological
typing inference to attach `[rules.types]` contracts based on tool semantics, ensuring
immediate validation by the BioTypes type checker.

When `--from` is omitted, `oxo-flow import` auto-detects the source format from the
file extension (`.nf` for Nextflow, `Snakefile` or `.smk` for Snakemake) or through
content inspection.

## Options

| Option | Description |
|--------|-------------|
| `<SOURCE>` | Path to the Nextflow (`.nf`) or Snakemake (`Snakefile`, `.smk`) source file |
| `--from <FORMAT>` | Explicit source format: `nextflow` or `snakemake` (auto-detected if omitted) |
| `-o, --output <OUTPUT>` | Output `.oxoflow` file (prints to stdout if omitted) |

## Examples

Transpile a Nextflow module and print to stdout:
```bash
oxo-flow import modules/fastqc.nf
```

Transpile a Snakemake pipeline directly to a `.oxoflow` workflow file:
```bash
oxo-flow import Snakefile -o workflow.oxoflow
```

Transpile with explicit format specification:
```bash
oxo-flow import custom_pipeline.txt --from nextflow -o pipeline.oxoflow
```
