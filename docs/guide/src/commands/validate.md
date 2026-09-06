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
| `--as-include` | — | Validate as a sub-workflow fragment (skips DAG and input-existence checks) |
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
```

### Invalid TOML syntax

```
✗ pipeline.oxoflow — parse error in pipeline.oxoflow: TOML parse error at line 15, column 1
```

### Circular dependency

```
✗ pipeline.oxoflow — DAG error: cycle detected in workflow DAG: cycle detected: align → sort_bam → align
```

### Wildcard input without a sample domain

An input containing a sample placeholder (`{sample}`, `{group}`, `{pair_id}`, …)
resolves only when the workflow declares a sample domain
(`[[sample_groups]]`, `[[pairs]]`, or `sample_pattern`). Without one the
run fails mid-flight with a literal brace token, so `validate` reports
the path as missing instead of approving it:

```
⚠ Warning: The following input files do not exist:
  - missing/{sample}.txt (no sample groups/pairs/sample_pattern declared)
```

`--json` includes the same entry in `missing_inputs`.

---

## Notes

- Exits with code `0` on success, `1` on failure
- Validates both TOML parsing and DAG construction
- Missing input files are reported as warnings (not errors); `--as-include` skips DAG validation and input-existence checks
- Relative paths resolve against the workflow file's directory — the same base rules run from — so warnings are accurate even when invoked from another directory
- Environments and tools are not verified here — use
  [`oxo-flow test --deep`](test.md#deep-checks-deep) (checks environment
  definition files D002 and PATH binaries D003) or `oxo-flow env check`
  for environment validation
- Run `validate` before `run` to catch errors early without consuming compute resources
- [`lint`](lint.md) is a strict superset: it runs all `validate` checks plus
  style linting and secret scanning
