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

---

## Notes

- Exits with code `0` on success, `1` on failure
- Validates both TOML parsing and DAG construction
- Missing input files are reported as warnings (not errors); `--as-include` skips DAG validation and input-existence checks
- Environments and tools are not verified — use `oxo-flow env check` for environment validation
- Run `validate` before `run` to catch errors early without consuming compute resources
