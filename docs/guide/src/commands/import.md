# oxo-flow import

Transpile external Nextflow DSL2 and Snakemake pipelines into native `.oxoflow` workflows.

## Usage

```
oxo-flow import [OPTIONS] <SOURCE>
```

## Description

The `import` command transpiles Nextflow DSL2 process definitions and
Snakemake rules into `.oxoflow` pipelines. It is a **best-effort translator**,
not a full language implementation: every construct it cannot map faithfully
is either reported as a warning on stderr or rejected with a clear error — it
never silently emits an empty command or an invented file path.

## What is extracted

### Nextflow

- Rule names (`process NAME` → lowercased rule name)
- `cpus` → `resources.threads`, `memory` (`GB`/`MB` canonicalized) →
  `resources.memory`, `time` → `resources.time_limit`; a process without
  `cpus` inherits `[defaults] threads`
- `conda` / `container` directives → `environment`
- The `script:` or `shell:` body, in both inline quoted form
  (`script: "wc -l < ${x}"`) and triple-quoted multi-line form
  (`script: """ ... """`), including `'''…'''` bodies
- Declared `path`/`file` input variables, and `${var}` / `$var` references to
  them inside the script → oxo-flow placeholders (`{input}`, `{input[N]}`)
- `$task.cpus` → `{threads}`, `${meta.id}` / `${prefix}` → `{sample}`
- A direct `Channel.fromPath("data/*.txt")` (or a `name = Channel.fromPath(…)`
  variable) feeding a process → the real wildcard input
  `data/{sample}.txt`, a matching `sample_pattern`, and input-derived outputs
  (`output: "${x}.count"` → `data/{sample}.txt.count`). Both invocation forms
  are supported — the call form `COUNT(Channel.fromPath("data/*.txt"))` and
  the canonical pipe form `Channel.fromPath("data/*.txt") | COUNT` — and
  produce identical TOML.
- Process wiring in the documented forms (see
  [Nextflow wiring forms](#nextflow-wiring-forms)): a downstream input fed by
  an upstream output is mapped to the upstream's output pattern and gets a
  `depends_on` edge.
- BioType contracts only when inference actually recognizes the tool/files
  (`[types.fastq]`, `[types.bam_coord]`, `[types.vcf_indexed]` are emitted
  **only** when a rule's inferred contract references them). A wired input
  inherits the type of the upstream output it consumes, so a derived
  intermediate never re-inherits the source file's type.

### Snakemake

- Rule names, `threads`, `resources.mem_mb` / `resources.time`
- `conda:` / `container:` directives → `environment`
- Literal input/output patterns (including `expand(...)` patterns and
  `temp()` / `protected()` markers)
- The `shell:` body

## Nextflow wiring forms

The importer resolves a process's input channels, in the order the process
declares them, from exactly these forms:

| Form | Example | Result |
|---|---|---|
| Call, direct source | `COUNT(Channel.fromPath("data/*.txt"))` | input `data/{sample}.txt` + `sample_pattern` |
| Call, channel variable | `reads = Channel.fromPath("data/*.txt")` then `COUNT(reads)` | same as above |
| Pipe, direct source | `Channel.fromPath("data/*.txt") \| COUNT` | same as above |
| Pipe, channel variable | `reads \| COUNT` | same as above |
| Upstream output | `ALIGN(TRIM.out)` | input = TRIM's single output pattern; adds `depends_on = ["trim"]` |
| Upstream emit | `ALIGN(TRIM.out.bam)` | input = TRIM's `emit: bam` output pattern; adds the edge |
| Upstream-output pipe | `TRIM.out.bam \| ALIGN` | same as `TRIM.out.bam` above |

Wiring is positional: argument *i* feeds the process's *i*-th declared `path`
input, and the argument count must match the declaration count.

**Loud-abort contract.** Any wiring expression outside the table above aborts
the import with an error naming the construct — for example a channel
operator chain (`Channel.fromPath(…).map { … } | COUNT`), an unbound channel
variable, a chained pipe (`Channel.fromPath(…) | A | B`), a multi-output
`PROC.out` with no `.out.<emit>` selector, a reference to an undefined
upstream process, or an argument/declaration count mismatch. The importer
never falls back to a fabricated path or type and never writes an output file
on this path. Resolve the channel in the source or import that rule by hand.

## What is not extracted

- **Nextflow workflow-local variables** (`def args = task.ext.args ?: ''`):
  the `def` prelude is stripped so bash does not receive `def: command not
  found`, and the import prints a warning naming the variable. References to
  it remain as shell text and may expand to nothing at run time.
- **Nextflow channel operators and expressions** (`params.*`, `task.ext.*`,
  `Channel.map`/`join`/`combine`, ternaries, closures): unsupported
  interpolations are left verbatim and warned about by name. Wiring
  expressions using these operators abort the import (see above) rather than
  being guessed.
- **Multiple-wildcard globs** (`Channel.fromFilePairs("data/*_{1,2}.fastq.gz")`)
  and character-class globs: only the first `*` maps to `{sample}`; the import
  warns loudly. Such a channel used as a wiring source aborts instead.
- **Output expressions that reference unknown variables** (e.g.
  `"${params.outdir}/x.txt"`): the output is omitted with a warning rather
  than fabricated.
- **Processes with no `script:`/`shell:` body**: the import fails with a clear
  message instead of writing `shell = ""` (such a rule would be skipped at run
  time; `validate` reports it as error `E022`).

## After importing

Always run `oxo-flow validate` and `oxo-flow lint` on the generated workflow,
then `oxo-flow dry-run` to confirm the DAG and command preview. Because the
translation is best-effort, expect to edit the result — the import warnings
are the checklist of what needs manual attention.

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
