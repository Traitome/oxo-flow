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
| `-o, --output <PATH>` | Lock file path. Default: `oxo-flow.lock` beside the workflow |
| `--check` | Verify the existing lock instead of writing; never mutates the file |
| `--profile <NAME>` | Pin the config a `run --profile NAME` would see (same merge semantics as `run`) |
| `--arg KEY=VALUE` | Pin a config override the run will use (same semantics as `run --arg`); repeatable |
| `--json` | Emit one machine-readable report on stdout (the human report stays on stderr) |

`lock` accepts the same workflow resolution as `run`: an explicit path, or
auto-discovery (`main.oxoflow`, else the first `*.oxoflow`) in the current
directory.

## Exit codes (`--check`)

| Code | Meaning |
|---|---|
| `0` | Lock matches the workflow and is valid |
| `2` | Drift, a missing lock, or an invalid/unreadable lock document |
| `4` | The workflow itself could not be loaded or expanded (usage/parse error) |

Write mode exits `0` on success and reports I/O failures as errors.

## What the lock pins

The document is canonical, key-sorted JSON with the format tag
`oxo-flow.lock/v1`:

- the workflow file's content `sha256`, its override-aware
  `workflow_resolved_hash` (the same identity a run records in its
  checkpoint), and the canonical metadata-contract digest;
- the resolved `[config]` snapshot — the same map the checkpoint stores,
  with engine-injected keys excluded and `sensitive = true` values stored
  as SHA-256 digests (a secret never lands in the lock in plaintext);
- the include graph — workflow-root-relative path plus content digest for
  local includes, `repo@ref` plus the resolved HEAD SHA and content digest
  for pinned git modules;
- per-rule environment identity from the engine's environment-lock
  resolution: the resolved container digest, the key-ordered env lock hash,
  or an explicit sentinel (see below), plus the declared software the
  workflow writes down (image tags, bare env names, env-file paths and
  content digests, module lists);
- `lock_digest` — SHA-256 over the canonical document excluding the digest
  field itself, so a hand edit or corruption is detectable.

Paths are recorded workflow-relative with `/` separators. The same logical
workflow in a different working directory generates byte-identical lock
bytes, and regenerating an unchanged workflow is byte-identical.

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

## Enforcing the lock

Two switches enforce verification; without either, no lock is read and
behavior is unchanged:

```bash
# Explicit per-invocation enforcement
oxo-flow run --locked my_pipeline.oxoflow
```

```toml
# Persistent enforcement — also covers web-spawned and queued runs
[engine]
locked = true
```

`run --locked` wins over the config key. Verification happens **before any
rule executes and before the run creates outputs**: a missing or stale lock
fails closed.

Because `[engine] locked = true` is part of the workflow bytes the lock
pins, add the key first and then generate (or regenerate) the lock.

Resume and the web server's spawned runs inherit the config key, so a
locked workflow cannot resume or be re-run across drift either. This also
means an AI/reviewed repair patch (which rewrites the workflow) is refused
under an enforced lock — review the change and regenerate the lock.

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
| `LOCK-005` | An include was changed, added, removed, or is unresolvable |
| `LOCK-006` | An environment identity is unresolved — unknown never compares equal |
| `LOCK-007` | The lock document is malformed, unsupported, or its own digest does not match |

Each finding names the rule/field plus the locked (`expected`) and current
(`actual`) value.

### Unknown is never a match

- `unresolved` — real lock content exists but could not be resolved
  offline. It never compares equal, **including to itself**, so a lock
  generated without local image access reports `LOCK-006` until the digest
  resolves.
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
