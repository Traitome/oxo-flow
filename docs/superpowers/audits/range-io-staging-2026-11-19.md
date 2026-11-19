# Range I/O and object-storage staging — first measured evidence (#1250)

> **Status**: first measured evidence for ROADMAP Phase 12.1 row *"Range I/O and
> object-storage staging"*. It measures the engine's **remote staging** path
> (`eager` download-to-scratch vs `stream` FIFO) against a **live S3-compatible
> object store**, producing the four exit-criterion measurements: end-to-end
> streaming throughput, interruption/resume behavior, backpressure bounding,
> and scratch footprint. It closes #1250. It is **not** a production
> object-store, multi-node, or network-partition campaign (see §7).

## 1. What was measured

The engine stages remote objects into a rule's working tree through
`crates/oxo-flow-core/src/executor/staging.rs` and
`crates/oxo-flow-core/src/storage/mod.rs`. `StagingMode` selects the path:

- **`eager`** (default) — `backend.stage()` downloads the whole object into
  `.oxo-flow/staged/in/<scheme>/<bucket>/<key>` (via `.part` + atomic rename +
  etag sidecar), then the shell reads a **regular file**.
- **`stream`** (aka `fifo`) — `mkfifo` a named pipe at the same path and a
  `tokio::task::spawn` task loops `read_range(offset, CHUNK_SIZE)` (4 MiB)
  copying the object into the FIFO as the consumer drains it; broken pipe on
  consumer exit is treated as success; a `StreamGuard` removes the FIFO on drop.
- **`passthrough`** — hands the raw URI to the shell (not measured here).

Against the **real `oxo-flow` binary** and a **live S3-compatible server**
(RustFS; MinIO's community server is no longer distributed — the same
substitution the engine's own `crates/oxo-flow-cli/tests/remote_staging.rs`
uses), the harness runs a one-rule workflow `input = ["s3://<bucket>/<key>"]`
under each mode and records raw numbers for:

- **M1 end-to-end streaming** — wall time, MiB/s, peak RSS for `eager` vs
  `stream` over the *same* 256 MiB object, fast consumer.
- **M2 interruption / resume** — SIGKILL the engine mid-`eager` download (once
  the `.part` passes 4 MiB) and mid-`stream` read (once the FIFO exists + 1 s),
  record the leftover artifact and failure mode, then re-run and assert the
  resumed output is byte-correct.
- **M3 backpressure** — a deliberately slow consumer (16 MiB/s) drains the
  pipe; record whether the producer (engine) memory/scratch stays bounded.
- **M4 scratch footprint** — peak bytes under `.oxo-flow/staged` for `eager`
  vs `stream` over one object.

Correctness for every arm is `sha256sum {input[0]}` written to `out.txt`,
compared to the seeded object's known SHA-256.

## 2. Per-arm identity (ledger §8)

| Field | Value |
| :--- | :--- |
| hostname | `tx-ubuntu` |
| `uname -a` | `Linux 6.8.0-137-generic x86_64` |
| arch | `x86_64` |
| CPUs | 4 (`Intel(R) Xeon(R) Platinum 8255C @ 2.50GHz`) |
| RAM | 3812568 kB (~3.6 GiB) |
| OS | Ubuntu 24.04.4 LTS |
| object-store server | RustFS `quay.io/rustfs/rustfs:latest`, loopback `http://127.0.0.1:9000` |
| corpus baseline commit | `1face885104d24af95b213d52b90ed68373ab957` (staging + storage code unchanged to this PR's head) |
| harness | introduced in this PR's head (`scripts/range-io-staging.sh`, `scripts/range_io_staging_run.py`) |
| branch | `research/1250-range-io-staging` |
| oxo-flow | `0.32.0` (`target/debug/oxo-flow`, built `--features s3-storage`) |
| rustc / cargo | `1.99.1` / `1.99.1` |
| python3 | `3.12.3` |
| docker | `29.7.2` |
| shell | `bash` (harness `set -euo pipefail`) |
| date (UTC) | 2026-11-19 |

## 3. Exact command

```bash
# from the worktree root, after:
#   cargo build -p oxo-flow-cli --features s3-storage
export OXO_FLOW_BIN="$PWD/target/debug/oxo-flow"
scripts/range-io-staging.sh --no-serve --self-test --obj-mib 256 --keep
```

The harness brings up RustFS (unless `--no-serve`), seeds a 256 MiB random
object, runs M1–M4 plus the FAIL-FIRST self-test, and writes
`results.json` into the scratch root. It cannot run vacuously: it resolves a
real `oxo-flow` binary (`OXO_FLOW_BIN` / `--bin` / `target/debug` / `PATH`) and
fails hard if none is found, and it probes the S3 endpoint with a live
`list_buckets` before running. It captures the driver status as
`rc=0; python3 ... || rc=$?` so `set -euo pipefail` cannot abort before
cleanup/exit.

## 4. Pre-registered acceptance criterion

**Registered 2026-11-19T05:30Z, BEFORE the measured run.** For one 256 MiB
object on a live S3-compatible endpoint:

> (a) **End-to-end streaming** both modes complete with `rc=0` and
> `out.txt` SHA-256 equal to the seeded object's, over the *same* object;
> (b) **interruption** both modes fail deterministically (non-zero exit) when
> SIGKILLed mid-transfer, and the **resume** re-run produces a byte-correct
> output with no stale artifact left behind;
> (c) **backpressure** the `stream` producer's peak footprint stays **bounded**
> — much smaller than the object size, not scaling with it;
> (d) **scratch footprint** `stream` peak scratch is ~0 while `eager` peak
> scratch ≈ the object size, i.e. streaming does **not** buffer the full object
> to scratch;
> (e) **non-vacuity** perturbing the streaming arm to `eager` flips the
> scratch-footprint discriminator, proving the measurement observes the staging
> path (a harness blind to it would report ~0 for both).

## 5. Raw numbers

Object: `268435456` bytes, SHA-256
`60584d982e0c13ab24fb3e8371bb9d53a4a4f004014e3b6dc256674461f98b8e`.
Slow consumer: `16` MiB/s. All arms `ok=True` unless noted.

| arm | rc | wall (s) | MiB/s | peak RSS (kB) | peak scratch (B) | ok |
| :--- | :--- | :--- | :--- | :--- | :--- | :--- |
| M1/M4 `eager` | 0 | 3.7402 | 68.45 | 61348 | 268435523 | true |
| M1/M4 `stream` | 0 | 2.7351 | 93.60 | 90516 | **0** | true |
| M2 `eager` (killed mid-download) | -9 (SIGKILL) | 0.3244 | — | 61048 | 11533983 | — |
| M2 `stream` (killed mid-read) | -9 (SIGKILL) | 1.2989 | — | 90452 | 0 | — |
| M3 `stream` (slow consumer) | 0 | 16.2926 | 15.71 | 93380 | **0** | true |
| M3 `eager` (slow consumer) | 0 | 18.7607 | 13.65 | 63172 | 268435523 | true |
| self-test (stream perturbed→eager) | 0 | 3.5284 | 72.55 | 62828 | 268435523 | true |

**M2 interruption + resume detail:**

| field | `eager` | `stream` |
| :--- | :--- | :--- |
| kill trigger | `.part` > 4 MiB | FIFO present + 1.0 s |
| leftover artifact | `…/obj-256MiB.bin.part` (11.5 MiB) | `…/obj-256MiB.bin` (FIFO) |
| resume rc | 0 | 0 |
| resume wall (s) | 3.5579 | 16.2936 |
| resume peak RSS (kB) | 64424 | 85012 |
| resume peak scratch (B) | 268435523 | 0 |
| resume `out.txt` SHA-256 | `60584d98…f98b8e` (= seeded) | `60584d98…f98b8e` (= seeded) |
| stale artifact after resume | none (`.part` gone) | none (FIFO gone, `staged_is_fifo=False`) |

**Self-test (FAIL-FIRST):** `stream_scratch = 0 B` (< 2% of object: true),
`perturbed_scratch = 268435523 B` (> 90%: true) → **PASS**. The scratch
discriminator flips when the streaming arm is perturbed to `eager`, so the
measurement is not vacuously reporting ~0 for both.

## 6. Observations

- **Streaming is faster and far cheaper in scratch here.** Same object, same
  host: `stream` 93.6 MiB/s vs `eager` 68.45 MiB/s (M1); `stream` peak scratch
  **0 B** vs `eager` **256 MiB** (M4). Streaming does not materialize the
  object to scratch — the FIFO's `st_size` is 0 by construction, and the
  producer stays bounded at ~90 MiB RSS while copying 256 MiB in 4 MiB chunks.
- **Backpressure is bounded (criterion (c) met).** With a 16 MiB/s consumer,
  the `stream` producer wall time stretched to the consumer's pace (16.29 s ≈
  256 MiB / 16 MiB/s) while peak RSS stayed at 93 MiB — **no ballooning to
  object size, no scratch growth**. The FIFO write blocks the `read_range`
  loop, so the object is not buffered ahead. **No defect found**; no separate
  issue filed.
- **Interruption is clean and resume is correct.** Both modes died
  deterministically on SIGKILL (`rc=-9`). `eager` left a resumable `.part`
  (11.5 MiB); `stream` left the FIFO. The resume run in both cases produced a
  byte-correct `out.txt` and left **no stale artifact** (`eager`'s `.part`
  consumed into the final file; `stream`'s FIFO removed). The `stream` resume
  wall time (16.29 s) equals the slow-consumer arm because the same workflow
  (slow consumer) was re-run.
- **`eager` scratch equals the object size by design**, not a leak: it is the
  staged regular file the shell reads (268435523 B = 268435456 B + the
  transient `.part` peak). The `< 2%` / `> 90%` self-test thresholds encode
  exactly this.

## 7. Honesty boundary

**What this is.** A four-measurement campaign of the engine's remote-staging
path against a **real S3-compatible server on loopback**, using the real CLI
binary built with `--features s3-storage`. It exercises the exact code path
`StagingMode` selects in `staging.rs`/`storage/mod.rs`.

**What this is NOT, stated loudly.**

- **Not a production object store.** The server is **RustFS** on `127.0.0.1`,
  not AWS S3 / GCS / Azure. MinIO's community server is no longer distributed,
  so RustFS is the substitute the engine's own gated E2E test also uses. Latency,
  retry, and consistency behavior on a WAN object store are **not** measured.
- **S3 only.** **GCS and Azure** staging were **not exercised at all** (no
  endpoint stood up). The criterion is met for the S3 path only.
- **Loopback / OS confound.** Producer and store share one host and one kernel;
  the "throughput" numbers include no real network. They are an **upper bound**,
  not a network measurement.
- **Single node, single run per arm.** No multi-node, no concurrent-instance,
  no multi-rule fan-out concurrency. **N = 1** per cell; no variance is
  characterized.
- **Local SIGKILL, not a network partition.** Interruption is a process kill on
  the same host — not a dropped connection, a stalled store, or a partial
  socket read. Re-staging after a *network* fault is **not** measured.
- **Small-object floor.** One 256 MiB object. No multi-GiB object, no many-object
  fan-out; the scratch/backpressure conclusions are for the measured size.
- **No engine change.** This PR adds measurement + docs only. It does not
  modify the staging code and does not close the broader "production-scale"
  question for the row.

**What would move it further**: a real WAN object store, GCS/Azure endpoints,
multi-node and concurrent fan-out, a network-partition injection, and multi-GiB
objects.

## 8. Raw-data lineage

| Artifact | Path |
| :--- | :--- |
| Harness (bash) | `scripts/range-io-staging.sh` |
| Measurement driver (python) | `scripts/range_io_staging_run.py` |
| Staging logic | `crates/oxo-flow-core/src/executor/staging.rs` |
| Storage / `StagingMode` / `stage_with_cache` | `crates/oxo-flow-core/src/storage/mod.rs` |
| Effective-mode resolution | `crates/oxo-flow-core/src/rule.rs` (`effective_staging_mode`) |
| Gated live E2E precedent (RustFS contract) | `crates/oxo-flow-cli/tests/remote_staging.rs` |
| Raw run output | `results.json` in the `--keep` scratch root |
| Corpus baseline commit | `1face885104d24af95b213d52b90ed68373ab957` — staging/storage code unchanged from this commit to the PR's head |
