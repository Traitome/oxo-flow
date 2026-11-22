# oxo-flow diagnostics

Enumerate the diagnostic catalogs from the command line — read-only.

## Usage

```
oxo-flow diagnostics <ACTION> [WHAT] [--format <FORMAT>]
```

## Description

`oxo-flow diagnostics` exposes the engine's own diagnostic tables as data.
Two catalogs exist and both ship inside the binary:

- **runtime** — the 31 failure patterns the runtime
  [`DiagnosticsEngine`](../reference/diagnostics-engine.md) matches against
  captured stdout/stderr. Each entry reports its id, error category, one-line
  summary, whether it is auto-fixable, the config path that fixes it (when
  any), and the tool exit codes it covers.
- **scientific** — the 51 `SCI-*` scientific-preflight rules the
  [Scientific Preflight Engine](../reference/diagnostics-engine.md#scientific-preflight--biological-contract-catalog)
  emits while inspecting a DAG before execution. Each entry reports its
  stable code and one-line summary.

The command reads the tables the engine itself uses
(`DiagnosticsEngine::catalog` and `sci_catalog`), so it can never enumerate
an id the engine does not know — and a test asserts the command's ids equal
the internal catalog ids exactly. Adding a pattern or rule to either table
without surfacing it (or vice versa) fails that test.

One action is available:

- `list [WHAT]` — enumerate the catalogs. `WHAT` is `runtime`, `scientific`,
  or `all` (the default), and selects which catalogs are listed.

JSON is the default output (machine-readable on stdout, stable key order —
the runtime patterns are id-sorted); `--format text` prints a human-readable
summary.

## Arguments

| Argument | Description |
|----------|-------------|
| `[WHAT]` | Which catalog to list: `runtime`, `scientific`, or `all` (default) |

## Options

| Option | Description |
|--------|-------------|
| `--format <FORMAT>` | `json` (default) or `text` |
| `-v, --verbose` | Enable verbose (debug-level) logging |
| `--quiet` | Suppress non-essential output (version banner; run-time narration) |
| `--no-color` | Disable colored output. Also respects the `NO_COLOR` environment variable |
| `--json` | Output machine-readable JSON to stdout (human-readable logs stay on stderr) |

## Examples

```bash
# Both catalogs as JSON (default)
oxo-flow diagnostics list

# Only the runtime failure patterns
oxo-flow diagnostics list runtime --format text

# Only the SCI-* scientific preflight rules
oxo-flow diagnostics list scientific

# Machine-readable pipe
oxo-flow diagnostics list all --json | jq '.runtime.count, .scientific.count'
```

## Output

`diagnostics list all` (JSON) carries both catalogs, each with a `count` and
its records:

```json
{
  "action": "list",
  "command": "diagnostics",
  "runtime": {
    "count": 31,
    "patterns": [
      {
        "auto_fixable": true,
        "category": "data",
        "exit_codes": [1],
        "fix_config_path": null,
        "id": "bam_index_missing",
        "summary": "BAM index file (.bai) not found. Most tools (samtools, IGV) require a sorted and indexed BAM."
      }
    ]
  },
  "scientific": {
    "count": 51,
    "rules": [
      {
        "code": "SCI-AGG-RACE",
        "summary": "Fan-out dimensions not keyed by declared outputs cause concurrent races."
      }
    ]
  },
  "what": "all"
}
```

`--format text` prints a compact human-readable list:

```text
31 runtime diagnostic patterns:
- bam_index_missing (Data) [auto-fixable] — BAM index file (.bai) not found. ...

51 scientific preflight rules:
- SCI-AGG-RACE — Fan-out dimensions not keyed by declared outputs cause concurrent races.
...
```

## Notes

- The command is read-only and never touches the network or a workflow file:
  it enumerates the catalogs compiled into the binary.
- Counts in the examples above are the current values; `diagnostics list`
  reports the values for your build.
- A `WHAT` that is not `runtime`, `scientific`, or `all` (and an unknown
  `--format`) exits non-zero with a diagnostic naming the valid choices.
- The catalogs are the same tables the web domain re-exports
  (`crates/oxo-flow-web/src/domains/execution/diagnostics.rs`); this command
  is the CLI surface for them.
