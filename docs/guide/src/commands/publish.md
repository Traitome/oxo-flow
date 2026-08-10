# oxo-flow publish

Bundle a workflow with its environment files into a verifiable, self-contained archive for sharing, archival, or remote execution.

## Usage

```
oxo-flow publish [OPTIONS] <WORKFLOW>
```

## Description

Reads the `.oxoflow` workflow file, recursively follows `[[include]]` references
to discover all environment spec files, collects `scripts/` and `bin/`
directories, and produces a single `.tar.zst` archive containing:

- The workflow file
- All referenced environment files (conda, mamba, pixi, venv)
- `scripts/` and `bin/` directories (Nextflow-style auto-PATH convention)
- `manifest.json` with per-file SHA-256 checksums and container image references

The archive is self-contained and checksum-verified — consumers can verify
every file's integrity before execution.

With `--with-lockfiles`, also generates deterministic conda lockfiles for
each environment YAML, ensuring exact reproducibility across time.

---

## Options

| Option | Short | Description |
|---|---|---|
| `--output` | `-o` | Output path for the bundle archive (default: `<name>-bundle.tar.zst`) |
| `--with-lockfiles` | | Generate `conda-lock` lockfiles for reproducible environments |

## Examples

```bash
# Publish a workflow
oxo-flow publish my_pipeline.oxoflow
# → my_pipeline-bundle.tar.zst

# Publish with custom output path
oxo-flow publish my_pipeline.oxoflow -o /path/to/bundle.tar.zst

# Publish with deterministic lockfiles
oxo-flow publish my_pipeline.oxoflow --with-lockfiles

# Run a published bundle (extract → verify → execute)
oxo-flow run --bundle my_pipeline-bundle.tar.zst -j 16

# Pull a remote bundle and run it
oxo-flow pull gh:user/repo@v0.10.0
oxo-flow run --bundle repo-bundle.tar.zst
```

## Manifest Format

The `manifest.json` inside each bundle:

```json
{
  "format": "oxoflow-bundle-v1",
  "workflow": "my_pipeline.oxoflow",
  "oxo_flow_version": "0.9.0",
  "created_at_epoch": 1234567890,
  "entrypoint": "my_pipeline.oxoflow",
  "files": [
    {
      "path": "my_pipeline.oxoflow",
      "sha256": "sha256:abcdef...",
      "size": 1024
    },
    {
      "path": "fastp.yaml",
      "sha256": "sha256:123456...",
      "size": 256
    }
  ],
  "containers": [
    {
      "type": "docker",
      "image": "biocontainers/bwa:0.7.17"
    }
  ]
}
```

## See Also

- [oxo-flow run](run.md) — `--bundle` flag for executing bundles
- [oxo-flow pull](../commands/pull.md) — download bundles from remote sources
- [Environment system](../reference/environment-system.md) — supported environment backends
