# `oxo-flow validate`

Validate a `.oxoflow` workflow file. Checks TOML syntax, rule definitions, and DAG construction (including cycle detection).

---

## Usage

```
oxo-flow validate <WORKFLOW>
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
✗ pipeline.oxoflow — expected `=`, found newline at line 15 column 1
```

### Circular dependency

```
✗ pipeline.oxoflow — DAG error: cycle detected involving rule "align"
```

---

## Notes

- Exits with code `0` on success, `1` on failure
- Validates both TOML parsing and DAG construction
- Does not check that referenced files, environments, or tools actually exist — use `oxo-flow env check` for environment validation
- Run `validate` before `run` to catch errors early without consuming compute resources
