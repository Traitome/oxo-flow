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
(`workflow_git_sha`). It also records a **content digest** of the workflow
file as read at run start (`workflow_content_hash`, a `sha256:<hex>` of the
top-level file) and a marker for whether the workflow file had
**uncommitted changes** (`workflow_git_dirty`, from
`git status --porcelain` scoped to the workflow file). Every output file,
benchmark, and report snapshot is thereby auditable to the exact workflow
version that produced it.

For a workflow built out of `[[include]]` modules, the content hash covers
only the top-level file: the complete identity is that file plus the
separately recorded include resolutions (`include_resolutions`, the
per-include checkout SHAs), so a change in a pinned module's content is
still captured there even though `workflow_content_hash` itself does not
move.

- Best-effort by design: running outside a git repository omits the SHA
  and dirty fields and never fails the run — plain directories remain fully
  supported for experimentation, and become version-audited the moment the
  workflow is committed.
- The HEAD SHA alone cannot distinguish two runs of the **same**
  uncommitted workflow file. When `workflow_git_dirty` is `true`, the
  recorded `workflow_content_hash` is the authoritative record of *which
  bytes* executed: it changes with every edit, so two runs of a dirty file
  never collide on provenance even though `workflow_git_sha` is identical.
- Neither the HEAD SHA nor the raw content digest can separate two runs of
  the same file invoked with **different `--arg KEY=VALUE` overrides** —
  both digests are override-blind by construction. The checkpoint
  therefore also records `workflow_resolved_hash`, a `sha256:<hex>` over
  the canonical JSON of the raw content digest combined with the engine's
  resolved config snapshot (the same order-independent map that drives
  config-change invalidation: built after CLI overrides, profile merge,
  and wildcard expansion; engine-injected keys excluded; sensitive values
  stored as digests). Two override-driven runs of one file always differ
  on this fingerprint; it is an identity label for run-to-run comparison
  — re-deriving it at verify time would need the full invocation context —
  not a verified claim.
- `oxo-flow provenance verify` now **re-checks the raw content digest**: it
  re-hashes the workflow file named in the checkpoint and compares it with
  the recorded `workflow_content_hash`. A workflow edited after the run is
  a verification finding (exit 1; JSON
  `workflow_content_check.status = "changed"`); a file missing or
  unreadable at its recorded path (e.g. a relocated checkpoint) is
  reported as `unverifiable` without failing the audit. Verify prints the
  recorded SHA, content hash, and dirty marker alongside the workflow path
  (see [provenance](../commands/provenance.md)); the JSON fields
  (`workflow_git_sha`, `workflow_content_hash`, `workflow_resolved_hash`,
  `workflow_content_check`, `workflow_git_dirty`) are there for
  programmatic use.
- The version travels with every run artifact, closing the audit chain:
  the **run log** (`.oxo-flow/logs/oxo-flow.log`, rotated per run, see
  [run](../commands/run.md#run-logs)) carries the SHA, content hash, and
  dirty marker in its header, and **report snapshots** embed
  `workflow_git_sha` in their JSON and show it in the HTML provenance
  section.
- Practical guidance: **commit before you run**; tag milestones
  (`git tag v0.32.0`) so the bare SHA has a human-readable alias in your
  records.

## Pillar 2 — Modules: includes pin an exact commit

A workflow composed from remote modules must stay reproducible even as
upstream evolves. `[[include]]` therefore accepts `repo` + `ref` (a git
repository URL plus a tag, branch, or commit) alongside `path`. Pinned
includes are cloned once into
`~/.cache/oxo-flow/modules/<repo>@<ref>@<hash>` (the `<hash>` suffix is
an FNV-1a of the repo+ref pair — flattened names alone are ambiguous,
e.g. `https://a/b` and `https://a_b`) and reused — the module is frozen
at that ref until the workflow author moves it deliberately. See
[Workflow Format](workflow-format.md#include-modular-workflow-composition)
for the include syntax and interface contracts.

Branch and tag pins re-fetch on every activation, so the workflow file
alone cannot prove which module content a past run used. Every repo-include
therefore also records **the checkout's resolved HEAD SHA** in the
checkpoint (`include_resolutions`) at parse time — one entry
per include, shown per include in the run report's Provenance section.
A run/resume pair (or any two runs) can be compared by these SHAs to prove
the module content did not drift between them.

## Pillar 3 — Ecosystem: the catalog is versioned

Published workflows are referenced by repo + version, never by an
unpinned "latest". `oxo-flow info --json` derives a workflow's git
identity directly — `git_sha` (HEAD commit), `git_remote` (origin URL),
and `git_describe` (nearest tag) — with the keys omitted outside a git
repository. Catalog generation consumes these fields to
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
