# oxo-flow knowledge

Query the embedded knowledge base from the command line — read-only.

## Usage

```
oxo-flow knowledge <ACTION> [--format <FORMAT>]
```

## Description

`oxo-flow knowledge` exposes the same data surfaces the AI agent tools use
through a deterministic, script-friendly CLI. The data ships inside the
binary (no network access, no configuration required) and is refreshed by
the scheduled knowledge-refresh pipeline (see
[Embedded Knowledge Freshness](../reference/ai-cli.md#embedded-knowledge-freshness)).

Four actions are available:

- `search <query>` — cross-source unified search across Bioconda CLI tools,
  the registry (nf-core modules + commercial tools + bio.tools overlay), and
  embedded pipeline skills.
- `get <kind> <name>` — exact fetch of one record (case-insensitive).
  `kind` is one of `tool` (Bioconda), `module` (registry: nf-core /
  commercial / bio.tools), `skill` (pipeline skill; names carry a `bio-`
  prefix, e.g. `bio-gatk-variant-calling`), or `doc` (full SKILL.md
  protocol document; names bridge through pipeline-graph node ids).
- `list <what>` — embedded indexes: `docs` (skill document ids) or `domains`
  (skill domains with record counts).
- `stats` — per-source record counts with freshness (auto-generated sources
  older than the staleness window are flagged), plus live record counts.

JSON is the default output (machine-readable on stdout, stable key order);
`--format text` prints the same human-readable summaries the AI tool layer
sees.

## Options

| Option | Description |
|--------|-------------|
| `--format <FORMAT>` | `json` (default) or `text` |
| `-v, --verbose` | Enable verbose (debug-level) logging |
| `--quiet` | Suppress non-essential output (errors only) |
| `--no-color` | Disable colored output |
| `--json` | Accepted for consistency with other commands; per-command output is controlled by `--format` |

## Examples

```bash
# Cross-source search
oxo-flow knowledge search "variant calling"
oxo-flow knowledge search samtools --format text

# Exact fetch
oxo-flow knowledge get tool samtools
oxo-flow knowledge get module fastqc
oxo-flow knowledge get skill bio-gatk-variant-calling
oxo-flow knowledge get doc variant-calling

# Indexes and freshness
oxo-flow knowledge list domains
oxo-flow knowledge list docs
oxo-flow knowledge stats
```

## Output

`stats` (JSON) groups an embedded-meta section (name, count, generated_at,
auto, stale per source) with live parsed counts:

```json
{
  "command": "knowledge",
  "action": "stats",
  "sources": [
    {
      "name": "bioconda_tools",
      "count": 6667,
      "generated_at": "2026-11-01T04:12:31Z",
      "auto": true,
      "stale": false
    }
  ],
  "live_counts": {
    "bioconda_tools": 6667,
    "registry": 4083,
    "skills": 562,
    "skill_docs": 77,
    "graph_nodes": 543,
    "graph_edges": 1187
  }
}
```

`get tool samtools --format text` prints the record the AI tool layer sees
(`L`/`M` are the bioconda platform codes for linux-64 and osx-64):

```text
samtools 1.25 — Tools for dealing with SAM, BAM and CRAM files
platforms: L, M
```

## Notes

- The command is read-only and never touches the network: all data is
  embedded at build time by `crates/oxo-flow-ai/src/knowledge/`.
- Record counts in examples are illustrative — check `knowledge stats` for
  the values in your build.
- The same data powers the AI tools (`lookup_tool`, `lookup_skill`,
  `lookup_pipeline`, `lookup_doc`); the CLI exists so humans and scripts can
  consult it without invoking an AI provider.
- Failed exact fetches exit non-zero with a diagnostic that points at the
  nearest index action.
