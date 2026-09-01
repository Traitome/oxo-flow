# `oxo-flow report`

Generate reports from workflow execution.

The report is built from **execution truth**: whatever happened in a run is
what the report shows — expanded commands that actually ran, per-rule exit
codes, real files with recorded checksums, and failure diagnosis for failed
runs. Without a checkpoint the report honestly shows template-level data
only, and never claims a workflow ran when it didn't.

---

## Usage

```
oxo-flow report [OPTIONS] [WORKFLOW]
```

`WORKFLOW` is optional: when omitted, the workflow (and its checkpoint) are
auto-discovered — see [Workflow discovery](#workflow-discovery).

---

## Arguments

| Argument | Description |
|---|---|
| `[WORKFLOW]` | Path to the `.oxoflow` workflow file (optional — auto-discovered when omitted) |

---

## Options

| Option | Short | Default | Description |
|---|---|---|---|
| `--format` | `-f` | inferred | Output format: `html`, `json`, `md`, `pdf`, or `pdf-command`. Inferred from the `-o` extension when omitted, else `html` |
| `--output` | `-o` | stdout | Output file path (`-` writes to stdout). For auto-discovered runs, defaults to `.oxo-flow/reports/report-<UTC timestamp>.html` (the filename stays `.html` even with `-f json`) |
| `--checkpoint` | — | `.oxo-flow/checkpoint.json` | Path to checkpoint file for execution metrics |
| `--run` | — | — | `DIR` — workdir of a previous run: the workflow and checkpoint are auto-discovered there. Conflicts with an explicit `WORKFLOW` |
| `--failed` | — | — | Failure-focused report: failure diagnosis is the first section (only when failures exist) |
| `--plan` | — | — | Template-only report — ignores execution data (no checkpoint required, no "no checkpoint" warning) |
| `--ai` | — | — | AI result interpretation — printed to stderr and included in the report as a marked AI-generated section |
| `--workdir` | `-d` | — | Working directory to look for `.oxo-flow` in (default: the workflow file's directory) |
| `--ci` | — | — | Reproducible output: pin the generation timestamp (`SOURCE_DATE_EPOCH` when set, else the Unix epoch) so identical state yields byte-identical reports |
| `--no-timestamps` | — | — | Omit the generation timestamp from the report |
| `--strict` | — | — | Fail (exit 2) when the checkpoint is missing or the configured template fails to render |
| `--diff` | — | — | `CHECKPOINT` — print a model-level diff of this report's checkpoint against another checkpoint (stderr, terminal-highlighted; the report still renders, exit code stays 0) |
| `--acct` | — | — | `PATH` — import sacct-style CSV accounting (JobID,JobName,State,Elapsed,CPUTime,MaxRSS) into a Resource Accounting section |
| `--r-data` | — | — | `DIR` — write R-friendly TSV files (`sample_table.tsv`, `metrics.tsv`) to DIR |
| `--versions-yml` | — | — | `PATH` — export an nf-core-style `versions.yml` of **declared** software per rule (docker `image:tag`, module `tool/version`, env files with sha256; `-` writes to stdout) and exit |
| `--list-sections` | — | — | List available report sections and exit (no workflow needed) |
| `--list-templates` | — | — | List available report templates and exit (no workflow needed) |
| `--init-template` | — | — | Write the built-in report template to `./report-template.tera` and exit (refuses to overwrite an existing file) |
| `--verbose` | `-v` | — | Enable debug-level logging |

### Exit codes

| Code | Meaning |
|---|---|
| `0` | Report generated (including template-only reports without a checkpoint; `--diff` always exits 0) |
| `1` | Rendering/usage error (e.g. unsupported format, `[report].format` set under `--strict`, `--diff` without both checkpoints) |
| `2` | Data source unavailable — under `--strict` when the checkpoint is missing or the configured template fails to render |

---

## Workflow discovery

When `WORKFLOW` is omitted, the workflow and checkpoint are discovered in
this order:

1. **`--run DIR`** — the previous run's workdir (its `.oxo-flow/`
   directory anchors the checkpoint).
2. **`--workdir DIR`** — or the current directory for zero-arg discovery.

Inside the discovery directory, the checkpoint at
`.oxo-flow/checkpoint.json` is loaded first (an explicit `--checkpoint
PATH` replaces it — that file is loaded instead):

- If it records a `workflow_path` that exists, that workflow file is used.
- Otherwise (no checkpoint, or one without a usable `workflow_path`), the
  directory is scanned for a unique `*.oxoflow` file — `main.oxoflow`
  takes priority, then the alphabetically first file. **Multiple**
  candidates are an error: a report is a one-shot artifact, so the command
  asks you to pass `WORKFLOW` explicitly rather than silently picking one.
- No workflow found suggests passing `WORKFLOW` explicitly or using
  `--plan` for a template-only report.

With `--plan`, no checkpoint is loaded at all: discovery goes straight to
the workflow scan, and the report shows template-level data only, without
the usual "no checkpoint" warning.

An auto-discovered run writes the report to
`.oxo-flow/reports/report-<UTC timestamp>.html` (path printed to stderr)
when no `-o` is given — the same directory that `run`'s auto-snapshots
fill (see [report snapshots](run.md#report-snapshots)).

---

## Examples

### Generate HTML report to stdout

```bash
oxo-flow report pipeline.oxoflow
```

### Discover and report on a previous run

```bash
# Zero-arg: workflow + checkpoint auto-discovered in the current directory
oxo-flow report

# Report on a specific previous run's workdir
oxo-flow report --run results/experiment1
```

### Write HTML to file

```bash
oxo-flow report pipeline.oxoflow -o report.html
```

### Generate JSON report (format inferred from the extension)

```bash
oxo-flow report pipeline.oxoflow -o report.json
```

The JSON is the versioned data model behind every view: it carries
`schema_version`, a `command: "report"` tag, and provenance fields
(`workflow_path`, `checkpoint_path`). All lists are sorted, and under
`--ci` the output is byte-reproducible — safe to diff, commit, and consume
with `jq`:

```bash
oxo-flow report pipeline.oxoflow --ci -o report.json && jq '.schema_version' report.json
```

### Generate Markdown report

```bash
# Git-friendly GFM tables — embed in docs or commit alongside the workflow
oxo-flow report pipeline.oxoflow -f md -o REPORT.md
```

### Generate PDF report

```bash
oxo-flow report pipeline.oxoflow -f pdf -o report.pdf
```

`-f pdf` requires the external `wkhtmltopdf` binary. If it is missing the
command degrades gracefully: it writes a printable HTML file (`report.html`)
and warns, instead of failing. Note that the wkhtmltopdf project upstream
has been archived — HTML output is the portable, supported-by-default
format.

### Output the PDF command (for scripting)

```bash
oxo-flow report pipeline.oxoflow -f pdf-command
```

### Failure-focused report

```bash
# Failure Diagnosis is the first section (when the run had failures)
oxo-flow report --run results/experiment1 --failed
```

### Template-only report

```bash
# Ignore execution data entirely — useful for previewing a workflow design
oxo-flow report pipeline.oxoflow --plan
```

### Enumerate available sections / templates

```bash
oxo-flow report --list-sections
oxo-flow report --list-templates
```

### Scaffold the built-in template

```bash
oxo-flow report --init-template   # writes ./report-template.tera
```

### Export R-friendly tables

```bash
oxo-flow report --run results/experiment1 --r-data analysis/
# writes analysis/sample_table.tsv (sample, group) and
#        analysis/metrics.tsv (rule, wall_time_secs, max_memory_mb, status)
```

### Diff two checkpoints

```bash
# Model-level diff on stderr (rules ±, status flips, benchmark deltas,
# checksum changes); the report still renders on stdout
oxo-flow report --run results/experiment1 --diff .oxo-flow/checkpoint.json
```

### Import cluster accounting

```bash
# sacct -o JobID,JobName,State,Elapsed,CPUTime,MaxRSS --parsable2
oxo-flow report --run results/experiment1 --acct sacct.csv
```

### Export declared software versions

```bash
# stdout
oxo-flow report pipeline.oxoflow --versions-yml -

# commit the file and diff it in CI to catch undeclared dependency drift
oxo-flow report pipeline.oxoflow --versions-yml versions.yml
```

Emits an nf-core-style `versions.yml` (also rendered as the report's
`software-versions` section): for every rule, what the workflow
**declares** — docker `image:tag` (registry kept as its own field), module
`(tool, version)`, conda/mamba/pixi/venv env files with sha256 content
hashes. The engine never executes anything to produce this data, so
resolved runtime package versions are **not** recorded — every entry
carries that caveat. Environment resolution follows the usual precedence
(env_group > rule.environment > defaults.environment).

---

## Data sources

The report reads three kinds of data, in this order of truth:

1. **Checkpoint** (`.oxo-flow/checkpoint.json`) — what actually happened:
   per-rule success/failure, exit codes, expanded commands, stderr excerpts
   (bounded, ~2 KiB per failed rule), wall time / peak memory / sampled CPU
   benchmarks, input file manifests (path, size, mtime), and output
   checksums (`sha256:`). Generated by `oxo-flow run`.
2. **Workflow config** — declared rules, environments, and `[report]`
   settings. Only used directly where execution data does not exist, and
   labeled as such ("declared template — no execution record").
3. **Runtime facts** — oxo-flow engine version and platform.

If no checkpoint exists, the report shows template-level data only with a
clear "No execution data" indicator — it never claims a run happened.

### Failure diagnosis

A failed run's report opens with a **Failure Diagnosis** section: for each
failed rule, the exit code, the downstream rules affected (transitive DAG
dependents), a stderr excerpt when available, and a suggested next step
based on common failure signatures (exit 127 command-not-found, exit 137
OOM-kill, exit 124 timeout, disk-full, …).

### Provenance

Every report records the oxo-flow version, the workflow file and its
`sha256` checksum, and the checkpoint path it was built from — the report
itself is an audit-trail anchor.

---

## Clinical Reporting

### Honest scope

The report system does **not** generate clinical data: no variant tables,
no biomarker summaries, no compliance audit records are produced. The
`clinical-compliance` section is a *static capability statement* — it
describes the classification frameworks modeled by oxo-flow's clinical
module, and says so explicitly. It only appears for explicitly
clinical-domain workflows or when listed in `[report].sections`; it is
never shown for generic workflows.

### ACMG/AMP Variant Classification (modeled, not generated)

**ACMG** (American College of Medical Genetics and Genomics) and **AMP**
(Association for Molecular Pathology) jointly published guidelines for the
interpretation of sequence variants. These guidelines define a five-tier
classification system:

| Tier | Classification | Clinical Meaning |
|------|---------------|-----------------|
| Tier I / Pathogenic | Strong clinical significance | Disease-causing; actionable for patient care |
| Tier II / Likely Pathogenic | Potential clinical significance | High confidence of disease association |
| Tier III / VUS | Uncertain significance | Insufficient evidence; re-evaluate periodically |
| Tier IV / Likely Benign | Probably not disease-causing | Strong evidence against pathogenicity |
| Benign | No clinical significance | Confirmed benign polymorphism |

oxo-flow's `VariantClassification` enum models both the somatic
(Tier I–IV, per AMP/ASCO/CAP) and germline (Pathogenic–Benign, per ACMG)
classification frameworks. Reporting on these classifications from
workflow data is future work; the report currently describes the modeled
frameworks only.

---

## Sections

Report sections are generated by a **pluggable system**; each generator
declares when it applies (e.g. `failure-diagnosis` only when rules failed,
`execution-status` only with a checkpoint). Filter them with
`[report].sections`:

```toml
[report]
sections = ["universal", "workflow-info", "commands"]
```

- Available built-in section IDs (generator names used by the filter):
  `universal`, `execution-status`, `failure-diagnosis`,
  `clinical-compliance`, `workflow-info`, `commands`, `file-manifest`,
  `environment`, `metrics`, `sample-matrix`, `provenance`, `task-summary`,
  `software-versions` —
  the rendered HTML id can differ from the generator name: `universal`
  renders the `dashboard` section, and `execution-status` renders both
  `execution-status` and `benchmarks`
- The filter applies to **all** sections, including `task-summary`
- Explicitly listing `clinical-compliance` includes it regardless of the
  detected workflow domain
- `--ai` and `--acct` sections are appended after the build and are never
  filtered — they were explicitly requested
- Custom section generators can be added by implementing the
  `ReportSectionGenerator` trait (see [report.rs
  source](https://github.com/Traitome/oxo-flow/blob/main/crates/oxo-flow-core/src/report.rs))

### Metrics (parsed from tool outputs)

The `metrics` section parses **real tool output files** found under the
run's working directory (the checkpoint-recorded workdir, else the
workflow file's directory — scanned recursively, skipping hidden
directories and symlinks) and renders one subsection per tool × sample:

| Tool | File pattern (sample prefix) | Metrics | Flags |
|---|---|---|---|
| fastp | `*.fastp.json` (`S1.fastp.json` → sample `S1`) | `total_reads`, `q30_rate`, `gc_content`, `duplication_rate` | `q30_rate`: Pass ≥ 0.85, Warn ≥ 0.75, else Fail; `duplication_rate` ≥ 0.5 Warn, else Info |
| samtools flagstat | `*.flagstat`, `*.flagstat.txt` | `total_reads`, `mapped_rate`, `properly_paired_rate` | `mapped_rate`: Pass ≥ 0.90, Warn ≥ 0.80, else Fail |
| STAR | `*Log.final.out` | `uniquely_mapped_pct`, `multimapping_pct` | `uniquely_mapped_pct`: Pass ≥ 70, Warn ≥ 60, else Fail |
| featureCounts | `*.summary` | `total_count`, `assigned_rate` | `assigned_rate`: Pass ≥ 0.60, Warn ≥ 0.40, else Fail |
| bcftools | `*.bcftools.stats` | `snps`, `indels`, `ts_tv_ratio` | Informational only (no thresholds) |
| kraken2 | `*.kraken2.report`, `*.kraken.report` | `unclassified_rate` | Pass ≤ 20, Warn ≤ 40, else Fail (percentage scale) |

Matching is case-insensitive; a file whose name is exactly the pattern
(e.g. `fastp.json`) is parsed without a sample attribution. Files that
match a known pattern but fail to parse are counted in a **Scan Notes**
subsection — a scanner that hid its gaps would look like full coverage.
The section is hidden entirely when nothing parses.

### Sample Matrix

With a checkpoint and sample definitions (`[[sample_groups]]` or
`[[pairs]]`), the `sample-matrix` section renders a rule × sample grid:
cells are `success` / `failed` / `-` per the engine's real expanded
instance names in the checkpoint (`{rule}_{group}_{sample}`,
`{rule}_auto-discovered_{sample}` for `sample_pattern` discovery, or
`{rule}_{pair_id}` for pairs). Rows are base rule names, sorted
failed-first so failing samples surface at the top.

### Software Versions (`software-versions` / `--versions-yml`)

The `software-versions` section is a static declaration extracted from
the workflow definition — the engine never executes anything to produce
it. Per rule it lists the declared backend: docker `image:tag` (registry
kept as its own field), module `(tool, version)` with a note when no
version segment exists, conda/mamba/pixi/venv env files with **sha256
content hashes**, or a "system environment" note when no environment is
declared. Resolved runtime package versions depend on the execution
environment and are deliberately **not** recorded — every entry carries
that caveat. Export a machine-readable copy with
`oxo-flow report --versions-yml PATH` and diff it in CI to catch
undeclared dependency drift.

### Resource Accounting (`--acct`)

`--acct <FILE>` imports an sacct-style CSV into a **Resource Accounting**
section: a `Rule / State / Elapsed / CPU Time / Max RSS (MB)` table plus
import provenance and an honest coverage note listing rules without a
record. Column detection is header-based and case-insensitive; `JobName`,
`State`, `Elapsed`, `CPUTime` and `MaxRSS` are required (`JobID` optional,
used to prefer the batch row over step rows). `Elapsed`/`CPUTime` accept
`MM:SS`, `HH:MM:SS` or `[D-]HH:MM:SS` (with optional fractional seconds);
`MaxRSS` accepts `K`/`M`/`G`/`T`/`c` suffixes (case-insensitive, base-1024,
rounded up so a peak is never under-reported). A missing required column is
a hard error — silently dropping a column would fake a complete table.

### Benchmarks CPU column

On **local** runs the **Benchmarks** table's CPU column shows **sampled**
CPU seconds: the executor's sampler reads each rule process's CPU time (all
its threads) at 200 ms ticks — child processes are not accumulated. Peak
memory is sampled the same way.

On **cluster** runs both columns come from the scheduler's accounting store
instead, read once as each job settles, and cover every step of the job.

`-` means neither source reported a number: very short local rules the
sampler never observed, LSF, a cluster job whose accounting row never
appeared, or legacy checkpoints written before sampling existed.

### Domain auto-detection

Workflow domains (DNA-seq, RNA-seq, epigenomics) are detected from the
tools a workflow's commands reference, and shown in the report's workflow
information. DNA tool signals (GATK/Picard/variant callers) take precedence
over RNA aligner signals, since variant-calling pipelines legitimately
contain `STAR`/`featureCounts` rules. `clinical` is never inferred from
commands — it is only reachable through explicit configuration.

---

## Notes

- If `--output` is not specified, the report is written to stdout
  (stderr carries diagnostics and the `--ai` interpretation — stdout stays
  pipe-safe: `oxo-flow report wf.oxoflow -f json | jq .` works). Exception:
  auto-discovered runs (zero-arg or `--run`) write to
  `.oxo-flow/reports/report-<UTC timestamp>.html` — and the filename stays
  `.html` even when `-f json`/`-f md` changed the content, so the default
  location is predictable
- HTML reports are self-contained single files: embedded CSS, dark-mode
  support, print styles, semantic landmarks, and HTML-escaped user
  content (rule names, commands, paths are safe to open and share)
- `--ai` includes the interpretation as an AI-generated report section
  (with the model name and a "review before relying" note); without an AI
  provider configured it degrades to the standard report with a warning
- `--no-timestamps` / `--ci` control the generation timestamp; `--ci`
  honors `SOURCE_DATE_EPOCH` for reproducible builds
- PDF output uses `wkhtmltopdf` for conversion (upstream archived; absent
  binary degrades to printable HTML): `brew install wkhtmltopdf` (macOS)
  or `apt install wkhtmltopdf` (Linux)
