# `oxo-flow validate`

Validate a `.oxoflow` workflow file. Checks TOML syntax, rule definitions, and DAG construction (including cycle detection).

---

## Usage

```
oxo-flow validate [OPTIONS] <WORKFLOW>
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
| `--as-include` | — | Validate as a sub-workflow fragment (skips input-existence checks and DAG construction; **cycle detection still applies**) |
| `--json` | — | Output machine-readable JSON to stdout |
| `--ai` | — | Enable AI-powered semantic validation |
| `--verbose` | `-v` | Enable debug-level logging |

---

## Examples

### Validate a workflow

```bash
oxo-flow validate pipeline.oxoflow
```

---

## Output

### Valid workflow

```
✓ pipeline.oxoflow — 5 rules, 4 dependencies

  ⚠ Warning: The following input files do not exist:
    - refs/genome.fa
```

`--json` prints the full result object (stdout):

```json
{
  "command": "validate",
  "workflow": "pipeline.oxoflow",
  "valid": true,
  "rules": 5,
  "dependencies": 4,
  "errors": [],
  "missing_inputs": ["refs/genome.fa"]
}
```

### Invalid TOML syntax

```
✗ pipeline.oxoflow — parse error in pipeline.oxoflow: TOML parse error at line 15, column 1
```

### Circular dependency

```
  error [E006]: DAG error: cycle detected in workflow DAG: align → sort_bam → align
    hint: check for circular dependencies between rules
✗ pipeline.oxoflow — 1 validation error(s)
```

### Wildcard input without a sample domain

An input containing a sample placeholder (`{sample}`, `{group}`, `{pair_id}`, …)
resolves only when the workflow declares a sample domain
(`[[sample_groups]]`, `[[pairs]]`, or `sample_pattern`). Without one the
run fails mid-flight with a literal brace token, so `validate` reports
the path as missing instead of approving it:

```
✓ wildin.oxoflow — 1 rule, 0 dependencies

  ⚠ Warning: The following input files do not exist:
    - {sample}.txt (no sample groups/pairs/sample_pattern declared)
```

`--json` includes the same entry in `missing_inputs`. That field is a
free-text diagnostic list, not a stable machine interface: entries mix
plain paths (file simply absent on disk) with paths annotated with a
trailing `(...)` reason, so consumers must not exact-match the strings
or split on whitespace — match on the path prefix instead.

---

## Notes

- Exits with code `0` on success, `1` on failure
- Validates TOML parsing, rule semantics, and DAG construction
- Missing input files are reported as warnings (not errors); `--as-include` skips input-existence checks and DAG construction but cycle detection still applies (cycles are semantic errors, not missing files)
- Relative paths resolve against the workflow file's directory — the same base rules run from — so warnings are accurate even when invoked from another directory
- Environments and tools are not verified here — use
  [`oxo-flow test --deep`](test.md#deep-checks-deep) (checks environment
  definition files D002 and PATH binaries D003) or `oxo-flow env check`
  for environment validation
- Run `validate` before `run` to catch errors early without consuming compute resources
- [`lint`](lint.md) is a strict superset: it runs all `validate` checks plus
  style linting and secret scanning
