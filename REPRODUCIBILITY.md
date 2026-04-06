# Reproducibility

Reproducibility is a core design principle of oxo-flow. This document describes
the mechanisms and methodology that ensure workflows produce identical results
across runs, machines, and time.

## Deterministic Execution Guarantees

oxo-flow provides the following guarantees for deterministic execution:

- **Fixed task ordering** — The DAG executor resolves dependencies via
  topological sort and executes tasks in a deterministic order when resource
  constraints are equal. Parallel tasks are dispatched in a stable order
  derived from the DAG structure.
- **Seed propagation** — When workflows involve stochastic tools, oxo-flow
  supports seed parameters that are propagated to each task, ensuring
  reproducible random behavior.
- **Isolated execution** — Each task runs in its own environment (container,
  conda env, or venv), preventing cross-task interference.

## Configuration Checksumming

oxo-flow computes checksums of all inputs that affect execution:

| Input | Checksum method |
| ----- | --------------- |
| `.oxoflow` workflow file | SHA-256 |
| Input data files | SHA-256 (content-based) |
| Environment specifications | SHA-256 of resolved spec |
| Software versions | Recorded in provenance log |
| Runtime parameters | SHA-256 of serialized config |

Before execution begins, oxo-flow computes a **workflow fingerprint** — a
composite hash of all input checksums. Two runs with the same fingerprint are
expected to produce identical outputs, given deterministic tools.

If any input changes between runs, the fingerprint changes and oxo-flow can
identify which tasks need to be re-executed (incremental re-runs).

## Container-Based Reproducibility

Containers are the strongest reproducibility mechanism available:

1. **Pinned base images** — Workflow authors specify exact container image
   digests (e.g., `sha256:abc123...`) rather than mutable tags.
2. **Immutable environments** — Docker and Singularity/Apptainer images capture
   the complete software stack, including OS libraries, tool versions, and
   configurations.
3. **Offline execution** — Pre-pulled container images allow workflows to run
   without network access, eliminating variability from upstream repositories.
4. **Packaging** — The `oxo-flow package` command bundles a workflow, its
   container images, and configuration into a self-contained archive that can
   be transferred and executed on any compatible system.

### Recommended Practices

- Always use image digests instead of tags for production workflows.
- Store container images in a private registry or archive for long-term
  reproducibility.
- Test containerized workflows in a clean environment before deployment.

## Version Pinning

oxo-flow enforces version pinning at multiple levels:

- **Tool versions** — Environment specs (conda YAML, `requirements.txt`,
  Dockerfiles) should pin exact versions of every dependency.
- **oxo-flow version** — The workflow fingerprint includes the oxo-flow version
  used to generate the execution plan.
- **Reference data** — Reference genomes, annotation databases, and other
  static resources should be versioned and checksummed.

### Lock Files

When using conda or pixi environments, oxo-flow generates and respects lock
files (`conda-lock.yml`, `pixi.lock`) to ensure the exact same package versions
are installed across environments.

## Provenance Tracking

Every workflow execution produces a **provenance record** containing:

- **Workflow definition** — A snapshot of the `.oxoflow` file as executed.
- **Input manifest** — File paths, sizes, and SHA-256 checksums of all inputs.
- **Output manifest** — File paths, sizes, and SHA-256 checksums of all outputs.
- **Environment details** — Container image digests, conda environment hashes,
  and resolved package lists.
- **Execution metadata** — Start time, end time, exit codes, resource usage
  (CPU, memory, wall time) per task.
- **System information** — Hostname, OS, architecture, and oxo-flow version.
- **DAG structure** — The complete dependency graph as executed.

Provenance records are stored as structured JSON alongside workflow outputs and
can be used to:

- Audit past executions for regulatory compliance
- Compare runs to identify sources of variability
- Reproduce exact execution conditions on a different system
- Generate compliance reports for clinical and regulated environments

## Validation

For clinical and regulated environments, oxo-flow's reproducibility mechanisms
support formal validation protocols. See
[VALIDATION_PROTOCOL.md](docs/VALIDATION_PROTOCOL.md) for IQ/OQ/PQ templates.

---

This project is licensed under the [Apache License 2.0](LICENSE).
