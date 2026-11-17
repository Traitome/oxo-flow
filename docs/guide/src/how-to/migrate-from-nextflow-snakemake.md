# Migrating from Nextflow or Snakemake

This guide is the end-to-end path from an existing Nextflow DSL2 or Snakemake
pipeline to a runnable oxo-flow workflow. It assumes you have read the
[`oxo-flow import` reference](../commands/import.md); this page is about
planning the migration and verifying the result.

**There is no semantic-parity claim.** The importer translates a deterministic
subset and refuses everything else with a named error. A clean import means the
workflow is structurally valid, not that it reproduces the source engine's
behavior for every tool and dataset.

## 1. Choose the migration path

Use this decision tree before writing anything:

1. **Import when the source fits the supported subset.** Run
   `oxo-flow import` and read its warnings. If every warning is a metadata
   note (tag/label/publishDir) and every construct is in the
   [supported table](../commands/import.md#what-is-translated), the generated
   workflow is a faithful deterministic translation you can validate and run.
2. **Hand-port at full fidelity when the source depends on dynamic logic.**
   Groovy channel operators, closures, `params` wiring, Snakemake `run:` /
   `script:` bodies, lambdas, and dynamic resources have no deterministic
   mapping. Port those rules by hand using the
   [community porting playbook](https://oxo-flow-community.github.io/about/porting/).
3. **Reuse an existing community port when one exists.** Search the
   [pipeline catalog](https://oxo-flow-community.github.io/pipelines/) before
   writing a new port. Catalog entries carry `engine`, `origin`, and
   `source.repo` metadata so you can check whether the upstream is already
   covered.

The common outcome is a mix: import the static skeleton, then hand-port the
dynamic steps and join them in the `.oxoflow` file.

## 2. End-to-end workflow

### 2.1 Inventory the source

List the processes/rules, their file inputs/outputs, environments, resource
directives, and any dynamic construct. Treat this as the fidelity checklist in
section 4. Note which constructs the importer will reject so you can plan the
manual work.

### 2.2 Import

```bash
oxo-flow import main.nf -o pipeline.oxoflow
# or
oxo-flow import Snakefile -o pipeline.oxoflow
```

Warnings go to stderr; a hard error means no output file was written. Resolve
each error in the source (or plan a hand-port) before moving on.

### 2.3 Triage and fix

Read every warning. The recurring ones:

- `label` / `publishDir` — `nextflow.config` resource settings and output
  publishing are not imported. Set `resources` and copy outputs explicitly.
- `task.ext.when` — the standard nf-core gate is accepted, but the rule runs
  unconditionally. Add an oxo-flow `when` if the gate matters.
- `stub` — test-only body, intentionally ignored.
- sink rule dropped (Snakemake `rule all`) — expected; the producing rules
  remain.

### 2.4 Run the static gates

```bash
oxo-flow validate pipeline.oxoflow
oxo-flow lint pipeline.oxoflow
oxo-flow dry-run pipeline.oxoflow
```

| Gate | Catches | Does not catch |
|---|---|---|
| `validate` | Schema/format errors, DAG cycles, wildcard sources (W024), BioType (`TYPE-*`) mismatches, undefined `env_group` references | Missing files, wrong tool flags, scientific validity |
| `lint` | Best-practice issues (`W0xx`): unbound wildcards, deprecated keys, output collisions, unsafe paths | Runtime behavior |
| `dry-run` | Resolved command preview, missing inputs, resource totals, fan-out plan, cluster/scratch problems | Whether a tool accepts the command |

All three are static: none executes a tool or proves equivalence with the
source pipeline. Use `--json` on each for machine-readable reports.

### 2.5 Fix environments, resources, and runtime knobs by hand

The importer cannot see `nextflow.config` profiles or Snakemake command-line
resource overrides. Before the first real run:

- **Environments** — check every `environment = { … }`: conda/container specs
  are literal strings from the source; HPC `modules` are mapped only when no
  other backend is declared.
- **Resources** — `cpus`/`memory`/`time`/`disk`/`queue`/`retries` are mapped;
  label-derived and `nextflow.config`-derived limits are not. Set them in
  `resources`.
- **Wildcards and samples** — declare where `{sample}`/`{group}` values come
  from (`sample_pattern`, `[[sample_groups]]`, `[[pairs]]`, or `[[values]]`).
  A wildcard with no declared source is a W024 warning, not an error, and it
  stays literal at run time.
- **Scratch** — route large temporary files away from the system volume with
  `[engine] tmp_dir` (or `OXO_FLOW_TMPDIR` per invocation).
- **Cluster** — author a `profiles/<name>.toml` with the scheduler queue and
  limits, then use `oxo-flow run --profile <name>`. The importer does not
  generate profiles.
- **Provenance** — add `--provenance` when you want SHA-256 output checksums
  recorded in the checkpoint.

```bash
oxo-flow run pipeline.oxoflow --provenance
oxo-flow provenance verify .oxo-flow/checkpoint.json
```

### 2.6 Pilot, then scale

Run a small subset first (`--samples first:2`), inspect the report snapshot,
and only then scale. Import-time warnings and dry-run output are the cheapest
place to catch a mistranslated path.

## 3. Supported subset and hard-error contract

The authoritative construct-by-construct tables live in
[`oxo-flow import`](../commands/import.md#what-is-translated). In summary:

| Area | Nextflow DSL2 | Snakemake |
|---|---|---|
| Unit | `process` blocks | `rule` blocks |
| I/O | `path`/`file` channels, literal patterns, workflow wiring (call/pipe/upstream/emit) | quoted literals, single-pattern `expand`, `rules.X.output` |
| Resources | `cpus`, `memory`, `time`, `disk`, `queue`, `maxRetries`, `scratch` | `threads`, `priority`, `retries`, `resources` (mem/time/gpu/disk) |
| Environment | literal `conda`/`container`, `module`, `env` | `conda`/`container`/`singularity`/`envmodules`, `envvars` |
| Script | `$task.cpus`, input variables, `meta.id`, `task.ext.args/prefix`, `def` args/prefix preludes | `{input}`/`{output}`/`{wildcards.X}`/`{threads}`/`{log}`/`{benchmark}` |
| Metadata | `tag` ignored; `label`/`publishDir` warned | `log`/`benchmark` mapped |
| Sink rules | n/a | `rule all` dropped with a warning |

Everything else is a **hard error that names the construct** (parse-level errors carry a source line) — dynamic resources,
channel operators, `include`, `params`, `run:`/`script:` bodies, lambdas,
directory outputs, dynamic containers, indexed input tuples, unresolved
interpolations, and unknown directives. The importer never writes a workflow
it could not translate, and it parses plus statically validates the generated
TOML before returning.

## 4. Fidelity checklist

Before calling a migration complete, verify:

- [ ] **Branches and conditions** — every `when`/`wildcard_constraints`/gate is
      either translated or hand-ported; no rule runs (or is skipped)
      differently from the source intent.
- [ ] **Tools and versions** — each rule's conda/container spec names the same
      tool version family as the source; `label`-derived resources are written
      explicitly.
- [ ] **File naming** — every output is a concrete filename or an expression
      anchored to an input/`meta.id`; no bare globs survived by hand-editing.
- [ ] **Inputs and wildcards** — the sample domain is declared; `validate`
      reports no unbound-wildcard (`W024`) warnings for production rules.
- [ ] **Resources and scheduling** — threads/memory/time/disk/queue/retries
      match the source profile; `dry-run` shows feasible totals.
- [ ] **Outputs and publishing** — `publishDir` behavior is reproduced with
      explicit copies or `export`; outputs are not silently left in the workdir
      when downstream systems expect a published path.
- [ ] **Provenance and reports** — `--provenance` is enabled where needed and
      the report snapshot captures the expected rules.
- [ ] **Pilot run** — a 1–2 sample run completes, and its report matches the
      source pipeline's expected artifacts.

## 5. Community path

The catalog and the porting playbook are maintained by the oxo-flow community:

- [Porting playbook](https://oxo-flow-community.github.io/about/porting/) —
  full-fidelity hand-porting guidance for constructs the importer rejects.
- [Pipeline catalog](https://oxo-flow-community.github.io/pipelines/) —
  curated ports, each carrying `engine`, `origin`, and `source.repo` metadata.

Check the catalog before porting: an existing entry may already cover the
upstream pipeline. If you port something new, contribute it back with the same
metadata so the next migration can find it.

**Compatibility caveat.** The community porting playbook is currently pinned
to an older engine (0.12.0). Verify any syntax you take from it against the
current [workflow format reference](../reference/workflow-format.md) before
running it; the playbook is guidance, not a version-matched specification.

## 6. What this guide does not promise

- No semantic equivalence with Nextflow or Snakemake.
- No support for Groovy/Python evaluation, network fetching, or dynamic code.
- No automatic conversion of `nextflow.config` profiles, labels, or
  `publishDir` behavior.
- A successful import is a structural starting point; scientific correctness
  still requires review plus the validate → lint → dry-run → pilot sequence.
