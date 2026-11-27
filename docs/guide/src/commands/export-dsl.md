# oxo-flow export-dsl

> **Migrating a whole pipeline?** `export-dsl` is the reverse lane of
> [`import`](import.md): it hands an oxo-flow workflow back to Nextflow. This
> page is the command reference; the subset contract mirrors the importer's.

Export a `.oxoflow` workflow back to a **Nextflow DSL2 document** over the
same deterministic subset the importer accepts. It is the export half of the
transpiler (issue #1319): a round-trip lane, not a universal converter.

## Usage

```
oxo-flow export-dsl [OPTIONS] <WORKFLOW>
```

## Description

`export-dsl` reads the engine's **parsed workflow model** (`WorkflowConfig`
plus its DAG) and emits one Nextflow DSL2 document with one `process` block
per rule, in declaration order, plus a `workflow { … }` wiring block. Every
emitted construct is chosen so that re-importing the document with
`oxo-flow import --from nextflow` reproduces the declared artifacts — for
the measured fixture corpus, the round trip is **byte-identical**.

The same honesty policy as the importer applies in reverse:

- **Hard error, never silent loss.** Anything outside the round-trippable
  subset aborts the export naming the construct (the rule, the field, or
  the section). The gate is exhaustive: each rule and the whole document
  are checked against the handled field allowlists, so a model field with
  no DSL equivalent can never be dropped silently.
- **Deterministic.** Output is a pure function of the parsed workflow:
  rules emit in declaration order, environment variables and maps are
  sorted, no timestamps, no hash-map iteration. Emitting the same workflow
  twice yields byte-identical text.
- **No second representation.** The emitter reuses the parsed model the
  engine already validates; the DAG is built first, so a cyclic workflow
  never reaches the emitter and every wired input is cross-checked against
  the DAG's edges.

## What is exported

### Per-rule mapping

| `.oxoflow` construct | Emitted Nextflow DSL2 |
|---|---|
| `[[rules]] name` (lowercase identifier) | `process NAME` |
| `resources.threads` (when ≠ 1) | `cpus N` |
| `resources.memory` / `time_limit` / `disk` / `partition` | `memory` / `time` / `disk` / `queue` directives |
| `environment.conda` / `docker` / `modules` | `conda` / `container` / `module` directives (one backend only — combined declarations are errors, mirroring Nextflow precedence) |
| `envvars` | `env "NAME", "value"` directives (sorted) |
| `scratch = true` | `scratch true` |
| `retries N` (when > 0) | `maxRetries N` |
| `input = ["…"]` | `input:` block with positional `path in0`, `path in1`, … channels |
| `output = ["…"]` | `output:` block with `path("…")` expressions |
| `shell` | `script:` block, body verbatim |
| `description` | DSL comment (sanitized) |

### Placeholder mapping

| Engine placeholder | Nextflow interpolation |
|---|---|
| `{threads}` | `$task.cpus` |
| `{sample}` | `${meta.id}` |
| `{config.ext_args}` | `$task.ext.args` |
| `{input}` (single-input rule) | `$in0` |
| `{input[N]}` | `$inN` |

Uppercase shell-environment variables (`$PATH`) pass through verbatim — the
importer preserves them as shell env too.

### Wiring

The `workflow { … }` block wires every rule that declares inputs, in
declaration order:

- an input pattern another rule produces → `PRODUCER.out`; when the
  producer has several outputs, the exporter attaches deterministic
  positional emit labels (`o1`, `o2`, …) to its outputs and wires
  `PRODUCER.out.oK`, so the re-import selects the same output;
- a source input pattern → `Channel.fromPath("glob")`, with exactly one
  `*` standing for `{sample}` (the importer maps it back).

## Hard errors (nothing is written)

- Workflow or rule fields with no DSL equivalent: `[engine]`, `[[values]]`,
  `[[pairs]]`, `references`, `cluster`, `webhook`, rule `log`/`benchmark`/
  `temporary`/`when`/`priority`/`target`/`params`/`extends`, GPU resources,
  venv/mamba/singularity/pixi environments, and every other unhandled key —
  each error names the construct.
- `[config]` keys other than `ext_args` (only `$task.ext.args` maps back
  through the importer). Engine-injected keys (`samples_list`,
  `paired_pairs_count`, …) are excluded from the check — the parse step
  adds them, they are not authoring surface.
- Ordering-only `depends_on` (no input produced by the dependency): Nextflow
  wiring feeds data channels, so the edge would not survive re-import.
- Rule names that are not lowercase `[a-z_][a-z0-9_]*` identifiers (the
  importer lowercases process names — the identity would drift).
- Non-list `input`/`output` forms (named maps, directory inputs).
- Source input patterns with wildcards other than a single `{sample}`.
- Shell constructs the importer rejects or skips: non-shell-env `$`
  interpolations, indexed refs, `def` preludes, version blocks, `Channel.*`,
  `{output}`-style placeholders, `"""`, and leading/trailing whitespace that
  the importer's body trim would strip (a trailing newline on a multi-line
  body is fine — it is what the importer itself serializes).
- `--format snakemake` (or any other target): the exporter covers Nextflow
  DSL2 only — it refuses rather than guessing. CWL/WDL export remains out
  of scope (see the roadmap).

## Determinism and round-trip fidelity

Two exports of the same workflow are byte-identical (pinned by tests). The
fixture-corpus round trip is measured in `tests/transpile_roundtrip.rs`: of
the 27 `tests/fixtures/import/**` fixtures, 19 are rejected by the importer
(no parsed model to export), **5** accepted fixtures export and re-import
**byte-identically** (the 4 accepted Nextflow fixtures plus the Snakemake
fixture `training_faidx.ok.smk`, whose imported model carries none of the
unexportable fields), and the 3 accepted Snakemake fixtures that declare
`[wildcard_constraints]`, rule `benchmark`, or rule `log` are refused naming
those constructs (Nextflow has no equivalent for what Snakemake import emits
there).

`[types.*]` contracts and per-rule `types` are not emitted (Nextflow has no
equivalent); the importer re-derives them from tool-name/IO inference on
re-import, so machine-imported workflows carry the same contracts back. A
hand-authored contract that deviates from that inference will not survive
verbatim — the export warns about it.

## Options

| Option | Description |
|--------|-------------|
| `-o, --output <FILE>` | Write the DSL to a file (prints to stdout if omitted). Refuses to overwrite an existing file — same "generators never clobber" convention as `import` |
| `--format <FORMAT>` | Target format; `nextflow` (default) is the only implemented target — anything else is refused by name |
| `-v, --verbose` | Enable verbose (debug-level) logging |
| `--quiet` | Suppress informational output |
| `--no-color` | Disable colored output |
| `--json` | Output machine-readable JSON to stdout |

## JSON output

With `--json`, one machine-readable document is emitted on stdout (the
`import --json` precedent):

```json
{
  "tool": "oxo-flow export-dsl",
  "workflow": "pipeline.oxoflow",
  "format": "nextflow",
  "accepted": true,
  "warnings": [],
  "dsl": "process demo {\n    ..."
}
```

- `accepted` — `true` when the warnings list is empty (a warning means a
  survivable nuance, e.g. re-derived `types` contracts); hard
  incompatibilities abort the command with a named-construct error instead.
- The document is byte-stable: two exports of the same workflow emit
  identical JSON.
- With `-o`, the file is written exactly as in the human path and the
  document is still emitted.

## Examples

```bash
# Round-trip a workflow to Nextflow on stdout
oxo-flow export-dsl workflow.oxoflow

# Write a .nf file for a Nextflow project
oxo-flow export-dsl workflow.oxoflow -o nextflow/main.nf

# Machine-readable export report
oxo-flow export-dsl workflow.oxoflow --json
```

## Notes

- The exporter is prototype-grade and deliberately narrow: one direction
  (`.oxoflow` → Nextflow DSL2) over the subset the importer documents.
  Snakemake export does not exist — asking for it is a hard error, not a
  best-effort translation.
- The workflow name, version, and declared `sample_pattern` travel as DSL
  comments: the importer re-derives the name (`<first-rule>_pipeline`),
  the version (`0.1.0`), and the sample pattern from the wiring, so they
  are context, not claims.

## See Also

- [oxo-flow import](import.md) — the forward direction; the documented
  subset contract lives there
- [LIMITATIONS.md](https://github.com/Traitome/oxo-flow/blob/main/LIMITATIONS.md)
