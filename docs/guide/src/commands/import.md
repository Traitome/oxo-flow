# oxo-flow import

> **Migrating a whole pipeline? Start here:** [Migrating from Nextflow or Snakemake](../how-to/migrate-from-nextflow-snakemake.md) walks the decision tree, the gate sequence, and the fidelity checklist. This page is the command reference.

Transpile a **supported deterministic subset** of Nextflow DSL2 or Snakemake
into a native `.oxoflow` workflow.

## Usage

```
oxo-flow import [OPTIONS] <SOURCE>
```

## Description

`oxo-flow import` extracts process/rule definitions, resource directives,
environments, literal I/O patterns, and shell bodies into `.oxoflow`. It is a
**deterministic translator, not an interpreter**: there is no semantic parity
with the source engine, and every construct outside the documented subset is a
hard error that names the construct (parse-level errors carry a source line). The importer never invents a path, drops
a semantic directive, or writes a workflow it could not translate.

Before returning, the generated TOML is parsed and statically validated by the
engine (`WorkflowConfig::parse` + `validate_format`). If the emitter ever
produces an unrunnable workflow, the import fails and no output file is written.

## What is translated

### Nextflow DSL2

| Construct | Behavior |
|---|---|
| `process NAME { … }` | One rule per process (name lowercased) |
| `cpus` / `memory` / `time` | `resources.threads` / `resources.memory` / `resources.time_limit`; unparseable or dynamic values are errors |
| `disk` / `queue` | `resources.disk` / `resources.partition` |
| `scratch true` | `scratch = true` |
| `maxRetries N` | `retries = N` |
| `errorStrategy 'terminate'` / `'retry'` | Accepted; `ignore` / `finish` are errors |
| `conda` / `container` | `environment` (literal specs only; dynamic values are errors) |
| `module` | `environment.modules` (HPC modules) |
| `env NAME, "value"` / `env 'NAME=value'` | `envvars` |
| `tag` | Ignored (cosmetic) |
| `label` / `publishDir` | Warning: resource settings from `nextflow.config` and output publishing are not imported |
| `when: task.ext.when == null \|\| task.ext.when` | Accepted with a warning (the rule runs unconditionally); any other `when` is an error |
| `stub:` | Ignored with a warning (test-only body) |
| `input: path/file` channels | Literal patterns or workflow-supplied variables; `val`/`tuple val` metadata is ignored |
| `output: path/file` | **Every** path expression on the declaration, including tuple members; `emit:` labels are kept |
| `script:` / `shell:` | Double-quoted, triple-quoted, and single-quoted bodies |
| `$task.cpus` | `{threads}` |
| `$task.ext.args` / `def args = task.ext.args ?: ''` | `{config.ext_args}`, with `[config] ext_args = ""` emitted |
| `$task.ext.prefix` / `def prefix = task.ext.prefix ?: "${meta.id}"` / `${meta.id}` | `{sample}` |
| Declared input variables (`$reads`, `${reads}`) | `{input}` / `{input[N]}` by declaration position |
| `versions.yml` output | Skipped (the engine records versions separately) |
| Workflow wiring | Call, pipe, channel-variable, upstream-output, and upstream-emit forms (table below) |

**Hard errors (no output written):**

- `include` / `includeConfig` (the include graph is not followed; concatenate
  the files or import each module).
- Dynamic `cpus`/`memory`/`time`/`container`/`conda`, `errorStrategy
  'ignore'|'finish'`, `maxForks`, `cache`, `storeDir`, `beforeScript`,
  `afterScript`, `resourceLimits`, `machineType`, `accelerator`,
  `clusterOptions`, `penv`, `exec:`, and unknown process directives.
- `stdin` and `env(...)` inputs.
- A bare output glob (`path("*.bam")`): the produced filename is unknown, and
  guessing would mistranslate the pipeline. Use an input-anchored expression
  (`${reads}.bam`, `${meta.id}.bam`) or a concrete filename.
- Indexed input references (`${reads[0]}`, `$reads[0]`): oxo-flow exposes
  whole inputs, not per-file tuple indexing. Split the channel or hand-port.
- Unresolved interpolations (`${params.x}`, unknown `meta.*`, `task.ext.*`
  other than args/prefix, `Channel.*` in a script, project/runtime paths).
- A single-quoted script that references a declared input variable: Nextflow
  does not interpolate single-quoted strings, so mapping the variable would
  change the command.
- Fan-in and mixed outputs: a wildcard input that appears in **no** output of
  the rule (e.g. `expand("quant/{sample}.sf", …)` feeding a constant
  `results/report.txt`, or `rules.<name>.output` into a constant output), or a
  wildcard fan-out whose output set mixes wildcard and constant paths (e.g.
  `out/{sample}.bam` plus `reports/summary.txt`). Every instance would write
  the constant output, and a wildcard input with no matching output has no
  fan-out source. Use the engine's `expand_inputs` fan-in construct
  (workflow-format reference) or hand-port the rule.
- A process declared with `path` inputs but never invoked in a `workflow {}`
  block (the input source is unknown).
- A process invoked more than once in the workflow block.
- Input globs with more than one `*` or character classes, and multi-output
  `PROC.out` selections without `.out.<emit>`.

### Snakemake

| Construct | Behavior |
|---|---|
| `rule NAME:` | One rule per rule block; duplicate names are errors |
| `input:` / `output:` | Quoted literal patterns; fan-OUT `expand("pattern/{wildcard}", …)` where the rule's output carries the same wildcard; `rules.<name>.output` (single-output rules only) |
| `temp(…)` / `protected(…)` / `ancient(…)` | `temporary` + `temp_output`, `protected_output`, `ancient` |
| `touch(…)` | Treated as a plain output with a warning (the engine verifies declared outputs exist) |
| `threads:` / `priority:` / `retries:` | Rule fields |
| `resources: mem_mb, mem_gb, time/runtime, gpu, disk_mb` | `resources` (exact MB; other resource keys are errors) |
| `conda:` / `container:` / `singularity:` / `envmodules:` | `environment`; container wins over conda (warning); combining `envmodules` with conda/container is an error |
| `log:` / `benchmark:` | Rule `log` / `benchmark` fields; `{log}` / `{benchmark}` placeholders are translated |
| `envvars:` | `envvars` |
| `localrules:` / global `wildcard_constraints:` | Rule flags / `[wildcard_constraints]` |
| `shell:` | Single-line and triple-quoted bodies |
| `{input}` / `{input.name}` / `{output.name}` / `{wildcards.X}` / `{threads}` | Engine placeholders (`{input[N]}` by declared position) |
| `rule all` (target-only sink) | Dropped with a warning; an oxo-flow rule with no command would be rejected by `validate` |

**Hard errors (no output written):** `params:`, `run:`, `script:`, `notebook:`,
per-rule `wildcard_constraints:`, `shadow:`, `group:`, `default_target:`,
`handlers:`, `include:`, `subworkflow`, `module`, `use rule`, `configfile:`,
`ruleorder:`, `onsuccess:`, `onerror:`, `directory(...)`, lambda/config/params/
checkpoints-based dynamic input/output, unknown directives, unknown resource
keys, duplicate rule names, fan-in/mixed outputs (a wildcard input with no
matching output wildcard, or wildcard inputs alongside a constant output that
every instance would overwrite — use `expand_inputs` or hand-port), and
`{params.*}` / `{resources.*}` / `{rule}` / `{checkpoints.*}` placeholders in
`shell:`.

For a fan-out `expand(pattern, key=[...])`, the explicit value list is **not
materialized**: the pattern is imported, but the wildcard domain must be
declared with `sample_pattern`, `[[sample_groups]]`, `[[pairs]]`, or
`[[values]]` before running.

Brace text the importer does not recognize (bash `${VAR}`, brace expansion,
awk `{print $1}`) passes through unchanged.

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
input, and the argument count must match the declaration count. Any other
wiring expression aborts the import with the construct named.

## After importing

The import warnings are the migration checklist. Always run the static gates
in order:

```bash
oxo-flow validate pipeline.oxoflow
oxo-flow lint pipeline.oxoflow
oxo-flow dry-run pipeline.oxoflow
```

`validate` checks the workflow contract and BioTypes, `lint` reports best
practices, and `dry-run` previews the resolved commands and missing inputs.
None of them executes a tool or proves scientific equivalence — see
[Migrating from Nextflow or Snakemake](../how-to/migrate-from-nextflow-snakemake.md)
for the full checklist.

When `--from` is omitted, `oxo-flow import` auto-detects the source format from
the file extension (`.nf` for Nextflow, `Snakefile` or `.smk` for Snakemake) or
through content inspection.

## Options

| Option | Description |
|--------|-------------|
| `<SOURCE>` | Path to the Nextflow (`.nf`) or Snakemake (`Snakefile`, `.smk`) source file |
| `--from <FORMAT>` | Explicit source format: `nextflow` or `snakemake` (auto-detected if omitted) |
| `-o, --output <OUTPUT>` | Output `.oxoflow` file (prints to stdout if omitted). Existing files are never overwritten |
| `--json` | Emit one machine-readable JSON document on stdout (see [JSON output](#json-output)) instead of raw TOML |

## Measured execution equivalence (real-world corpus)

`scripts/import-execution-equivalence.sh` runs each supported fixture's
SOURCE engine and the IMPORTED `.oxoflow` workflow on identical planted inputs
and compares the complete produced-artifact path→SHA-256 map (planted inputs,
workflow seeds, and engine metadata are excluded). When the imported workflow
declares literal outputs, each must also be present in both arms, and a
fixture whose `execution` block pins a container must import to that same
container (scheme-insensitive); otherwise the fixture is SKIPPED. A fixture is
MATCHED only when **both** arms genuinely execute and produce artifacts; a
missing runtime, missing environment backend, or non-zero exit is SKIPPED with
the reason — never MATCHED. Two executed arms that write nothing are an
explicit NO_ARTIFACTS verdict.

As of 2026-11-27, two real-world fixtures execute on both arms and match:

- an nf-core `samtools/faidx` module (upstream commit cited in the fixture),
  run as Nextflow `-with-docker` versus imported; and
- a community Snakemake tutorial `index_reference` rule, run with
  `--use-singularity` versus imported.

Both arms used the pinned `htslib_samtools:1.25` image and produced a
byte-identical `.fai` (SHA-256 `696683592f087bb9aa2bf8cb2b9bd85df1b482c809c3770d86c9450d629d6cfe`).
Over the eight supported fixtures the measured tally was executed 2 /
matched 2 / drifted 0 / skipped 6. The six skips: four name a missing backend
(three conda/mamba, one HPC modules), one source fixture is not valid DSL2 for
the installed Nextflow engine, and one (`samtools_sort.ok.nf`) is an imported
arm whose wildcard expansion finds no planted BAM (its source arm exits 0
without a declared artifact). No skip is counted as a match. See
[`import-execution-realworld-2026-11-27.md`](../../../superpowers/audits/import-execution-realworld-2026-11-27.md).

```bash
# source vs imported on identical inputs (engine binaries must be on PATH)
scripts/import-execution-equivalence.sh --bin ./target/debug/oxo-flow
# falsifiable: exit non-zero only if an executed fixture drifted
scripts/import-execution-equivalence.sh --strict --bin ./target/debug/oxo-flow
```

## Examples

Transpile a Nextflow pipeline and print to stdout:

```bash
oxo-flow import main.nf
```

Transpile a Snakemake pipeline to a file:

```bash
oxo-flow import Snakefile -o workflow.oxoflow
```

Transpile with an explicit format:

```bash
oxo-flow import custom_pipeline.txt --from nextflow -o pipeline.oxoflow
```

Emit a machine-readable translation verdict instead of raw TOML:

```bash
oxo-flow import Snakefile --json
```

## JSON output

`--json` emits one JSON document on stdout describing the translation, so CI and
tooling can assert what was accepted and what was lost without parsing colored
stderr. The colored human warnings and the raw TOML echo are suppressed — stdout
stays a single JSON document. The exit code is unchanged (a translation warning
is a report, not a failure). With `-o`, the file is still written and the
document is still emitted.

| Field | Description |
|-------|-------------|
| `tool` | Always `"oxo-flow import"` |
| `source` | The source path that was transpiled |
| `format` | Detected or requested source format: `nextflow` or `snakemake` |
| `accepted` | `true` when no translation warnings were produced; `false` when a source construct was not mapped faithfully |
| `warnings` | Ordered array of `{ "message": "..." }` objects, one per translation loss (empty when `accepted` is `true`) |
| `toml` | The generated oxo-flow TOML workflow |

```json
{
  "tool": "oxo-flow import",
  "source": "Snakefile",
  "format": "snakemake",
  "accepted": false,
  "warnings": [
    {
      "message": "line 7: `benchmark:` becomes a declared path, but oxo-flow does not auto-populate benchmark files — the rule's command must write it"
    }
  ],
  "toml": "[workflow]\nname = \"bwa_mem_pipeline\"\n..."
}
```

> **Next:** [Migrating a whole pipeline from Nextflow or Snakemake](../how-to/migrate-from-nextflow-snakemake.md)
