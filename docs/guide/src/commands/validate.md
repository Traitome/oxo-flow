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
| `--as-include` | — | Validate as a sub-workflow fragment (skips input-existence checks, the pixi-manifest preflight, and DAG construction; **cycle detection still applies**) |
| `--json` | — | Output machine-readable JSON to stdout |
| `--ai` | — | Enable AI-powered semantic validation |
| `--verbose` | `-v` | Enable debug-level logging (global) |
| `--quiet` | — | Suppress informational output, including the version banner (global) |
| `--no-color` | — | Disable colored output, also respects the `NO_COLOR` environment variable (global) |

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
  "warnings": [],
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
✗ pipeline.oxoflow — DAG error: cycle detected in workflow DAG: align → sort_bam → align
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

### Unbound wildcard (W024 surfaced in validate)

A rule that uses a wildcard no rule in the workflow declares a source for
(`output = ["out/{sample}.txt"]` with no `[[sample_groups]]`, `[[pairs]]`,
`sample_pattern`, or `[[values]]` entry) keeps the brace token **literal**
at run time and then fails with a precise hard error. `lint` warns about
this (`W024`); `validate` — the first gate in the recommended
validate → dry-run → run path — now surfaces the same `W024` diagnostic
so a green validate no longer hides a workflow that cannot run:

```
✓ p.oxoflow — 1 rule, 0 dependencies

  warning [W024]: wildcard '{sample}' has no declared source — it will stay literal at run time and files may be written literally named after the placeholder (rule: step1)
    hint: declare a source for the wildcard (sample_pattern in [config], [[sample_groups]], [[pairs]], or a [[values]] table), or remove the placeholder
```

Warnings keep exit code `0` — only errors fail. `validate` scopes itself
to `W024` (runnability); the full best-practice sweep remains
[`lint`](lint.md)'s job, and the `warnings` array is empty when nothing
blocks runnability. At run time the corresponding unbound-placeholder
WARN is emitted once per (rule, placeholder) — a 200-sample fan-out logs
the line once, not once per instance.

---

## Notes

- Exits with code `0` on success, `1` on failure
- Validates TOML parsing, rule semantics, and DAG construction
- Missing input files are reported as warnings (not errors). A rule's declared pixi manifest, by contrast, is existence-checked and reported as error `E019` (a missing environment file blocks every rule that needs it at run time); `--as-include` skips input-existence checks, the pixi-manifest preflight, and DAG construction, but cycle detection still applies (cycles are semantic errors, not missing files)
- Relative paths resolve against the workflow file's directory — the same base rules run from — so warnings are accurate even when invoked from another directory. This includes the pixi-manifest preflight: `pixi = "envs/x.toml"` is checked at `<workflow dir>/envs/x.toml`, never against the invoking CWD
- Environments and tools are not verified here — the pixi-manifest check only confirms the manifest file exists, not that the pixi binary is installed or that the environment solves. Use
  [`oxo-flow test --deep`](test.md#deep-checks-deep) (checks environment
  definition files D002 and PATH binaries D003) or `oxo-flow env check`
  for environment validation
- Run `validate` before `run` to catch errors early without consuming compute resources
- [`lint`](lint.md) is a strict superset: it runs all `validate` checks plus
  style linting and secret scanning
