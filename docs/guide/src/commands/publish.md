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
| `--output` | `-o` | Output path for the bundle archive (default: `<name>-bundle.<ext>`) |
| `--with-lockfiles` | | Generate `conda-lock` lockfiles for reproducible environments |
| `--format` | | Archive format: `tar.zst` (default, better compression) or `tar.gz` (universal compatibility) |
| `--verbose` | `-v` | Enable verbose (debug-level) logging |
| `--quiet` | | Suppress non-essential output (errors only) |
| `--no-color` | | Disable colored output |
| `--json` | | Output machine-readable JSON to stdout |

## Examples

```bash
# Publish a workflow (zstd compression, default)
oxo-flow publish my_pipeline.oxoflow
# → my_pipeline-bundle.tar.zst

# Publish with gzip compression (universal compatibility)
oxo-flow publish my_pipeline.oxoflow --format tar.gz
# → my_pipeline-bundle.tar.gz

# Publish with custom output path
oxo-flow publish my_pipeline.oxoflow -o /path/to/bundle.tar.zst

# Publish with deterministic lockfiles
oxo-flow publish my_pipeline.oxoflow --with-lockfiles

# Run a published bundle (extract → verify → confirm → execute)
oxo-flow run --bundle my_pipeline-bundle.tar.zst -j 16

# Run with --yes to skip confirmation (CI/scripts)
oxo-flow run --bundle my_pipeline-bundle.tar.zst -j 16 --yes

# Pull a remote bundle and run it
oxo-flow pull gh:user/repo@v0.12.0
oxo-flow run --bundle repo-bundle.tar.zst --yes
```

## Manifest Format

The `manifest.json` inside each bundle:

```json
{
  "format": "oxoflow-bundle-v1",
  "workflow": "my_pipeline.oxoflow",
  "oxo_flow_version": "0.10.2",
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
  ],
  "resources": {
    "rules": [
      {
        "rule": "bwa_align",
        "threads": 16
      }
    ],
    "recommendations": {
      "min_threads": 16
    }
  },
  "signatures": []
}
```

`signatures` is reserved for future bundle signing and is always empty today.
It is present so that adding signatures later is an additive change rather than
a manifest format bump.

## Reproducibility Caveats

A bundle captures the workflow, its environment specifications, and checksums for
every file. That makes a bundle *verifiable* — you can prove you received exactly
what was published. It does not make execution *identical* everywhere, and it is
worth being explicit about the limits:

- **Environment specs are resolved on the consumer's machine.** `publish` bundles
  `environment.yaml` / `pixi.toml` spec files, not solved environments. A solve
  run months later, or against different channels, can pick different package
  versions. Use `--with-lockfiles` to pin the resolution.
- **Lockfiles still are not binaries.** Even an exact package set can behave
  differently across glibc versions, CPU features, or filesystem layouts. Tools
  built against a newer glibc will not run on an older host.
- **Container images are referenced, not vendored.** The manifest records image
  type and tag. The image is pulled at run time, so a mutable tag can resolve to
  different content later. Prefer digest-pinned references where it matters.
- **Containers do not normalise resources.** An image that runs fine on the
  publisher's machine can be OOM-killed on a smaller host, and thread counts vary
  with the available CPUs.

None of these are specific to oxo-flow — they apply to Snakemake and Nextflow
bundles equally. The goal is honest reproducibility, not a guarantee we cannot
make.

## See Also

- [oxo-flow run](run.md) — `--bundle` flag for executing bundles
- [oxo-flow pull](../commands/pull.md) — download bundles from remote sources
- [Environment system](../reference/environment-system.md) — supported environment backends
