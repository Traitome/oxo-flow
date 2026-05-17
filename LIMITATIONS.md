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
- **Memory scaling** — DAG metadata grows linearly with the number of samples
  and rules. For >10,000 samples, consider chunked execution strategies.

## Unsupported Features

- **No native CWL/WDL import** — oxo-flow uses its own `.oxoflow` TOML-based
  format. There is no built-in importer for Common Workflow Language (CWL) or
  Workflow Description Language (WDL) files. This is planned for a future
  release.
- **No named inputs/outputs** — Currently, input and output files are accessed by index
  (e.g., `{input[0]}`). Named inputs (e.g., `{input.reads}`) are not yet supported.
- **No native Nextflow/Snakemake import** — Similarly, there is no automatic
  conversion from Nextflow or Snakemake workflow definitions.
- **No built-in cloud object storage streaming** — While cloud execution
  backends are supported, oxo-flow does not natively stream data from S3, GCS,
  or Azure Blob Storage. Users must stage data locally or use FUSE-based
  mounts.
- **No GUI workflow editor** — The web interface provides monitoring and
  management but does not include a visual drag-and-drop workflow editor.
- **No Kubernetes operator** — Cloud-native orchestration via Kubernetes CRD/operator
  is not yet available. Users should use cluster backends (SLURM/PBS) or local execution.
- **No native distributed consensus** — oxo-flow assumes a shared filesystem for
  multi-node execution. True distributed execution without shared storage is not supported.
- **No serverless execution** — AWS Lambda / Google Cloud Functions backends are
  not implemented.

## Domain-Specific Limitations

- **Not a domain-specific pipeline** — oxo-flow is a general-purpose workflow engine.
  It does not include pre-built analysis logic for specific omics domains
  (microbiome, proteomics, metabolomics, spatial transcriptomics). The Venus pipeline
  demonstrates clinical genomics workflows; other domains can build pipelines using
  the same framework.
- **No built-in bioinformatics algorithms** — oxo-flow orchestrates external tools;
  it does not implement alignment, variant calling, or statistical analysis natively.
- **Reference database management** — While reference database versions can be tracked
  via the `[[reference_db]]` configuration section, oxo-flow does not automatically
  download or update databases.

## Standards Compliance

- **No GA4GH TES/WES support** — Task Execution Service and Workflow Execution Service
  APIs from GA4GH are not implemented.
- **No FHIR/HL7 integration** — Clinical data interchange standards are not natively
  supported. Report output is HTML/JSON which can be post-processed into FHIR resources.
- **No OpenAPI specification** — The REST API does not yet publish an OpenAPI/Swagger spec,
  which currently requires developers to refer directly to the Rust source for API structures.
- **TOML is not an industry standard** — The `.oxoflow` format is purpose-built for
  readability but is not an established bioinformatics standard like CWL or WDL.

## Clinical Compliance

- **No FDA 21 CFR Part 11 certification** — While oxo-flow provides audit trails,
  checksums, and provenance tracking, it has not undergone formal FDA validation.
- **No HIPAA/GDPR de-identification tools** — PHI handling and data de-identification
  must be managed by the user or external tools.
- **No PDF Export** — Clinical reporting output is currently limited to HTML and JSON.
  Exporting directly to PDF (often required for electronic medical records) is not yet supported.
- **"Clinical-grade" refers to design intent** — The engineering practices (audit trails,
  reproducibility, provenance) are designed with clinical workflows in mind, but formal
  regulatory certification is the responsibility of the deploying organization.

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
- **GPU scheduling** — GPU resource declarations (including model, memory, and compute
  capability via `gpu_spec`) are passed to the cluster scheduler but oxo-flow does not
  verify GPU availability on the local executor.

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
[open an issue](https://github.com/Traitome/oxo-flow/issues).
