# Cross-architecture reproduction measurement — first field observation (#1166)

> **Status**: operational field observation, **not** the preregistered primary
> result. The preregistered contrast (PR-01 §C4 / PR-06 §C4) requires
> *identical container digests or identical environment-lock hashes across two
> CPU architectures*. This run uses the `system` environment backend, which has
> no lock and no digest, and it pairs **macOS-arm64 with Linux-x86_64** — a
> comparison that PR-01 explicitly declares **confounded** (architecture + OS +
> toolchain). It may be reported as an operational field observation and must
> never be reported as a pure architecture claim.
>
> **Update (2026-11-28).** A governed follow-up now exists:
> [`cross-arch-provenance-governed-2026-11-27.md`](cross-arch-provenance-governed-2026-11-27.md)
> froze `τ_ARCH = 0.80`/`n_min = 3` before data collection and measured native
> x86_64 vs native aarch64 through one shared container digest: 24/24 declared
> artifacts SHA-256 identical (100%, exact 95% CI [85.75%, 100%]), eight/eight
> classes stable, category POSITIVE. This 2026-11-19 observation remains the
> OS-confounded first field observation and is not the governed result.

## 1. What was measured

A single pinned, deterministic 3-rule `.oxoflow` workflow
(`scripts/cross-arch-provenance.oxoflow`), executed with `oxo-flow run
--provenance`, then re-audited with `oxo-flow provenance verify --json`, on two
real hosts. Only the `system` environment backend is used (no conda/pixi/mamba,
no network): the guaranteed tool set is `sh`/`sort`/`wc`/`gzip`/`python3`.

Three declared artifact classes are produced:

- **pure text** — `data/payload.txt`, a fixed 64-line payload (`echo`, no locale- or FP-sensitive tool);
- **byte-exact compressed** — `data/payload.txt.gz` via `gzip -n -c` (mtime/name stripped from the gzip header, so two different gzip builds should agree);
- **IEEE-754 floating point** — `float.txt` via `python3 -c "print('%.17g' % sum(1.0/i for i in range(1,1001)))"` (`%.17g` round-trips a double exactly).

Plus one **derived text report** — `summary.txt`, a shell composition of
`sort`/`wc -l`/`sort -u`/`head` over the payload (this is where host-tool
divergence becomes observable).

Reusable harness: `scripts/cross-arch-provenance.sh` (runs unchanged on both
hosts; picks up the binary via `OXO_FLOW_BIN` or the repo target dir). It emits
`identity.txt`, `checkpoint.json`, `verify.json`, `artifacts.sha256`, `rows.tsv`
and exits non-zero on any recorded-checksum mismatch.

## 2. Per-arm identity (ledger §8)

| Field | Arm A (local) | Arm B (remote) |
| :--- | :--- | :--- |
| hostname | `shixiangwangdeiMac.local` | `VM-4-4-ubuntu` |
| `uname -a` | `Darwin 25.6.0 … RELEASE_ARM64_T8122 arm64` | `Linux VM-4-4-ubuntu 6.8.0-137-generic … x86_64` |
| arch | `arm64` (Apple M3) | `x86_64` |
| nproc | 8 | 4 |
| oxo-flow | `0.32.0` | `0.32.0` |
| commit (`4b21018f`, same both arms) | `4b21018f9b3975ce3a9796ce0e9a126c35e6bd65` | `4b21018f9b3975ce3a9796ce0e9a126c35e6bd65` |
| `gzip` | `Apple gzip 479` | `gzip 1.12` |
| `python3` | `Python 3.12.4` | `Python 3.12.3` |
| `sort` | `2.3-Apple (199)` (BSD) | `sort (GNU coreutils) 9.4` |
| locale | `LANG=zh_CN.UTF-8` `LC_COLLATE=zh_CN.UTF-8` | `LANG=en_US.UTF-8` `LC_COLLATE=en_US.UTF-8` |
| `sh` | `/bin/sh` (bash) | `/usr/bin/dash` |

Both arms ran the same commit; `workflow_git_sha` recorded in each checkpoint is
`4b21018f9b3975ce3a9796ce0e9a126c35e6bd65`.

## 3. Exact commands

```bash
# arm A (local macOS arm64)
OXO_FLOW_BIN=$PWD/target/debug/oxo-flow \
  bash scripts/cross-arch-provenance.sh scripts/cross-arch-provenance.oxoflow /tmp/armA3

# arm B (tx-ubuntu Linux x86_64); binary prebuilt at the shared target dir
ssh tx-ubuntu 'cd ~/crossarch-1166 && OXO_FLOW_BIN=/home/ubuntu/oxo-flow/target/debug/oxo-flow \
  bash scripts/cross-arch-provenance.sh scripts/cross-arch-provenance.oxoflow /home/ubuntu/armB'
```

Within-arm determinism was established for arm A by two independent runs
(`/tmp/armA`, `/tmp/armA2`): `artifacts.sha256` identical.

## 4. Per-artifact table

| file | status A | sha256 (A) | status B | sha256 (B) | verdict |
| :--- | :---: | :--- | :---: | :--- | :--- |
| `data/payload.txt` | matched | `c3d522d442fe9b8e08ea1cc225886a96b7c6f2520e3ed49ce616912e8de61440` | matched | `c3d522d442fe9b8e08ea1cc225886a96b7c6f2520e3ed49ce616912e8de61440` | **identical** |
| `data/payload.txt.gz` | matched | `4f8c577971a39ab66656127c2e228a35afad3cf185facfba1cf669f9d96e68ff` | matched | `4f8c577971a39ab66656127c2e228a35afad3cf185facfba1cf669f9d96e68ff` | **identical** |
| `float.txt` | matched | `71d665e59199296a7ad04a87a6dec9a1e796263eb29384bb179f65f932423dd6` | matched | `71d665e59199296a7ad04a87a6dec9a1e796263eb29384bb179f65f932423dd6` | **identical** |
| `summary.txt` | matched | `5472870346c71cbaa5608518cf555e0ee8a13b26e0131b42c2250f571dee1472` | matched | `226bc0c70b74a6b87331981a4f6b9230c4658d85078807dbbdeebdcbf46c251a` | **DIFFERS** |

`provenance verify --json` on each arm reports `mismatched: 0, missing: 0,
matched: 4` (each arm re-verifies *its own* checkpoint; the cross-arm divergence
is only visible by comparing the two arms' hashes).

## 5. Enumerated differing set

```
summary.txt
  arm A sha256: 5472870346c71cbaa5608518cf555e0ee8a13b26e0131b42c2250f571dee1472
  arm B sha256: 226bc0c70b74a6b87331981a4f6b9230c4658d85078807dbbdeebdcbf46c251a
```

No other declared artifact differs.

## 6. Verdict, by artifact class

- **Pure text (`data/payload.txt`) — architecture-stable in this observation.**
  Identical SHA across both arms. Produced by shell `echo`; no locale- or
  FP-sensitive tool is involved.
- **Byte-exact compressed (`data/payload.txt.gz`) — architecture-stable in this
  observation.** Identical SHA across two *different* gzip builds (`Apple gzip
  479` vs `gzip 1.12`), confirming `gzip -n` header normalization makes the
  compressed stream host-independent. Header bytes 4–7 are zeroed
  (`1f 8b 08 00 00 00 00 00`).
- **IEEE-754 floating point (`float.txt`) — architecture-stable in this
  observation.** Identical bytes (`7.4854708605503451`) from two different
  Python builds (3.12.4 arm64 vs 3.12.3 x86_64). No FP non-determinism.
- **Derived text report (`summary.txt`) — differs, and the difference is
  ATTRIBUTED to host-tool behavior, not to engine or architecture
  non-determinism.** Two independent, reproduced effects compose in this file:

  1. **`wc -l` output formatting.** Apple/BSD `wc` pads the count to column width
     8 when reading a pipe; GNU `wc` does not. Reproduced directly:
     `printf 'a\nb\n' | wc -l | od -c` → arm A: `       2\n` (seven spaces); arm B: `2\n`.
     BSD `wc` pads the count to width 8 in all single-count invocations (pipe,
     named file, and redirected stdin all measured padded on arm A:
     `wc -l data/payload.txt` → `      64 data/payload.txt`); GNU `wc` pads
     for none of them.
  2. **`sort` collation order.** Arm A's BSD `sort` produced byte order for the
     `sort -u | head -3` lines (`line 1`, `line 10`, `line 11`); arm B's GNU
     `sort` under `en_US.UTF-8` produced dictionary order (`line 10`, `line 11`,
     `line 12`). Reproduced directly: on arm A, forcing `LC_ALL=C`,
     `en_US.UTF-8`, or `zh_CN.UTF-8` all yield `line 1` first (BSD sort ignores
     these collations for this data); on arm B, `LC_ALL=C` yields `line 1` first
     while the default `en_US.UTF-8` yields `line 10` first.

  Both effects are **tool-version / locale effects** (BSD-vs-GNU `wc` padding;
  BSD-vs-GNU `sort` collation), i.e. exactly the class PR-01 §C4 wants recorded
  so a mismatch "can be attributed to tooling rather than to non-determinism".
  Neither is a defect in oxo-flow: the engine faithfully captured each host's
  shell output; both arms' `provenance verify` correctly reported 4/4 matched.

**Primary endpoint (operational).** Architecture-stable artifact proportion =
3/4 = 0.75 over the declared outputs. Under PR-06 §C4 step 1 this is **INVALID
as a primary result** because only the confounded macOS-arm64 vs Linux-x86_64
contrast is available, and because the `system` backend supplies no
environment-lock hash or container digest for either arm. No
architecture-stability claim is made.

## 7. Honesty boundary — what this does NOT measure

- It does **not** satisfy the preregistered primary claim: no identical container
  digests and no identical environment-lock hashes (the `system` backend has
  neither). The contrast is OS-confounded by construction.
- It does **not** measure the container/`docker`/`singularity`/`conda`/`pixi`
  backends, nor any full environment-lock matrix.
- It is **N=1 workflow, 4 artifacts** — far below any `n_min`; `τ_ARCH` and
  `n_min` in PR-06 §C4 **were** UNDECIDED at the time of this observation, so
  no threshold-based statement was possible for it. They are now frozen
  (amendment 2) and the governed campaign is in
  [`cross-arch-provenance-governed-2026-11-27.md`](cross-arch-provenance-governed-2026-11-27.md).
- It does **not** cover the scheduler/cluster path (the cluster arm of #1166 is
  deferred and blocked on cloud credentials; issue #1158).
- The observed `summary.txt` divergence is a host-**tool** divergence deliberately
  surfaced by a shell composition; it is not evidence about oxo-flow's own
  determinism (which the within-arm reruns show is stable), and it is a useful
  illustration that "same command, different host" breaks bitwise output for
  derived text even when the engine is deterministic.

## 7a. Environment-lock identity field (issue #1274) — the unblocking prerequisite

The reason the contrast above is invalid *by construction* is that neither arm
recorded any environment-lock identity: the `system` backend supplies neither a
lock hash nor a container digest, so nothing was held constant across hosts.

Issue #1274 adds the in-repo recording layer that a future two-arm re-run needs.
It claims **no** two-arm measurement — it only makes environment-lock equality
assertable instead of assumed.

- **New checkpoint field:** `env_lock_identity` — a `BTreeMap<rule name,
  identity string>` in `.oxo-flow/checkpoint.json` (`CheckpointState`), persisted
  when a run records its checkpoint. It is surfaced verbatim by
  `oxo-flow provenance verify <checkpoint> --json` (under
  `verify.env_lock_identity`) and echoed in the human report.
- **Identity, per backend** (a self-describing `"<kind>|<value>"` string):
  - container (`docker`/`singularity`) → the resolved image *digest*
    (`repo@sha256:...`), never a tag; when the runtime cannot report one offline
    (no pull), the explicit sentinel `docker|unresolved`.
  - env backends (`pixi`/`conda`/`mamba`/`venv`) → a deterministic, key-ordered
    SHA-256 *lock hash* over the resolved spec files (including the sibling
    `pixi.lock`): `pixi|sha256:...`.
  - `system` (or any rule with no lockable content) → the explicit sentinel
    `system|unavailable`.
- **Detection:** `oxo_flow_core::env_lock::env_lock_is_resolved(value)` returns
  `false` for either sentinel, so a caller can tell "no lock" / "not resolved"
  from real lock content. A digest is **never fabricated**; a component that
  fails to resolve does not get hashed into a value that could read as a false
  match.
- **Harness:** `scripts/cross-arch-provenance.sh` now emits `env_lock.txt`
  (`rule<TAB>identity<TAB>resolved`) read from the checkpoint, so two arms'
  files can be diffed.

**Protocol to make two arms identical except architecture** (for the future
re-run): pin the fixture to a lockable backend (e.g. a `pixi` rule with a
committed `pixi.lock`, or a container rule), run the harness on both hosts, then
require every rule's identity in both `env_lock.txt` files to be byte-identical
**and** resolved (`resolved=yes`). Only then does an architecture contrast carry
weight. The two-arm measurement itself is **still pending hosts** (#1169 is the
container-substrate blocker); this field is the prerequisite, not the result.

## 8. Raw-data lineage

- Arm A: `/tmp/armA3/{identity.txt,checkpoint.json,verify.json,artifacts.sha256,rows.tsv}` (run `/tmp/armA3/work`); determinism baseline `/tmp/armA`, `/tmp/armA2`. (The `env_lock.txt` artifact and the `env_lock_identity` checkpoint field post-date this observation — see §7a.)
- Arm B: `tx-ubuntu:/home/ubuntu/armB/…` (run `/home/ubuntu/armB/work`), copied to `/tmp/armB/`.
- Fixture + harness: `scripts/cross-arch-provenance.oxoflow`, `scripts/cross-arch-provenance.sh` (the harness now also emits `env_lock.txt`).
