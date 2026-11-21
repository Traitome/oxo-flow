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

## Global Options

Like all oxo-flow commands, `diff` accepts the global flags `--verbose` /
`-v` (debug-level logging), `--quiet` (suppress informational output — the
version banner is omitted; the `Diff:` report and the exit code are
unaffected), `--no-color` (disable colored output; also respects the
`NO_COLOR` environment variable), and `--json` (emit one machine-readable
diff document on stdout; the human/colored report is suppressed — see
[JSON output](#json-output) below).

---

## Examples

### Compare two workflows

```bash
oxo-flow diff v1.oxoflow v2.oxoflow
```

### Emit a machine-readable diff

```bash
oxo-flow diff --json v1.oxoflow v2.oxoflow
```

---

## Output

```
Diff: 2 difference(s) between v1.oxoflow and v2.oxoflow:
  • [rules] rule "bwa_align": shell command changed
  • [config] config variable changed: "threads"
```

---

## JSON output

With `--json`, `diff` writes **one** JSON document to stdout and suppresses
the human report, so CI and tooling can consume the diff directly:

```json
{
  "difference_count": 2,
  "differences": {
    "config": [
      "config variable changed: \"threads\""
    ],
    "rules": [
      "rule \"bwa_align\": shell command changed"
    ]
  },
  "equal": false,
  "tool": "oxo-flow diff",
  "workflow_a": "v1.oxoflow",
  "workflow_b": "v2.oxoflow"
}
```

- `equal` is `true` when the workflows are identical (`differences` is then
  `{}` and `difference_count` is `0`).
- `differences` is an object keyed by diff category (`workflow`, `rules`,
  `config`, `defaults`, `pairs`, `sample_groups`); keys appear in sorted
  order and the descriptions within each category are sorted, so repeated
  runs on the same pair emit byte-identical JSON.
- The exit code is unchanged: `0` when identical, `1` when differences are
  found.

---

## Notes

- Performs a semantic comparison of workflow structures, not just a line-by-line diff
- Detects changes in workflow metadata (name, version, description), added/removed rules, per-rule inputs/outputs/shell/threads/memory/environment, config variables, and defaults
- Useful for tracking changes during pipeline development
- Output goes to **stdout**; like `diff(1)`, the exit code is `0` when the
  workflows are identical and `1` when differences are found (printed as
  `✓ Workflows are identical`), so scripts can branch on either
