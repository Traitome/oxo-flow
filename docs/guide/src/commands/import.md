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
- `cpus` → `threads`, `memory` (`GB`/`MB` canonicalized), `time` → `resources.time_limit`
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
  (`output: "${x}.count"` → `data/{sample}.txt.count`)
- BioType contracts only when inference actually recognizes the tool/files
  (`[types.fastq]`, `[types.bam_coord]`, `[types.vcf_indexed]` are emitted
  **only** when a rule's inferred contract references them)

### Snakemake

- Rule names, `threads`, `resources.mem_mb` / `resources.time`
- `conda:` / `container:` directives → `environment`
- Literal input/output patterns (including `expand(...)` patterns and
  `temp()` / `protected()` markers)
- The `shell:` body

## What is not extracted

- **Nextflow workflow-local variables** (`def args = task.ext.args ?: ''`):
  the `def` prelude is stripped so bash does not receive `def: command not
  found`, and the import prints a warning naming the variable. References to
  it remain as shell text and may expand to nothing at run time.
- **Nextflow channel operators and expressions** (`params.*`, `task.ext.*`,
  `Channel.map`/`join`/`combine`, ternaries, closures): unsupported
  interpolations are left verbatim and warned about by name.
- **Process wiring / dependencies**: each process becomes an independent rule;
  `depends_on` edges are not inferred. Add them by hand after import.
- **Multiple-wildcard globs** (`Channel.fromFilePairs("data/*_{1,2}.fastq.gz")`)
  and character-class globs: only the first `*` maps to `{sample}`; the import
  warns loudly.
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
