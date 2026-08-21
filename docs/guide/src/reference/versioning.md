# Workflow Versioning

oxo-flow's versioning model is **"the git ref is the version"**: a workflow
is a git repository, and every workflow version is an immutable git commit,
identified by its SHA or by a tag that points to one. The model threads
through three layers — the engine, module composition, and the workflow
ecosystem — so that versioning is not a naming convention but an enforced
property of how oxo-flow records, composes, and publishes workflows.

## Pillar 1 — Provenance: results know their workflow version

At run start the engine resolves the **HEAD commit SHA** of the git
repository containing the workflow and stores it in the checkpoint
(`workflow_git_sha`). Every output file, benchmark, and report snapshot is
thereby auditable to the exact workflow version that produced it.

- Best-effort by design: running outside a git repository omits the field
  and never fails the run — plain directories remain fully supported for
  experimentation, and become version-audited the moment the workflow is
  committed.
- `oxo-flow provenance verify` prints the recorded SHA alongside the
  workflow path (see [provenance](../commands/provenance.md)); the JSON
  field is there for programmatic use.
- The version travels with every run artifact, closing the audit chain:
  the **run log** (`.oxo-flow/logs/oxo-flow.log`, rotated per run, see
  [run](../commands/run.md#run-logs)) carries the SHA in its header, and
  **report snapshots** embed `workflow_git_sha` in their JSON and show it
  in the HTML provenance section.
- Practical guidance: **commit before you run**; tag milestones
  (`git tag v1.2.0`) so the bare SHA has a human-readable alias in your
  records.

## Pillar 2 — Modules: includes pin an exact commit

A workflow composed from remote modules must stay reproducible even as
upstream evolves. `[[include]]` therefore accepts `repo` + `ref` (a git
repository URL plus a tag, branch, or commit) alongside `path`. Pinned
includes are cloned once into `~/.cache/oxo-flow/modules/<repo>@<ref>`
and reused — the module is frozen at that ref until the workflow author
moves it deliberately. See
[Workflow Format](workflow-format.md#include--modular-workflow-composition)
for the include syntax and interface contracts.

## Pillar 3 — Ecosystem: the catalog is versioned

Published workflows are referenced by repo + version, never by an
unpinned "latest". `oxo-flow info --json` derives a workflow's git
identity directly — `git_sha` (HEAD commit), `git_remote` (origin URL),
and `git_describe` (nearest tag) — with the keys omitted outside a git
repository (issue #124). Catalog generation consumes these fields to
record the repository and version of each published workflow, so a
reference to a catalog entry resolves to a specific, reproducible
artifact.

## Comparison with other systems

| System | Versioning model |
|--------|-----------------|
| **oxo-flow** | The git ref is the single source of truth: provenance SHAs in checkpoints, pinned module includes, versioned catalog entries |
| **Nextflow / nf-core** | DSL2 modules released with per-module git tags; pipelines pin module versions to keep runs reproducible |
| **Snakemake** | Workflows addressed by git URL + branch/tag (snakedeploy); catalogs pin explicit versions |

## See Also

- [Provenance](../commands/provenance.md) — verify results against their recorded version
- [run](../commands/run.md) — checkpoint contents, including `workflow_git_sha`
- [Workflow Format](workflow-format.md) — `[[include]]` composition with `repo`/`ref` pinning
