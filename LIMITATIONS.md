# Known Limitations

This document provides an honest account of the current limitations of
oxo-flow. We believe transparency helps users make informed decisions and
helps contributors identify areas for improvement.

## Current Scale Limits

- **Maximum concurrent tasks** — The local executor is bounded by system
  resources (CPU cores, memory). For very large workflows (>10,000 tasks),
  cluster or cloud backends are recommended.
- **DAG size** — Workflows with extremely large DAGs (>100,000 nodes) may
  experience increased planning time during the topological sort and dependency
  resolution phase.
- **File handle limits** — Workflows that produce a very large number of output
  files simultaneously may hit OS-level file descriptor limits. Increase
  `ulimit -n` as needed.

## Unsupported Features

- **No native CWL/WDL import** — oxo-flow uses its own `.oxoflow` TOML-based
  format. There is no built-in importer for Common Workflow Language (CWL) or
  Workflow Description Language (WDL) files. This is planned for a future
  release.
- **No native Nextflow/Snakemake import** — Similarly, there is no automatic
  conversion from Nextflow or Snakemake workflow definitions.
- **No built-in cloud object storage streaming** — While cloud execution
  backends are supported, oxo-flow does not natively stream data from S3, GCS,
  or Azure Blob Storage. Users must stage data locally or use FUSE-based
  mounts.
- **No GUI workflow editor** — The web interface provides monitoring and
  management but does not include a visual drag-and-drop workflow editor.
- **Limited Windows support** — oxo-flow is developed and tested primarily on
  Linux. macOS is supported. Windows support is experimental and limited to WSL2.

## Platform Limitations

- **Container runtime required for full reproducibility** — While oxo-flow can
  run without containers, full reproducibility guarantees require Docker or
  Singularity/Apptainer.
- **Conda/Pixi environment resolution** — Environment creation depends on
  external package managers. Network issues or solver conflicts are outside
  oxo-flow's control.
- **Cluster backend specifics** — SLURM, PBS, and SGE backends rely on the
  scheduler being correctly configured on the host system. oxo-flow cannot
  diagnose cluster misconfiguration.
- **GPU scheduling** — GPU resource declarations are passed to the cluster
  scheduler but oxo-flow does not verify GPU availability on the local
  executor.

## Known Issues

- **Wildcard expansion with deeply nested directories** — Wildcard patterns
  like `{sample}` that resolve to paths with many directory levels may be
  slower than flat structures.
- **Report generation memory usage** — Generating large HTML reports with
  embedded images can consume significant memory. Consider using JSON output
  for very large datasets.
- **Hot-reload of `.oxoflow` files** — Changes to workflow files during
  execution are not detected. The workflow must be re-run from the beginning.

## Roadmap

Many of these limitations are actively being addressed. See
[ROADMAP.md](ROADMAP.md) for planned improvements and timelines.

---

If you encounter a limitation not listed here, please
[open an issue](https://github.com/oxo-flow/oxo-flow/issues).
