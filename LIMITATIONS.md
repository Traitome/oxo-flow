# Known Limitations

This document provides an honest account of the current limitations of
oxo-flow. We believe transparency helps users make informed decisions and
helps contributors identify areas for improvement.

## Design Scope

oxo-flow is a **general-purpose workflow engine** — it orchestrates external
tools but does not implement bioinformatics algorithms (alignment, variant
calling, etc.) natively. It is not tied to any specific omics domain;
pipelines for any data type can be built using the same framework.

The `.oxoflow` format uses TOML by design for its readability and
composability. It is not an established bioinformatics standard like CWL or
WDL, and no importers for those formats are currently available.

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
- **Memory scaling** — DAG metadata grows linearly with the number of samples
  and rules. For >10,000 samples, consider chunked execution strategies.

## Unsupported Features

- **No native CWL/WDL import** — There is no built-in importer for Common
  Workflow Language or Workflow Description Language files.
- **No native Nextflow/Snakemake import** — There is no automatic conversion
  from Nextflow or Snakemake workflow definitions.
- **No built-in cloud object storage streaming** — oxo-flow supports
  `s3://` and `gs://` URIs via feature flags (`s3-storage`, `gcs-storage`),
  but data is fully downloaded before execution rather than streamed.
  Streaming support is on the roadmap.
- **No GUI workflow editor** — The web interface provides monitoring and
  management but does not include a visual drag-and-drop workflow editor.
- **No Kubernetes operator** — Cloud-native orchestration via Kubernetes
  CRD/operator is not yet available.
- **No native distributed consensus** — oxo-flow assumes a shared filesystem
  for multi-node execution. True distributed execution without shared storage
  is not supported.

## Practical Constraints

- **Reference database management** — oxo-flow does not automatically download
  or update reference databases. Versions can be tracked via the
  `[[reference_db]]` configuration section.
- **Conda/Pixi environment resolution** — Environment creation depends on
  external package managers. Network issues or solver conflicts are outside
  oxo-flow's control.
- **Cluster backend specifics** — SLURM, PBS, SGE, and LSF backends rely on
  the scheduler being correctly configured on the host system. oxo-flow
  cannot diagnose cluster misconfiguration.
- **GPU scheduling** — GPU resource declarations are passed to the cluster
  scheduler. On the local executor, a warning is emitted when GPU specs are
  declared but GPU availability is not verified.

## Standards & Compliance

- **No GA4GH TES/WES support** — Task Execution Service and Workflow Execution
  Service APIs are not implemented.
- **No FHIR/HL7 integration** — Clinical data interchange standards are not
  natively supported. Report output is HTML/JSON.
- **Limited OpenAPI specification** — A basic OpenAPI 3.0 specification is
  available but may not cover all endpoints comprehensively.
- **No HIPAA/GDPR de-identification tools** — PHI handling and data
  de-identification must be managed by the user or external tools.
- **No native PDF export** — PDF generation requires `wkhtmltopdf` to be
  installed separately. See `oxo-flow report --help`.
- **Regulatory certification** — oxo-flow provides audit trails, checksums,
  and provenance tracking, but formal regulatory certification (FDA, CLIA,
  etc.) is the responsibility of the deploying organization.

## Known Issues

- **Wildcard expansion with deeply nested directories** — Patterns like
  `{sample}` that resolve to paths with many directory levels may be
  slower than flat structures.
- **Report generation memory usage** — Generating large HTML reports can
  consume significant memory. File checksums use streaming I/O (64KB buffer)
  to handle large files without loading them entirely into memory.
- **Hot-reload of `.oxoflow` files** — Changes to workflow files during
  execution are not detected. The `oxo-flow watch` command provides
  edit-time re-validation but does not automatically re-execute.

## Roadmap

Many of these limitations are actively being addressed. See
[ROADMAP.md](ROADMAP.md) for planned improvements and timelines.

---

If you encounter a limitation not listed here, please
[open an issue](https://github.com/Traitome/oxo-flow/issues).
