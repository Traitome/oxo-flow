# Reproducibility

Reproducibility is a core design principle of oxo-flow. This document describes
the mechanisms and methodology that ensure workflows produce identical results
across runs, machines, and time.

## Deterministic Execution Guarantees

oxo-flow provides the following guarantees for deterministic execution:

- **Fixed task ordering** — The DAG executor resolves dependencies via
  topological sort and executes tasks in a deterministic order. The DAG
  engine computes parallel execution groups based on dependency depth.
- **Isolated execution** — Each task runs in its own environment (container,
  conda env, or venv), preventing cross-task interference.

## Configuration Checksumming

oxo-flow computes checksums of inputs that affect execution:

| Input | Recorded as |
| ----- | ----------- |
| `.oxoflow` workflow file | SHA-256 (streamed) |
| Input data files | SHA-256 (content-based) for files up to 64 MiB; larger files are tracked by size + mtime |
| Software versions | Not recorded at runtime — the workflow's static software declarations are exported by `oxo-flow report --versions-yml` for CI diffing |
| Runtime parameters | Per-key config snapshot in the checkpoint (sensitive keys store a SHA-256 digest), compared on every run |

Re-running a workflow with the same config and outputs already present will
skip already-completed rules via the checkpoint system.

## Container-Based Reproducibility

Containers are the strongest reproducibility mechanism available:

1. **Pinned base images** — Workflow authors specify exact container image
   digests (e.g., `sha256:abc123...`) rather than mutable tags.
2. **Immutable environments** — Docker and Singularity/Apptainer images capture
   the complete software stack, including OS libraries, tool versions, and
   configurations.
3. **Offline execution** — Pre-pulled container images allow workflows to run
   without network access, eliminating variability from upstream repositories.
4. **Packaging** — The `oxo-flow export` command generates a Dockerfile or
   Singularity definition file that captures the workflow, environment setup,
   and execution entrypoint. The generated files can be built into container
   images with standard tools (`docker build`, `singularity build`).

### Recommended Practices

- Prefer container-based environments (Docker/Singularity) for maximum
  reproducibility.
- Use image digests instead of tags for production workflows.
- Store container images in a private registry or archive for long-term
  reproducibility.
- Test containerized workflows in a clean environment before deployment.

## Version Pinning

oxo-flow encourages version pinning at multiple levels:

- **Tool versions** — Environment specs (conda YAML, `requirements.txt`,
  Dockerfiles) should pin exact versions of every dependency.
- **oxo-flow version** — The provenance record includes the oxo-flow version
  used to execute the workflow.
- **Reference data** — Reference genomes, annotation databases, and other
  static resources should be versioned and checksummed.

### Lock Files

When using conda or pixi environments, oxo-flow's lint command will advise
generating lock files (`conda-lock.yml`, `pixi.lock`) with external tools
such as `conda-lock` or `pixi lock` to ensure reproducible installs.

## Provenance Tracking

oxo-flow captures execution metadata through checkpoint and provenance
mechanisms:

- **Checkpoint state** — Persistent JSON file (`.oxo-flow/checkpoint.json`)
  tracking completed rules, failed rules, benchmarks, and execution metrics
  (wall time per task).
- **Input/output checksums** — The `oxo-flow provenance verify` command
  compares stored SHA-256 checksums of outputs against current files,
  supporting output integrity verification.
- **Execution metadata** — Per-rule benchmark records capture wall time,
  and checkpoints track rule completion status and exit codes.
- **System information** — The oxo-flow version and the workflow file's
  checksum are recorded in the report's provenance section; the hostname is
  not recorded.

The checkpoint (`CheckpointState`) captures output checksums (`checksums`,
written under `--provenance`), the per-key config snapshot, per-rule
structural fingerprints, and per-rule input manifests, alongside benchmarks
and rule completion status. Run workflows with `--provenance` to record
output checksums for later `oxo-flow provenance verify`.

Provenance records are stored as structured JSON alongside workflow outputs and
can be used to:

- Audit past executions for regulatory compliance
- Compare runs to identify sources of variability
- Verify output file integrity against stored checksums
- Generate compliance reports for clinical and regulated environments

## Validation

For clinical and regulated environments, oxo-flow's reproducibility mechanisms
support formal validation protocols. See
[VALIDATION_PROTOCOL.md](docs/VALIDATION_PROTOCOL.md) for IQ/OQ/PQ templates.
