# `oxo-flow report`

Generate reports from a workflow definition.

---

## Usage

```
oxo-flow report [OPTIONS] <WORKFLOW>
```

---

## Arguments

| Argument | Description |
|---|---|
| `<WORKFLOW>` | Path to the `.oxoflow` workflow file |

---

## Options

| Option | Short | Default | Description |
|---|---|---|---|
| `--format` | `-f` | `html` | Output format: `html` or `json` |
| `--output` | `-o` | stdout | Output file path |
| `--verbose` | `-v` | — | Enable debug-level logging |

---

## Examples

### Generate HTML report to stdout

```bash
oxo-flow report pipeline.oxoflow
```

### Write HTML to file

```bash
oxo-flow report pipeline.oxoflow -o report.html
```

### Generate JSON report

```bash
oxo-flow report pipeline.oxoflow -f json -o report.json
```

---

## Output

The HTML report is a self-contained single file with embedded CSS. The JSON report includes structured sections with workflow metadata, rule details, and configuration.

---

## Notes

- If `--output` is not specified, the report is written to stdout
- HTML reports can be opened directly in any web browser
- JSON reports are suitable for programmatic processing and integration with other tools
- The `[report]` section in the `.oxoflow` file can customize report templates and sections
