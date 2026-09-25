# `oxo-flow diff`

Compare two `.oxoflow` workflow files and show differences.

---

## Usage

```
oxo-flow diff <WORKFLOW_A> <WORKFLOW_B>
```

---

## Arguments

| Argument | Description |
|---|---|
| `<WORKFLOW_A>` | First workflow file |
| `<WORKFLOW_B>` | Second workflow file |

---

## Examples

### Compare two workflows

```bash
oxo-flow diff v1.oxoflow v2.oxoflow
```

---

## Output

```
Diff: 2 difference(s) between v1.oxoflow and v2.oxoflow:
  • [rules] rule "bwa_align": shell command changed
  • [config] config variable changed: "threads"
```

---

## Notes

- Performs a semantic comparison of workflow structures, not just a line-by-line diff
- Detects changes in workflow metadata (name, version, description), added/removed rules, per-rule inputs/outputs/shell/threads/memory/environment, config variables, and defaults
- Useful for tracking changes during pipeline development
- Output goes to **stdout**; like `diff(1)`, the exit code is `0` when the
  workflows are identical and `1` when differences are found (printed as
  `✓ Workflows are identical`), so scripts can branch on either
