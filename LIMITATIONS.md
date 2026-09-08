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
- **No HIPAA/GDPR de-identification tools** — PHI handling and data
  de-identification must be managed by the user or external tools.
- **PDF export requires an external renderer** — `oxo-flow report -f pdf`
  shells out to `wkhtmltopdf`; `-f pdf-command` prints the equivalent
  command without running it, and `-f md` emits Markdown. HTML and JSON
  need no external tooling. See `oxo-flow report --help`.
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
  execution are not detected; the workflow must be re-validated and
  re-executed explicitly.
- **A tampered output is verified, not re-executed** — `oxo-flow provenance
  verify` reports an output file whose content no longer matches its recorded
  checksum, but a re-run skips rules already marked completed in the
  checkpoint (their outputs exist), so the tampered file is served as-is even
  with `--provenance`. Use `--rerun` to force re-execution.
- **Content hashing has a 64 MiB cap** — input manifests (and the freshness
  gate) content-hash files up to 64 MiB; larger files — typically BAM/CRAM
  intermediates — are tracked by size + mtime, so an in-place change that
  preserves both is not detected.
- **`sample_pattern` binds `{sample}` only** — `{read}`, `{replicate}`, or any
  other wildcard name in `sample_pattern` does not create per-read or
  per-replicate instances: every matching file collapses to the same
  `{sample}` value and expansion fails with `duplicate rule name`. For
  paired-end data, point the pattern at R1 and list R2 in each rule's `input`
  (the gallery examples' pattern); for genuine per-read/per-replicate
  fan-out, declare the domain with `[[pairs]]`/`[[sample_groups]]` or bind
  the extra wildcards with `input_groups`.
- **Undeclared intermediates are invisible to provenance** — only declared
  `input`/`output` paths are recorded. A file a rule creates but does not
  declare — `samtools index`'s `.bai`, `tabix`'s `.tbi`, GATK's
  `.recal_data.table` (gallery 07 produces one without declaring it) — is
  not tracked, and deleting it does **not** re-run its producer: the
  producer's declared outputs still exist, so the next run skips the rule and
  the file stays missing until you force it with `--rerun`. Declare every
  artifact a downstream step depends on.
- **A duplicate sample id across groups silently shares one output path** —
  the same `{sample}` value in two `[[sample_groups]]` (or in a group and a
  `sample_pattern` discovery) expands to two rule instances that write the
  identical output path. `validate` exits 0 and `dry-run` lists both
  instances without complaint; whichever finishes last overwrites the other.
  Keep sample ids unique across groups.
- **`transform` `n = N` chunks the value space, not the data** — it generates
  the labels `"0"`…`"N-1"` and repeats the rule once per label, and every map
  instance receives the rule's whole input. The engine never slices a file:
  the `map` command must use the split value to select its own slice (e.g.
  `-L {chr}`), or the rule's `input` must be the split value itself
  (`input = ["{chr}"]`). See the `transform` operator section of the
  [Workflow Format reference](docs/guide/src/reference/workflow-format.md).

## Roadmap

Many of these limitations are actively being addressed. See
[ROADMAP.md](ROADMAP.md) for planned improvements and timelines.

---

If you encounter a limitation not listed here, please
[open an issue](https://github.com/Traitome/oxo-flow/issues).
