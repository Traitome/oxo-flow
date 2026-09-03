# Cloud Storage

oxo-flow supports reading and writing workflow inputs and outputs from
cloud object storage, transparently resolving `s3://` and `gs://` URIs
through its pluggable storage backend system.

## Overview

Workflows can reference remote files using standard URI schemes:

```toml
[[rules]]
name = "fetch_data"
input = ["s3://my-bucket/raw/{sample}.fastq.gz"]
output = ["local/{sample}.fastq.gz"]
shell = "cp {input[0]} {output[0]}"
```

When the pipeline engine encounters an `s3://` or `gs://` URI, it
detects the remote scheme and stages the file into the workdir before
the rule runs, then uploads rule outputs with remote URIs back to the
bucket after the rule succeeds (see
[Remote staging and upload](#remote-staging-and-upload)). The storage module is also usable
as a library API: callers can resolve URIs and read, write, stage, or
upload objects programmatically through the
[`StorageBackend`](#storage-backend-api) trait.

### Prerequisites

Both backends are feature-gated and are **not** included by default.
Enable them at build time:

```bash
cargo build --release --features "s3-storage,gcs-storage"
```

## AWS S3

The S3 backend uses the official `aws-sdk-s3` Rust SDK. Credentials are
resolved **from environment variables only** — the SDK's broader chain
(shared credentials files, web identity tokens, instance metadata) is not
consulted.

When using MinIO or LocalStack for testing, set `AWS_ENDPOINT_URL` to
point to your local S3-compatible service:

```bash
export AWS_ENDPOINT_URL=http://localhost:9000
export AWS_ACCESS_KEY_ID=minioadmin
export AWS_SECRET_ACCESS_KEY=minioadmin
```

### Example Workflow

```toml
[workflow]
name = "s3-example"
version = "1.0.0"

[[rules]]
name = "align"
input = ["s3://genomics-bucket/raw/{sample}.fastq.gz"]
output = ["s3://genomics-bucket/aligned/{sample}.bam"]
shell = "bwa mem reference.fa {input[0]} | samtools sort -o {output[0]}"

[rules.resources]
threads = 8
```

## Google Cloud Storage

The GCS backend uses the GCS XML API with HMAC-SHA1 authentication.
HMAC keys can be created in the GCP Console under
**Cloud Storage → Settings → Interoperability**.

### Credential Setup

Set the following environment variables:

```bash
export GCS_ACCESS_KEY="GOOG1ABCDEF..."
export GCS_SECRET_KEY="your-secret-key"
```

For interoperability with tools that use S3-style credentials,
`STORAGE_ACCESS_KEY` and `STORAGE_SECRET_KEY` are also accepted.

### Example Workflow

```toml
[workflow]
name = "gcs-example"
version = "1.0.0"

[[rules]]
name = "qc"
input = ["gs://my-bucket/raw/{sample}.fastq.gz"]
output = ["gs://my-bucket/qc/{sample}_report.html"]
shell = "fastqc {input[0]} -o {output[0]}"

[rules.resources]
threads = 2
```

## Storage Backend API

The `StorageBackend` trait in `oxo_flow_core::storage` defines the
interface that all backends implement:

| Method | Description |
|---|---|
| `exists` | Check whether a path exists |
| `read_to_string` | Read a remote file into a UTF-8 string |
| `write` | Write bytes to a remote location |
| `stage` | Download a remote file to a local directory |
| `upload` | Upload a local file to a remote location |
| `head` | Object metadata for invalidation: size + content identity (S3 ETag / GCS `md5Hash`) |
| `name` | Human-readable backend name for diagnostics |

The `StorageResolver` maintains a registry of backends keyed by URI
scheme.  Custom backends can be registered at runtime:

```rust
use oxo_flow_core::storage::{StorageResolver, StorageScheme};
use std::sync::Arc;

let mut resolver = StorageResolver::with_local();
resolver.add_backend(StorageScheme::S3, Arc::new(s3_backend));
```

## Content-addressed invalidation

Input manifests now record remote inputs alongside local ones. When a rule's
inputs include `s3://` / `gs://` URIs and a backend is registered for that
scheme, the checkpoint's input manifest stores a remote entry —
`(scheme, key, size, etag)` — and `manifests_match` compares them:

- S3: the raw `ETag` from `HeadObject`. Composite `"hash-N"` ETags from
  multipart uploads are recorded verbatim and compared for equality — they
  are never recomputed locally. Same content re-uploaded with different
  part boundaries produces a different ETag (a conservative, spurious
  invalidation).
- GCS: `md5Hash` (base64) from the `x-goog-hash` header — GCS has no native
  ETag; the md5 hash is the stronger pure content hash.

A same-size remote rewrite with a changed etag invalidates the rule exactly
as a local content change does; unchanged etag → skipped. When neither side
has an etag, matching degrades to size-only (documented
conservative-for-availability fallback). Without a registered backend the
entry is skipped with a warning — the run completes and the remaining local
entries still invalidate as before. Only exact object references participate:
remote globs and directory references are rejected (the same boundary
staging enforces).

## Remote staging and upload

When a backend is registered for a scheme, the local executor stages remote
inputs before running a rule and uploads remote outputs after it succeeds:

- **Inputs** download into `.oxo-flow/staged/in/<scheme>/<bucket>/<key>`
  before execution. Downloads are cached against a sidecar metadata file
  (`<file>.meta.json` holding size + etag): an unchanged object is never
  re-downloaded, and the cached file keeps its original mtime so the
  executor's freshness gate keeps working. A changed etag re-downloads
  atomically (`.part` → rename) and the fresh mtime correctly marks the
  rule stale.
- **Outputs** declared as remote URIs are written locally to
  `.oxo-flow/staged/out/<scheme>/<bucket>/<key>` — reference them in the
  shell via `{output[n]}` / `{output.name}`. After output validation the
  engine uploads them; an upload failure fails the rule (a declared remote
  output that did not land is a broken contract). Remote outputs are only
  "up to date" while the uploaded object still exists (verified with a
  `head()` on every freshness check), so deleting a cloud result re-runs
  and re-uploads the rule.
- **Shells see staged paths only through placeholders** — the
  substitution happens on a copy of the rule, so `{input[n]}` renders the
  staged local path. A raw `s3://…` URI typed directly into the shell text
  is not rewritten (the engine warns about it). Checkpoint manifests keep
  recording the original remote URIs, so invalidation stays etag-driven.
- **Scope** — staging is a local-executor feature. Cluster runs
  (`BackendDriver`) submit scripts unchanged: nodes use their own shared
  storage. `dry-run` never stages or downloads. Remote globs and directory
  references are rejected (exact object URIs only), remote `temp_output`s
  are unsupported.

Deleting `.oxo-flow/staged/` is safe — a later run re-downloads (and may
re-execute) instead of using the cache.

## Current Limitations

- **Feature-gated** — Both backends are opt-in at compile time: enable
  `oxo-flow-cli`'s `s3-storage` / `gcs-storage` features to register them
  in the shared run/dry-run storage resolver. The default build includes
  only the local filesystem backend. The `s3-storage` feature compiles and
  is tested (unit fixtures + a live MinIO E2E suite, see
  `crates/oxo-flow-cli/tests/remote_staging.rs`).
- **S3 credentials come from the environment** —
  `AWS_ACCESS_KEY_ID` / `AWS_SECRET_ACCESS_KEY` / `AWS_SESSION_TOKEN`
  (plus `AWS_REGION`); the SDK's profile-file/IMDS chain lives in
  aws_config's async loader and is deliberately not loaded — the same
  env-only contract the GCS backend has. S3-compatible servers (MinIO,
  LocalStack) additionally need `AWS_ENDPOINT_URL` and
  `OXO_S3_FORCE_PATH_STYLE=1` (path-style addressing).
- **UTF-8 only** — `read_to_string` requires the content to be valid
  UTF-8.  Binary files should use `stage` instead.
- **Azure Blob Storage** — Not yet supported.  Contributions are
  welcome via the `StorageBackend` trait.
