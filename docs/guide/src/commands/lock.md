# `oxo-flow lock`

Generate or verify a workflow-level lockfile that pins what a run is
allowed to consume.

```bash
# Write (or refresh) oxo-flow.lock beside the workflow
oxo-flow lock my_pipeline.oxoflow

# CI gate: verify without mutating anything
oxo-flow lock --check my_pipeline.oxoflow
```

| Flag | Description |
|---|---|
| `-o, --output <PATH>` | Lock file path for writing/checking. Default: `oxo-flow.lock` beside the workflow. `run --locked` always reads `oxo-flow.lock` beside the workflow — see [Where the lock must exist](#where-the-lock-must-exist) |
| `--check` | Verify the existing lock instead of writing; never mutates the file |
| `--profile <NAME>` | Pin the config a `run --profile NAME` would see (same merge semantics as `run`) |
| `--arg KEY=VALUE` | Pin a config override the run will use (same semantics as `run --arg`); repeatable |
| `--json` | Emit one machine-readable report on stdout (the human report stays on stderr) |

`lock` accepts the same workflow resolution as `run`: an explicit path, or
auto-discovery (`main.oxoflow`, else the first `*.oxoflow`) in the current
directory. The lock path may never be the workflow file itself (a usage
error), so a typo cannot overwrite the workflow with lock JSON.

## Exit codes

| Code | Mode | Meaning |
|---|---|---|
| `0` | both | Lock written / lock matches and is valid |
| `2` | `--check` | Drift, a missing lock, or an invalid/unreadable lock document |
| `4` | both | The workflow itself could not be resolved, loaded, or expanded (usage/parse error) |
| `1` | write | Any other failure (e.g. the lock file could not be written) |

Workflow-discovery failures and explicit bad paths behave identically
(`4`), in both modes.

## What the lock pins

The document is canonical, key-sorted JSON with the format tag
`oxo-flow.lock/v1`:

- the workflow file's content `sha256`, its override-aware
  `workflow_resolved_hash` (the same identity a run records in its
  checkpoint), and the canonical metadata-contract digest;
- the resolved `[config]` snapshot (engine-injected keys excluded). Stored
  forms: keys declared `sensitive = true` are always SHA-256 digests; a key
  that is not declared sensitive but whose name looks like a credential
  (a `KEY`/`TOKEN`/`SECRET`/`PASSWORD`/`PASSWD`/`CREDENTIAL(S)` segment —
  including camel-case splits — or the bare `apikey`/`pwd` spellings, e.g.
  `API_TOKEN`, `apiKey`, `apikey`, `db_password`) is **also** stored as a
  SHA-256 digest; **every other config value, including an undeclared
  `--arg KEY=VALUE`, is stored verbatim**. `lock` warns before committing
  when an undeclared override is pinned in plaintext. Never pin a secret
  through an ordinary config key — declare it `sensitive = true` (or use a
  credential-shaped name);
- the include graph — workflow-root-relative path plus content digest for
  local includes, `repo@ref` plus the resolved HEAD SHA and content digest
  for pinned git modules;
- per-rule environment identity from the engine's environment-lock
  resolution: the resolved container digest, the key-ordered env lock hash,
  or an explicit sentinel (see below), plus the declared software the
  workflow writes down (image tags, bare env names, env-file paths and
  content digests, module lists);
- `lock_digest` — SHA-256 over the canonical document excluding the digest
  field itself, so an edit to a pinned field — or an injected unknown field
  (unknown fields are rejected as `LOCK-007`) — is detectable. The
  generating engine version is inside the digest.

Paths are recorded workflow-relative with `/` separators. The same logical
workflow in a different working directory generates byte-identical lock
bytes, and regenerating an unchanged workflow with the **same engine
version** is byte-identical — `generated_by.engine_version` is part of the
digest, so a different release produces different lock bytes (regenerate
deliberately, then review the diff).

## What is *not* pinned

A lock is **evidence of declared inputs, not a guarantee of bitwise
reproducibility**:

- It never pulls an image, contacts a registry, or solves packages. A
  container digest is whatever the local runtime could report offline; an
  absent image records the `unresolved` sentinel rather than a fabricated
  digest. Pull the images you intend to pin first, then generate the lock.
- It does not prove a tool behaves identically on another host, and it does
  not solve transitive dependencies beyond what the workflow's own env
  files/lockfiles declare.
- `lock_digest` proves the document's fields were the ones hashed together.
  It is not a signature and not an external notarization; an actor who
  rewrites the whole document (including the digest) produces a
  self-consistent but unauthentic lock. Commit the lock to version control
  so a previously reviewed revision is what exposes a rewrite.
- Dynamic inputs are not pinned: runtime checkpoint re-entry instances
  (`checkpoint_manifest` values discovered during execution) and the
  contents of a `--samples @file` samplesheet are not in the lock (declared
  metadata is). Sample discovery at generation time expands whatever data
  is on disk then, so the rule map reflects that moment; verification is
  permissive in the subset direction (lock-only rules are not findings).
- A lock-enforced `resume` overwrites the predecessor checkpoint's
  `lock_digest` with the current one without comparing them first; reuse of
  the predecessor's outputs is still governed by the checkpoint's recorded
  freshness and content hashes, but the digest does not prove the earlier
  run was lock-verified.
- An `-o custom.lock` file is a valid artifact for `lock --check -o` and
  external tooling, but **enforcement does not read it** (see below).

## Enforcing the lock

Two switches enforce verification; without either, no lock is read and
behavior is unchanged:

```bash
# Explicit per-invocation enforcement
oxo-flow run --locked my_pipeline.oxoflow
```

```toml
# Persistent enforcement — inherited by web-spawned, queued, and cluster runs
[engine]
locked = true
```

`run --locked` wins over the config key. Verification happens **before any
rule executes and before the run creates outputs**: a missing or stale lock
fails closed.

Because `[engine] locked = true` is part of the workflow bytes the lock
pins, add the key first and then generate (or regenerate) the lock.

Resume inherits the config key, so a locked workflow cannot resume across
drift either. The repair paths rewrite the workflow and then re-verify: an
AI or reviewed repair under an **enforced** lock is refused at the
verification rerun (`LOCK-002`) in both the `--locked` and `[engine]`
forms — the parent run exits non-zero with `REPAIR_RERUN_FAILED`, and no
patched rule executes. Review the fix, apply it without enforcement (or
apply it and re-lock), then regenerate the lock and run with enforcement.

### Where the lock must exist

Enforcement reads exactly `<workflow dir>/oxo-flow.lock`; `run --locked`
has no lock-path override and does not read an `-o` custom path. This
matters for runs that execute a **copy** of the workflow rather than the
source tree:

- **Web / queued runs** (`POST /api/runs`) stage `workflow.oxoflow` plus
  the client's `include_files` into a per-run workdir. A workflow with
  `[engine] locked = true` must also stage `oxo-flow.lock` — send it as an
  `include_files` entry with the key `oxo-flow.lock`. Without it the run
  fails closed with `LOCK-001` (a closed failure, not a bypass).
- **Remote cluster runs** ship the run workdir; the same `include_files`
  entry travels with it. Without the lock the remote run fails `LOCK-001`.
- **`publish` bundles** do not include `oxo-flow.lock` (see
  [`publish`](publish.md)); regenerate the lock inside the extraction if a
  bundled workflow enforces.

The `run`/`resume` checkpoint records the verified `lock_digest`, and
`provenance verify` / `provenance attest` report it. The digest is folded
into `workflow_resolved_hash` only when a lock is in effect, so runs
without a lock keep their pre-lock identity and bytes.

## Drift diagnostics

| Code | Meaning |
|---|---|
| `LOCK-001` | Lock missing under enforcement |
| `LOCK-002` | Workflow content hash, resolved hash, or metadata digest changed |
| `LOCK-003` | A `[config]` value was added, removed, or changed |
| `LOCK-004` | A rule's environment identity or declared software changed, or a current rule is not pinned |
| `LOCK-005` | An include was changed, added, removed, or is unresolvable (field-level findings name `source`/`repo`/`ref`/`resolved_sha`/`content_sha256`) |
| `LOCK-006` | An environment identity is unresolved — unknown never compares equal |
| `LOCK-007` | The lock document is malformed, unsupported, or its own digest does not match |

Each finding names the rule/field plus the locked (`expected`) and current
(`actual`) value.

### Unknown is never a match

- `unresolved` — real lock content exists but could not be resolved
  offline. It never compares equal, **including to itself**, so a lock
  generated without local image access reports `LOCK-006` until the digest
  resolves (an unresolved **include** digest reports `LOCK-005` the same
  way).
- `unavailable` — the backend supplies no lockable content at all
  (`system`). It compares equal only to `unavailable`.

Verification covers the rules the current invocation declares. Partial
execution (`--samples`, `--target`, `--module`, `when` pruning) is not
drift; a genuinely removed rule is caught by the workflow content hash
(`LOCK-002`).

## CI example

```bash
oxo-flow lock --check my_pipeline.oxoflow
```

Machine-readable form:

```bash
oxo-flow lock --check --json my_pipeline.oxoflow
```

```json
{
  "command": "lock",
  "mode": "check",
  "workflow": "my_pipeline.oxoflow",
  "lock": "oxo-flow.lock",
  "ok": false,
  "lock_digest": "sha256:…",
  "findings": [
    {
      "code": "LOCK-002",
      "subject": "workflow.content_sha256",
      "expected": "sha256:…",
      "actual": "sha256:…",
      "message": "the workflow file's bytes changed since the lock was generated"
    }
  ]
}
```

## Related

- [`run`](run.md) — `--locked` enforcement and checkpoint recording
- [`provenance`](provenance.md) — the recorded lock digest in verify and
  attestations
- [`publish`](publish.md) — bundle lockfiles are a different mechanism; the
  workflow lock is not bundled automatically
- [`environment-system`](../reference/environment-system.md) — how the
  per-rule environment identity is derived
- [`workflow-format`](../reference/workflow-format.md#engine--runtime-disk-pressure-guard)
  — `[engine] locked`
