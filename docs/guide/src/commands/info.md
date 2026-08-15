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
  "version": "1.0.0",
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
  derived type (`string` / `int` / `float` / `bool` / `array` / `table`), and
  the rules that reference it: `{config.<key>}` in shells, scripts, inputs, or
  outputs, plus brace-less `config.<key>` in `when` conditions. Keys sorted,
  rule lists sorted. Engine-injected keys (`samples_list`, `pairs_list`,
  `samples_*`) are excluded; `config_keys` carries the bare key list.
- **`tools`** — conda/mamba environment YAML stems and container image names
  (registry path and tag stripped), deduped and sorted.
- **`resources`** — max threads/memory across rules, in the winning rule's
  original string format.

---

## Examples

### Inspect a workflow before running it

```
oxo-flow info --format text workflow/rnaseq.oxoflow
```

```
Workflow: rnaseq v1.0.0
Rules: 44
Config keys: fasta = refs/genome.fa, reads_dir = test/fixtures/raw, ...
```

### Feed the catalog pipeline

The oxo-community catalog derives its per-workflow Parameters tables from the
JSON output and commits it as `data/configs.json` in the site repository —
regenerated with `scripts/regen-configs.py` whenever a workflow changes.
