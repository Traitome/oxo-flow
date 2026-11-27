# Cross-architecture reproduction measurement — container-digest two-arm run (#1166)

> **Status**: measured two-architecture comparison using **one shared
> multi-arch container digest** on **two CPU architectures, one OS
> (Linux)** — the non-OS-confounded contrast the preregistration asks for. It
> is still **not** the preregistered primary result as a governed
> POSITIVE/NEGATIVE: the governed thresholds `τ_ARCH` and `n_min`
> (`docs/superpowers/preregistration/PR-06-publication-decision-rule.md` §C4)
> **were** UNDECIDED at the time of this observation, and one arm ran under
> **CPU emulation** (stated below); the later governed campaign is in
> [`cross-arch-provenance-governed-2026-11-27.md`](cross-arch-provenance-governed-2026-11-27.md).
> The per-artifact and per-class verdicts are reported as an operational
> measurement, with every caveat the brief requires.
>
> **Update (2026-11-28).** A governed follow-up now exists:
> [`cross-arch-provenance-governed-2026-11-27.md`](cross-arch-provenance-governed-2026-11-27.md)
> froze `τ_ARCH = 0.80`/`n_min = 3` before data collection and replaced the
> qemu-emulated arm64 leg with a **native aarch64** leg (Apple Silicon Docker
> Desktop `linux/arm64` VM): 2 workflows / 24 declared artifacts / 8 classes,
> one shared manifest-list digest, 24/24 artifacts SHA-256 identical (100%,
> exact 95% CI [85.75%, 100%]), eight/eight classes stable, category POSITIVE.
> This 2026-11-20 emulated-arm measurement remains an operational measurement
> and is not the governed result.

This supersedes the earlier **OS-confounded** `system`-backend observation
(`docs/superpowers/audits/cross-arch-provenance-2026-11-19.md`, macOS-arm64 vs
Linux-x86_64), which PR-01 §C4 declares confounded. That document remains as
the first field observation; this document is the container-digest measurement.

## 1. Question

Does the *same pinned workflow*, run on a **different CPU architecture**,
yield **identical recorded provenance checksums** — and if not, exactly which
declared output artifacts differ, by class? Cross-architecture bit
reproducibility is the hardest cell; it is measured here, not assumed.

## 2. Method

A single pinned, deterministic 3-rule `.oxoflow` fixture
(`scripts/cross-arch-provenance-container.oxoflow`) produces **8 declared
outputs spanning six artifact classes** engineered to expose architecture
sensitivity:

| artifact | class | how produced |
| :--- | :--- | :--- |
| `data/payload.txt` | pure-text | shell `echo`, fixed 64-line payload |
| `data/bytes.bin` | raw-binary | `python3` fixed 4096-byte pattern (no `/dev/urandom`) |
| `summary.txt` | derived-text | `sort`/`wc -l`/`sort -u`/`head` composition |
| `data/payload.txt.gz` | byte-exact-compressed | `gzip -n -c` (header mtime/name stripped) |
| `data/bytes.bin.gz` | byte-exact-compressed | `gzip -n -c` |
| `manifest.sha256` | checksum-manifest | `sha256sum` of the two inputs |
| `float.txt` | ieee754-float | `python3 '%.17g' % sum(1.0/i for i in range(1,1001))` |
| `float_series.txt` | ieee754-float | `python3` `fsum` vs `sum` over 2000 values + difference |

Each rule declares `environment = { docker = "crossarch1166:pin" }`. The image
is `python:3.12-slim` pinned by **one multi-arch manifest-list digest**
`sha256:5e2bdb1b0d542d5039cffb8181c65930093949194a65c365dfb1b0fe13a4fa5b`.
A single manifest-list digest resolves to a per-platform layer set, so **both
arms share an identical container *identity* (digest) and an identical
toolchain** (same coreutils 9.7, same Python 3.12.16, same gzip 1.13) — only
the resolved CPU architecture differs. This removes the OS/toolchain confound
the earlier observation carried.

Both arms run `oxo-flow run <fixture> --provenance`, then
`oxo-flow provenance verify <ckpt> --json`. The harness
(`scripts/cross-arch-provenance-container.sh`) stages each platform's image
from the shared digest, records host + container identity, copies the
checkpoint, re-audits it, hashes artifacts independently, and exits non-zero on
any mismatch. `scripts/cross-arch-provenance-compare.sh` +
`cross-arch-provenance-compare.py` drive both arms and emit the machine-
readable verdict.

The fixture references the staged **tag** `crossarch1166:pin`, not the digest,
because Docker's classic (non-containerd) snapshotter stores a digest-only
reference for exactly ONE platform and refuses to re-resolve it to another
(`cannot overwrite digest`); both staged images nevertheless come from the ONE
shared manifest-list digest, and the resolved per-platform image ids are
recorded in `container.txt`.

## 3. The two architectures (one native, one emulated — stated honestly)

| | Arm A | Arm B |
| :--- | :--- | :--- |
| **platform** | `linux/amd64` | `linux/arm64` |
| **execution** | **native** | **qemu-emulated** (qemu-user-static + binfmt_misc, `qemu-aarch64` registered) |
| host | `VM-4-4-ubuntu` (x86_64, Linux 6.8.0-137) | same host (`VM-4-4-ubuntu`, x86_64) |
| in-container `uname -m` | `x86_64` | `aarch64` |
| shared manifest-list digest | `sha256:5e2bdb1…4fa5b` | `sha256:5e2bdb1…4fa5b` |
| resolved per-platform image id | `sha256:78976a69c067…41ca0db` | `sha256:3617028275c0…f3c1d31ae6` |
| image arch | `amd64` | `arm64` |
| `python3` | 3.12.16 | 3.12.16 |
| `sort` / `wc` | GNU coreutils 9.7 | GNU coreutils 9.7 |
| `gzip` | 1.13 | 1.13 |
| oxo-flow | 0.32.0 | 0.32.0 |
| engine commit (checkpoint `workflow_git_sha`) | `4b21018f9b3975ce3a9796ce0e9a126c35e6bd65` | `4b21018f9b3975ce3a9796ce0e9a126c35e6bd65` |
| date (UTC) | 2026-11-19T19:53:06Z | 2026-11-19T19:53:32Z |

Both checkpoints record the same engine commit `4b21018f9b3975ce3a9796ce0e9a126c35e6bd65`
(`workflow_git_sha`). The `commit:` line in each `identity.txt` shows the
placeholder `1166-local` because the harness was invoked with
`CROSSARCH_COMMIT=1166-local` on the shared build host; the authoritative
engine commit is the checkpoint's `workflow_git_sha`. The fixture source is
byte-identical to the committed `scripts/cross-arch-provenance-container.oxoflow`.

**Emulation is a stated methodology, not a hidden one.** Arm B executes
linux/arm64 binaries under qemu running on the same x86_64 host. This is a
genuinely distinct *architecture* (aarch64 vs x86-64: different instruction
set, different floating-point unit), which is exactly the axis under test;
it is **not** native arm64 silicon. A native second architecture was not
available on `tx-ubuntu` (it is an x86_64 VM), so the two genuinely distinct,
executable architectures obtained are native `linux/amd64` and emulated
`linux/arm64`. This is the legitimate methodology the brief sanctions and it is
recorded here and in §7 as a caveat.

## 4. Per-artifact result

`provenance verify --json` reported `matched: 8, mismatched: 0, missing: 0` on
**each** arm (each arm re-verifies its own checkpoint). The cross-arm
comparison is the second axis:

| artifact | class | arm A sha256 | arm B sha256 | verdict |
| :--- | :--- | :--- | :--- | :---: |
| `data/payload.txt` | pure-text | `c3d522d4…61440` | `c3d522d4…61440` | **identical** |
| `data/bytes.bin` | raw-binary | `10306603…12f75` | `10306603…12f75` | **identical** |
| `summary.txt` | derived-text | `110991a1…1b10ee` | `110991a1…1b10ee` | **identical** |
| `data/payload.txt.gz` | byte-exact-compressed | `4f8c5779…96e68ff` | `4f8c5779…96e68ff` | **identical** |
| `data/bytes.bin.gz` | byte-exact-compressed | `62de6539…ab1b536` | `62de6539…ab1b536` | **identical** |
| `manifest.sha256` | checksum-manifest | `01eb1d2c…faeef5f` | `01eb1d2c…faeef5f` | **identical** |
| `float.txt` | ieee754-float | `71d665e5…423dd6` | `71d665e5…423dd6` | **identical** |
| `float_series.txt` | ieee754-float | `96330315…7d54ee3` | `96330315…7d54ee3` | **identical** |

**8 / 8 artifacts SHA-256 identical across native linux/amd64 and emulated
linux/arm64. Differing set: empty.** Full 64-hex digests are in
`docs/superpowers/audits/data/crossarch-1166/crossarch-results.json`.

## 5. Verdict, by artifact class

Every eligible class is **architecture-stable in this measurement** — every
artifact in the class SHA-256 identical across the two architectures:

| class | artifacts | identical | architecture-stable? |
| :--- | :---: | :---: | :---: |
| pure-text | 1 | 1 | **yes** |
| raw-binary | 1 | 1 | **yes** |
| derived-text | 1 | 1 | **yes** |
| byte-exact-compressed | 2 | 2 | **yes** |
| checksum-manifest | 1 | 1 | **yes** |
| ieee754-float | 2 | 2 | **yes** |

Notable: the **derived-text** report (`summary.txt`, a `sort`/`wc`/`head`
composition) is identical here, where in the earlier OS-confounded observation
it differed — because both arms now use the *same* GNU coreutils/locale, so
the BSD-vs-GNU tool effect is gone and only the architecture axis remains.
The **ieee754-float** outputs also reproduced bit-exactly: x86-64 and aarch64
use different floating-point instruction sets, yet Python 3.12.16 on glibc
produced identical rounding for `sum`, `math.fsum`, and their difference over
these values (`%.17g` round-trips a double exactly, so this is a real bit
match, not formatting loss).

## 6. Decision-rule mapping (honest status)

Under `PR-06-publication-decision-rule.md` §C4: step 1 (INVALID) — the
measurement is not invalid as a contrast (one OS, one shared container digest,
two architectures) **but** it carries a stated emulation caveat and `N=1`
fixture; step 2 (NEGATIVE) — would require an eligible class (≥ `n_min`) with
≥1 differing artifact; **no class differs**, so no NEGATIVE; step 3 (interval
partition on `τ_ARCH`) — `τ_ARCH` was **UNDECIDED** at the time of this
observation, so **no governed POSITIVE is claimed for it**; the later
governed campaign is in
[`cross-arch-provenance-governed-2026-11-27.md`](cross-arch-provenance-governed-2026-11-27.md).
The honest, report-local statement for this observation is: *every declared artifact
class was architecture-stable in this one container-digest, one-OS,
native-vs-emulated measurement.*

## 7. Exclusions / caveats

- **CPU emulation on Arm B.** `linux/arm64` ran under qemu-user on an x86_64
  host (see §3). Distinct architecture, but not native arm64 silicon; an
  emulator could in principle mask or introduce an architecture-specific
  effect. Native-vs-native cross-architecture is not measured here.
- **`N=1` fixture, 8 artifacts.** Far below any `n_min`; `τ_ARCH` and `n_min`
  **were** UNDECIDED at the time of this observation — no threshold-based
  statement was possible for it; the later governed campaign is in
  [`cross-arch-provenance-governed-2026-11-27.md`](cross-arch-provenance-governed-2026-11-27.md).
- **Toolchain is deliberately held constant** (same image digest ⇒ same
  coreutils/Python/gzip). This isolates the architecture axis but means the
  measurement says nothing about how *differing* tool versions behave across
  architectures; that is the environment-lock-matrix axis, still unmeasured.
- **Per-platform image digests differ** (`78976a69…` vs `36170282…`) because a
  multi-arch digest resolves to per-platform layer sets; identity is pinned by
  the shared manifest-list digest, and the per-platform ids are recorded.
- **Registry egress**: `registry-1.docker.io` is network-blocked from
  `tx-ubuntu`; images were pulled via the host's configured registry mirror
  (`/etc/docker/daemon.json`). The mirror serves the same canonical
  manifest-list digest; this affects how the image was fetched, not what was
  fetched.
- **Only the `docker` container backend** is exercised. `system`, `singularity`,
  `conda`, `pixi`, and the cluster/scheduler path (#1158, blocked on cloud
  creds) remain out of scope.

## 8. Raw-data lineage

- Arm A (`linux/amd64`): `docs/superpowers/audits/data/crossarch-1166/arm-amd64/{identity.txt,container.txt,checkpoint.json,verify.json,artifacts.sha256,rows.tsv}`.
- Arm B (`linux/arm64`, emulated): `docs/superpowers/audits/data/crossarch-1166/arm-arm64/…` (same set).
- Machine-readable verdict: `docs/superpowers/audits/data/crossarch-1166/crossarch-results.json`.
- Fixture + harness: `scripts/cross-arch-provenance-container.oxoflow`,
  `scripts/cross-arch-provenance-container.sh`,
  `scripts/cross-arch-provenance-compare.sh`,
  `scripts/cross-arch-provenance-compare.py`.

Reproduce:

```bash
# on a Linux host with docker + qemu-user-static/binfmt (for the arm64 arm):
CROSSARCH_IMAGE_SRC=python@sha256:5e2bdb1b0d542d5039cffb8181c65930093949194a65c365dfb1b0fe13a4fa5b \
OXO_FLOW_BIN=/path/to/oxo-flow \
  bash scripts/cross-arch-provenance-compare.sh \
    scripts/cross-arch-provenance-container.oxoflow /tmp/crossarch-1166
# emits /tmp/crossarch-1166/crossarch-results.json
```
