# `oxo-flow export`

Export a workflow to a container definition or standalone TOML.

---

## Usage

```
oxo-flow export [OPTIONS] <WORKFLOW>
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
| `--format` | `-f` | `docker` | Export format (`docker`, `singularity`, `toml`) |
| `--output` | `-o` | stdout | Output file path |

---

## Examples

### Export to Dockerfile

```bash
oxo-flow export pipeline.oxoflow -f docker
```

### Export to Singularity definition

```bash
oxo-flow export pipeline.oxoflow -f singularity -o Singularity.def
```

### Export to standalone TOML

```bash
oxo-flow export pipeline.oxoflow -f toml -o bundle.oxoflow
```

---

## Output

```
oxo-flow 0.4.1 — Bioinformatics Pipeline Engine
✓ Exported docker to stdout
FROM ubuntu:22.04
RUN apt-get update && apt-get install -y ...
...
```

---

## Notes

- Container exports include all environment requirements specified in the workflow
- TOML export bundles all includes into a single, standalone workflow file
- Useful for archiving workflows or deploying to restricted environments
