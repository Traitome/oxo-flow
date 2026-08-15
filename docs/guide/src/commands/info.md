# `oxo-flow info`

Derive catalog metadata from a workflow file — the machine-checkable subset
that the oxo-community catalog shows on every workflow page (including the
Parameters table).

---

## Usage

```
oxo-flow info [OPTIONS] <WORKFLOW>
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
| `--format <FORMAT>` | | Output format: `json` (default, machine-readable on stdout) or `text` (human-readable summary) |

---

## Output (JSON)

```json
{
  "name": "simple-variant-calling",
  "version": "0.13.0",
  "rule_count": 6,
  "tools": ["alignment", "fastp", "gatk", "qc"],
  "resources": { "max_threads": 16, "max_memory": "32G" },
  "environments": { "conda": 3, "singularity": 4 },
  "config": [
    {
      "key": "reference",
      "default": "/data/reference/GRCh38.fa",
      "value_type": "string",
      "used_by": ["apply_bqsr", "base_recalibrator", "bwa_align", "haplotype_caller"]
    },
    {
      "key": "out_dir",
      "default": "results",
      "value_type": "string",
      "used_by": [],
      "description": "Output directory (upstream: --outdir)."
    }
  ],
  "sample_groups": [],
  "pairs": [],
  "references": [],
  "input_dirs": ["raw"],
  "output_dirs": ["aligned", "qc", "variants"]
}
```

- **`config`** — every `[config]` parameter with its default value, its
  derived type (`string` / `int` / `float` / `bool` / `array` / `table`), the
  rules that reference it (`{config.<key>}` in shells, scripts, inputs, or
  outputs, plus brace-less `config.<key>` in `when` conditions), and — when
  the workflow comments the key — its `description` taken verbatim from the
  `[config]` section comments. Declared parameters
  (`key = { default, type, … }`) render their typed default from the
  declaration metadata. Keys sorted, rule lists sorted.
  Engine-injected keys are excluded — the run-time churn keys
  (`samples_list`, `pairs_list`, `samples_*`), the reference keyed-config
  values (`config.<reference name>` = its output), and the
  `reference_dir`-derived paths — `config_keys` carries the bare key list.
- **`config[].description`** — optional; present only when the key is
  commented. A contiguous block of `#` lines immediately above the key line
  (no blank line in between) is the description, joined into one line; a
  trailing `#` comment on the key line itself is the fallback. Empty `#`
  lines and pure decorator lines (`# ----`) are dropped. Banner lines
  (`# --- section ---`) act as section markers: dropped when the block also
  has regular text, otherwise kept with their dashes stripped. Comments
  inside multi-line values (e.g. `#` lines within an array) never associate,
  and comments above `[config.<name>]` subtable headers describe the table
  key itself.
- **`tools`** — conda/mamba environment YAML stems and container image names
  (registry path and tag stripped), deduped and sorted.
- **`resources`** — max threads/memory across rules on the defaults-applied
  view (the same value the engine uses at run time), in the winning rule's
  original string format.

---

## Examples

### Inspect a workflow before running it

```
oxo-flow info --format text workflow/rnaseq.oxoflow
```

```
Workflow: rnaseq v0.13.0
Rules: 44
Config keys: fasta = refs/genome.fa, reads_dir = test/fixtures/raw, ...
```

### Feed the catalog pipeline

The oxo-community catalog derives its per-workflow Parameters tables from the
JSON output and commits it as `data/configs.json` in the site repository —
regenerated with `scripts/regen-configs.py` whenever a workflow changes.
