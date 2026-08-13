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
detects the remote scheme and logs a warning — the executor does not
yet stage remote files into the workdir or upload outputs back (see
[Current Limitations](#current-limitations)). The storage module is
usable today as a library API: callers can resolve URIs and read,
write, stage, or upload objects programmatically through the
[`StorageBackend`](#storage-backend-api) trait.

### Prerequisites

Both backends are feature-gated and are **not** included by default.
Enable them at build time:

```bash
cargo build --release --features "s3-storage,gcs-storage"
```

> The example workflows below illustrate the URI syntax only — remote
> URIs are not yet staged or uploaded by the executor (see
> [Current Limitations](#current-limitations)).

## AWS S3

The S3 backend uses the official `aws-sdk-s3` Rust SDK with the standard
AWS credential chain.  No additional configuration is required beyond
what the AWS SDK normally reads.

### Credential Resolution

The SDK discovers credentials in this order:

1. Environment variables (`AWS_ACCESS_KEY_ID`, `AWS_SECRET_ACCESS_KEY`,
   `AWS_SESSION_TOKEN`)
2. `~/.aws/credentials` (standard AWS config file)
3. Web identity tokens
4. Instance metadata (EC2, ECS)

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
| `name` | Human-readable backend name for diagnostics |

The `StorageResolver` maintains a registry of backends keyed by URI
scheme.  Custom backends can be registered at runtime:

```rust
use oxo_flow_core::storage::{StorageResolver, StorageScheme};
use std::sync::Arc;

let mut resolver = StorageResolver::with_local();
resolver.add_backend(StorageScheme::S3, Arc::new(s3_backend));
```

## Current Limitations

- **Not wired into the executor** — The engine only detects remote
  URIs and logs a warning (`warn_if_remote_paths` in the executor is a
  stub); it does not stage inputs or upload outputs during a run.
  Workflows referencing `s3://`/`gs://` paths currently fail at
  execution unless the tool handles the URI itself. Full staging and
  upload integration is planned.
- **Feature-gated** — Both backends are opt-in at compile time.  The
  default build includes only the local filesystem backend.
- **UTF-8 only** — `read_to_string` requires the content to be valid
  UTF-8.  Binary files should use `stage` instead.
- **Azure Blob Storage** — Not yet supported.  Contributions are
  welcome via the `StorageBackend` trait.
