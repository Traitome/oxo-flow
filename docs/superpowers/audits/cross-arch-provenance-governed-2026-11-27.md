# Governed cross-architecture reproduction campaign (#1166 / #1397)

> **Status: governed campaign executed under PR-06 §C4 (amendment 2).**
> The pre-registration values were frozen and committed before the first run
> (freeze commit `dd5dcd44ee91a2c93bfd24c29f189d8de32e87d5`, 2026-11-28
> 00:52:56 +08:00 / 2026-11-27T16:52:56Z). Two harness revisions followed
> before the final analyzable run; the manifest lineage and the
> attempt-1/attempt-2 timeline are in §2.1. The campaign measures **native
> x86_64** against **native aarch64** through one shared multi-arch
> manifest-list digest, with an excluded negative control that must differ.
> PR-05 C4 independent reviewer sign-off remains **PENDING**; per PR-05 this
> result is a governed decision under the frozen rule, not a publication-ready
> confirmatory result.

## 1. Question

Does the same frozen, deterministic workflow produce **bit-identical declared
outputs** on two different CPU architectures when the operating system and
toolchain are held identical by one pinned container image? PR-01 §C4 defines
the primary endpoint and calls a macOS-arm64 versus Linux-x86_64 comparison
confounded; this campaign uses Linux containers on both arms, so the executed
environment is the same and only the CPU architecture differs.

## 2. Pre-registration and freeze record

### 2.1 Amendment and manifest

| Item | Value |
| :--- | :--- |
| Campaign | #1166 (governed run filed as #1397) |
| Pre-registration section | PR-06 §C4 |
| Version | PR-06 v1.0.2 (amendment 2) |
| Freeze date | 2026-11-28 |
| Parameter-freeze commit | `dd5dcd44ee91a2c93bfd24c29f189d8de32e87d5` |
| Parameter-freeze timestamp | 2026-11-28T00:52:56+08:00 (`2026-11-27T16:52:56Z`) |
| Freeze-time manifest SHA-256 | `0eaca24be6d00f5ab369f1fb6769131bfeed336107128e76b26ccb6c4ff63908` |
| Final-run manifest SHA-256 | `770ae8f2769f112cd308249ea94882dee2b8c6e7a7e383550d790c229c2a75bf` (commit `56b109c17b95e4c610a874aaf63fc0ae583a57a6`, 2026-11-27T17:00:43Z) |
| Post-run review-hardened manifest SHA-256 | `3082b22899537eef7a75585f61347b4d81cf57d727565ee35d5f0f062f1e550e` |
| `MANIFEST.sha256` | `b07721b846cbd74808cae156cebdbb93f4e6c140cf289aad217ea6cc06e07609` |
| Pre-run/final-run manifest | `docs/superpowers/audits/data/crossarch-1397/preregistration.json` |

The amendment log entry 2 records the value changes and that **no confirmatory
outcome was examined** before the parameter freeze. The two earlier #1166
observations (the OS-confounded 2026-11-19 field observation and the
2026-11-20 native-amd64-versus-qemu-emulated-arm64 container observation) are
report-local operational measurements; they are not this corpus and did not
set any frozen value.

**Timeline (all UTC, 2026-11-27).** `16:52:56` parameter freeze, commit
`dd5dcd44`, freeze manifest `0eaca24b`; `16:56:36` attempt-1 starts and Docker
Desktop crashes on `docker rmi` during staging (identity only, no workflow
artifact); `16:59:01` staging-fix commit `834dc183`; `16:59:18` attempt-2
complete two-arm run with an evidence-stem defect (checkpoints/outputs exist
but were **not** analyzed as the campaign result; preserved under
`attempt-2-stem-mismatch/`); `17:00:43` stem-fix commit `56b109c1`, final-run
manifest `770ae8f2`; `17:01:00`–`17:01:19` final analyzable run. The
freeze-time and final-run manifests carry the **same scientific hashes**
(corpus, workflows, analysis script, PR-06, MANIFEST); only harness hashes and
status text changed. The current file is the post-run review-hardened
revision (amendment log entry 6), which adds sealed-hash/host-arch/tool/
env-lock checks without changing the scientific values or the observed
hashes.

### 2.2 Frozen values

| Parameter | Frozen value |
| :--- | :--- |
| `τ_ARCH` | **0.80** |
| `n_min` | **3** |
| Primary endpoint | Architecture-stable artifact proportion: artifacts with identical SHA-256 in both arms / artifacts produced in both arms. Higher is better. |
| Planned N | **24** primary declared artifacts (2 workflows × 12), in **8 classes of 3** |
| CI method | Exact Clopper-Pearson two-sided 95% interval; no p-value |
| Threshold derivation | All-identical exact lower bound `Beta.ppf(0.025, 24, 1) = 0.025^(1/24) = 0.857526`, which clears 0.80. The threshold is set by the planned N and the interval construction, not by an observed outcome. |
| Environment lock | One multi-arch manifest-list digest `sha256:5e2bdb1b0d542d5039cffb8181c65930093949194a65c365dfb1b0fe13a4fa5b`; identical in-container tool versions; same engine commit with `workflow_git_dirty = false` |
| Negative control | `crossarch-governed-a/control/os_arch.txt` (`uname -m`), excluded from the primary endpoint and every class statement; must differ |
| Missingness limit | **0%** — any missing primary artifact makes the campaign INVALID |

### 2.3 Manifest hashes (executed/final-run and post-run hardened)

The machine-readable pre-run manifest records the SHA-256 of every campaign
file before the run. The sealed hashes are:

| File | SHA-256 |
| :--- | :--- |
| `scripts/cross-arch-provenance-governed-corpus.json` | `33a7531882f266dc067ba705b91863eacb709dee36bc00c2720aa0e6aba20bfa` |
| `scripts/cross-arch-provenance-governed-a.oxoflow` | `8daa37215b0a5871253094d23cbe32035d21737c9a5fa097f63bbd5587e12847` |
| `scripts/cross-arch-provenance-governed-b.oxoflow` | `8e6d1cf56939df49e2e760cf68249a85e32e53569b3bc9dd50043dc0fcaa8048` |
| `scripts/cross-arch-provenance-governed-analyze.py` | `fb7ee1f5af85dcefa721283dcbb3e2cfc67ae2b8212c0f4578fb7c30a1d989b8` |
| `scripts/cross-arch-provenance-arm.sh` | `ceee31e545c02a1f44b47ebe0957154641609bd28a78bc646236d480905b955a` |
| `scripts/cross-arch-provenance-governed.sh` | `fc5f29c80d9678cdac415c83be37e7cfdb5f3769a10ca836c6d62da1ecc3c35a` |
| `eval/scripts/test_crossarch_governed.py` | `7667609c436e2a349a5deed5b83637dd5e854400a7b060f3c16a3fc60a036df7` |
| `docs/superpowers/preregistration/PR-06-publication-decision-rule.md` | `e3e098c53d791c9a3ad6de8fd21802965b9eb8ac0237b19c9fa59308233bef21` |

The manifest's `files` map is the **executed/final-run seal** at commit
`56b109c1`. The post-run hardened analysis/arm/orchestrator/test hashes are in
`post_run_hardening`; every scientific corpus/workflow/PR-06/MANIFEST hash is
unchanged from the freeze-time manifest.

| Post-run hardened file | SHA-256 |
| :--- | :--- |
| `scripts/cross-arch-provenance-governed-analyze.py` | `e31bc0e0443ec4789381341833755b3aaaaefc8c60810bbe3ff8f3ea81785df8` |
| `scripts/cross-arch-provenance-arm.sh` | `8647a0c303f1208c05a2985f9623a56ab8bb1086f3c4088dfcb2949faa298ce3` |
| `scripts/cross-arch-provenance-governed.sh` | `634dd6a7722de1d72ce36e3eef42f5958b5f83b797891f2e1a6d03a3812a796d` |
| `eval/scripts/test_crossarch_governed.py` | `6441be427ab612b9f18571a419f8669a00ff1e90a885e628da93e8b13f712137` |

The hardened analysis re-derives the same **POSITIVE** result on the committed
evidence; its output is the current `crossarch-results.json` (SHA-256
`5dd4ce2813a8059a319983437932a63552f23bfbfe549f850cc5b75fcd988407`).

### 2.4 Pre-data harness revision

The first staging attempt stopped before any workflow artifact existed:
Docker Desktop 4.94 crashed while the arm harness ran `docker rmi` on the
digest-only reference. The identity-only attempt is preserved under
`docs/superpowers/audits/data/crossarch-1397/attempt-1-docker-stopped/`.
A pre-data harness revision added a `CROSSARCH_STAGE=0` pre-staged image path
with a fail-closed architecture check, and the orchestrator now pre-stages
both images without `docker rmi`. No threshold, endpoint, corpus, class,
denominator, environment identity, or negative-control value changed; the
same manifest-list digest remains the lock. The amendment log records this as
entry 3.

### 2.5 Pre-analysis / pre-final-run harness revision 2

The first complete two-arm run (attempt-2, 16:59:18Z) produced intact
checkpoints and outputs, but the harness named the per-workflow evidence files
by the `.oxoflow` file basename (`cross-arch-provenance-governed-a`) instead
of the frozen workflow name (`crossarch-governed-a`), so the analysis could
not pair the arms. No cross-arm comparison was made or used. The evidence is
preserved under
`docs/superpowers/audits/data/crossarch-1397/attempt-2-stem-mismatch/`. The
stem-fix commit `56b109c1` (17:00:43Z) was committed after attempt-2 and
before the final analyzable run; the arm script now derives the stem from the
workflow's declared `[workflow] name`, matching the frozen corpus keys. No
threshold, endpoint, corpus, class, denominator, environment identity, or
analysis script changed. The amendment log records this as entry 4.

## 3. Corpus

Two deterministic workflows, 12 primary declared artifacts each, in eight
classes of three. Every rule executes inside the same pinned image; every
shell sets `LC_ALL=C`; compressed outputs use `gzip -n`; there is no
timestamp, no `/dev/urandom`, and no network access.

| Class | Artifacts (workflow-relative) |
| :--- | :--- |
| pure-text | `data/payload.txt`, `data/alphabet.txt`, `data/notes.txt` |
| raw-binary | `data/bytes.bin`, `data/pattern.bin`, `data/mask.bin` |
| derived-text | `summary.txt`, `sorted.txt`, `dedup.txt` |
| byte-exact-compressed | `data/payload.txt.gz`, `data/bytes.bin.gz`, `data/pattern.bin.gz` |
| checksum-manifest | `manifest.sha256`, `digests.tsv`, `checksums.txt` |
| ieee754-float | `float.txt`, `float_series.txt`, `float_fsum.csv` |
| integer-arithmetic | `factorial.txt`, `primes.txt`, `bigint.txt` |
| json-structured | `data.json`, `summary.json`, `matrix.json` |

Workflow A (`crossarch-governed-a`) produces the first four classes plus the
excluded negative control `control/os_arch.txt`; workflow B
(`crossarch-governed-b`) produces the last four classes.

## 4. Exact commands

```bash
# Parameter-freeze commit (before any run):
git show -s --format='%H %cI' dd5dcd44ee91a2c93bfd24c29f189d8de32e87d5

# Harness-revision / engine commit used by both arms:
git show -s --format='%H %cI' 56b109c17b95e4c610a874aaf63fc0ae583a57a6

# Two-host campaign (from the 56b109c1 harness-revision checkout on the arm64
# host; the dd5dcd44 parameter-freeze commit is an ancestor, and the
# orchestrator's local-HEAD check requires the run commit):
bash scripts/cross-arch-provenance-governed.sh \
  docs/superpowers/audits/data/crossarch-1397 \
  --repo-root "$PWD" \
  --remote-host tx-ubuntu \
  --remote-repo /home/ubuntu/crossarch-1397-src \
  --expected-commit 56b109c17b95e4c610a874aaf63fc0ae583a57a6

# The orchestrator runs, per arm:
#   bash scripts/cross-arch-provenance-arm.sh <platform> <outdir> <repo> <bin> <workflow...>
# and then:
#   python3 -I scripts/cross-arch-provenance-governed-analyze.py \
#     --corpus scripts/cross-arch-provenance-governed-corpus.json \
#     --preregistration docs/superpowers/audits/data/crossarch-1397/preregistration.json \
#     --repo-root "$PWD" \
#     --arm-a <local arm> --arm-b <remote arm> \
#     --expected-commit 56b109c17b95e4c610a874aaf63fc0ae583a57a6 \
#     --out <outroot>/crossarch-results.json
```

## 5. Hosts and arms

| Field | arm A (aarch64) | arm B (x86_64) |
| :--- | :--- | :--- |
| host | `ShixiangWang-WorkSpacedeMacBook-Pro.local` | `VM-4-4-ubuntu` (tx-ubuntu) |
| `uname` | Darwin 27.2.0, `root:xnu-13432.41.4~25/RELEASE_ARM64_T8142 arm64` | Linux 6.8.0-137-generic, `x86_64` |
| CPU | Apple M5 (`host-supplement.txt`, captured post-run) | Intel Xeon Platinum 8255C (`host-supplement.txt`, captured post-run; the run's `identity.txt` `cpu-model` field is `unknown` because its Linux awk probe printed nothing) |
| nproc | 10 | 4 |
| execution | native aarch64 silicon; Docker Desktop 4.94.0 (241994) native `linux/arm64` VM (`host-supplement.txt`); no qemu; Docker server 29.8.2 (`host-supplement.txt`) | native x86_64; Docker server 29.7.2 (`host-supplement.txt`) |
| oxo-flow | 0.32.0 | 0.32.0 |
| binary SHA-256 | `9ce0065a42e60afe17dcb3308a9e430813301ea8e01bbf2964a75d14719236d6` | `aef9eecc49a09d177bcdbe102f309f64891935f2f18858fd441193c009b0cefd` |
| engine commit (`workflow_git_sha`) | `56b109c17b95e4c610a874aaf63fc0ae583a57a6` | `56b109c17b95e4c610a874aaf63fc0ae583a57a6` |
| `workflow_git_dirty` | `false` | `false` |
| run date (UTC) | `2026-11-27T17:01:06Z` | `2026-11-27T17:01:11Z` |

The engine commit `56b109c17b95e4c610a874aaf63fc0ae583a57a6` is the
harness-revision commit; the campaign parameters were frozen two commits
earlier at `dd5dcd44ee91a2c93bfd24c29f189d8de32e87d5`. The Rust source is
identical between those commits (they changed only the harness and manifest
text), and both arms built their CLI from the same tree. The run-time
`identity.txt` did not capture `cpu-model` or the Docker version; the
post-run `host-supplement.txt` files record them and are cited here as
supplementary evidence, not as part of the sealed run-time identity.

## 6. Environment lock

| Check | arm A | arm B | Verdict |
| :--- | :--- | :--- | :--- |
| manifest-list digest | `sha256:5e2bdb1b0d542d5039cffb8181c65930093949194a65c365dfb1b0fe13a4fa5b` | `sha256:5e2bdb1b0d542d5039cffb8181c65930093949194a65c365dfb1b0fe13a4fa5b` | **equal** (the lock) |
| platform | `linux/arm64` | `linux/amd64` | differs as designed |
| container arch | `aarch64` | `x86_64` | differs as designed |
| image arch | `arm64` | `amd64` | differs as designed |
| per-platform image id | `sha256:5e2bdb1b0d542d5039cffb8181c65930093949194a65c365dfb1b0fe13a4fa5b` | `sha256:78976a69c067520ef8be34cde307ef9f6fea6dcefa50edb57039773d241ca0db` | expected to differ (one manifest list, per-platform layer sets); Docker Desktop containerd reports the digest as the image id |
| python3 | `Python 3.12.16` | `Python 3.12.16` | equal |
| sort | `sort (GNU coreutils) 9.7` | `sort (GNU coreutils) 9.7` | equal |
| wc | `wc (GNU coreutils) 9.7` (from `container-supplement.txt`) | `wc (GNU coreutils) 9.7` (from `container-supplement.txt`) | equal; the sealed run-time `container.txt` omitted `wc` |
| gzip | `gzip 1.13` | `gzip 1.13` | equal |
| sha256sum | `sha256sum (GNU coreutils) 9.7` | `sha256sum (GNU coreutils) 9.7` | equal |
| glibc | `ldd (Debian GLIBC 2.41-12+deb13u4) 2.41` | `ldd (Debian GLIBC 2.41-12+deb13u4) 2.41` | equal |
| engine commit | `56b109c17b95e4c610a874aaf63fc0ae583a57a6` | `56b109c17b95e4c610a874aaf63fc0ae583a57a6` | **equal** |
| workflow content hash A | `sha256:8daa37215b0a5871253094d23cbe32035d21737c9a5fa097f63bbd5587e12847` | `sha256:8daa37215b0a5871253094d23cbe32035d21737c9a5fa097f63bbd5587e12847` | **equal** |
| workflow content hash B | `sha256:8e6d1cf56939df49e2e760cf68249a85e32e53569b3bc9dd50043dc0fcaa8048` | `sha256:8e6d1cf56939df49e2e760cf68249a85e32e53569b3bc9dd50043dc0fcaa8048` | **equal** |
| host-arch → container-arch | `arm64` → `aarch64` | `x86_64` → `x86_64` | **native on each host** (no emulation) |
| image-src | `python@sha256:5e2bdb1b…4fa5b` | `python@sha256:5e2bdb1b…4fa5b` | **equal** to the corpus image block |
| staged-tag | `crossarch1397:pin` | `crossarch1397:pin` | **equal** to the corpus image block |
| env-lock identity, workflow A `prepare` | `docker\|crossarch1397@sha256:5e2b…` | `docker\|python@sha256:5e2b…` | raw prefixes differ; **normalized `@sha256` digest equal** |
| env-lock identity, workflow A `derive` | `docker\|crossarch1397@sha256:5e2b…` | `docker\|python@sha256:5e2b…` | raw prefixes differ; **normalized `@sha256` digest equal** |
| env-lock identity, workflow B (4 rules) | `docker\|crossarch1397@sha256:5e2b…` | `docker\|python@sha256:5e2b…` | raw prefixes differ; **normalized `@sha256` digest equal** |
| `workflow_git_dirty` | false | false | **clean** |

The validity check reports **no environment-lock mismatch**. The local engine
resolved the staged tag to repository prefix `crossarch1397`; the remote engine
resolved the pulled digest ref to `python`. Both carry the same trailing
`@sha256:5e2bdb1b…4fa5b` digest, and the hardened analysis compares the parsed
digest per rule (6/6 rules equal) rather than the raw prefix. The `wc` row is
supplied by the post-run `container-supplement.txt` because the sealed
run-time `container.txt` omitted it; the supplement was captured from the same
pinned digest and is labeled post-run. The analysis also asserts the sealed
corpus/workflow/PR-06/MANIFEST hashes and the host-arch↔container-arch match
(`manifest_checked: true` in `crossarch-results.json`). Each arm's
`provenance verify --json` re-verified its own checkpoint:
workflow A matched 13/13 declared outputs, workflow B matched 12/12, with
0 mismatched and 0 missing on both arms. The independent artifact hashes
agree with every recorded checkpoint checksum (the analysis rejects a
campaign whose recorded and independently re-hashed values differ). The
recorded workflow content hashes equal the frozen corpus file hashes in
§2.3 (`sha256:8daa3721…` for workflow A and `sha256:8e6d1cf5…` for workflow
B), so the executed bytes are the sealed bytes.

## 7. Per-artifact result

| Artifact | Class | arm A (aarch64) SHA-256 | arm B (x86_64) SHA-256 | Verdict |
| :--- | :--- | :--- | :--- | :--- |
| `crossarch-governed-a/data/payload.txt` | pure-text | `c3d522d442fe9b8e08ea1cc225886a96b7c6f2520e3ed49ce616912e8de61440` | `c3d522d442fe9b8e08ea1cc225886a96b7c6f2520e3ed49ce616912e8de61440` | **identical** |
| `crossarch-governed-a/data/alphabet.txt` | pure-text | `9e593b90f6fd0ef5636102094a488600be03a4913ea6e08836a5c924401b66c7` | `9e593b90f6fd0ef5636102094a488600be03a4913ea6e08836a5c924401b66c7` | **identical** |
| `crossarch-governed-a/data/notes.txt` | pure-text | `64c9a93dcb7c3a6da918cbf728e3a2ae6745760625e132b99eea6047cb0e5800` | `64c9a93dcb7c3a6da918cbf728e3a2ae6745760625e132b99eea6047cb0e5800` | **identical** |
| `crossarch-governed-a/data/bytes.bin` | raw-binary | `103066039ffcce85ebb793b744cae7e0017cd660c2bb6dff327ecbfe55212f75` | `103066039ffcce85ebb793b744cae7e0017cd660c2bb6dff327ecbfe55212f75` | **identical** |
| `crossarch-governed-a/data/pattern.bin` | raw-binary | `0dd21bd42470a439e676fa1e6e4706bb7909712910d854e229671a51a26a3941` | `0dd21bd42470a439e676fa1e6e4706bb7909712910d854e229671a51a26a3941` | **identical** |
| `crossarch-governed-a/data/mask.bin` | raw-binary | `9c478d59f2a43f86656e4a15b12fbfa14d0f9026bfba48f92e0b5a9e620d1a15` | `9c478d59f2a43f86656e4a15b12fbfa14d0f9026bfba48f92e0b5a9e620d1a15` | **identical** |
| `crossarch-governed-a/summary.txt` | derived-text | `83d625670eed3424f9db4e517856744778dba9c5d40155cb84152fcd4d4d398a` | `83d625670eed3424f9db4e517856744778dba9c5d40155cb84152fcd4d4d398a` | **identical** |
| `crossarch-governed-a/sorted.txt` | derived-text | `aa7d83943d7ebf9dfb0f0d0e0ad57a231efadf5fcf481935cb85eec3b9fbe80c` | `aa7d83943d7ebf9dfb0f0d0e0ad57a231efadf5fcf481935cb85eec3b9fbe80c` | **identical** |
| `crossarch-governed-a/dedup.txt` | derived-text | `9932a2cc85d679f070afd979ccd9640daf4c46c9bd99e579d9a0e012b7e40f94` | `9932a2cc85d679f070afd979ccd9640daf4c46c9bd99e579d9a0e012b7e40f94` | **identical** |
| `crossarch-governed-a/data/payload.txt.gz` | byte-exact-compressed | `4f8c577971a39ab66656127c2e228a35afad3cf185facfba1cf669f9d96e68ff` | `4f8c577971a39ab66656127c2e228a35afad3cf185facfba1cf669f9d96e68ff` | **identical** |
| `crossarch-governed-a/data/bytes.bin.gz` | byte-exact-compressed | `62de65398cfb8c3a523eeb8707ae1b65c5318003328d08d408db5f996ab1b536` | `62de65398cfb8c3a523eeb8707ae1b65c5318003328d08d408db5f996ab1b536` | **identical** |
| `crossarch-governed-a/data/pattern.bin.gz` | byte-exact-compressed | `ff3c7d2f916148f28ec0bef862c8969f777b749b3c99a3083f11de4a4b20a397` | `ff3c7d2f916148f28ec0bef862c8969f777b749b3c99a3083f11de4a4b20a397` | **identical** |
| `crossarch-governed-b/manifest.sha256` | checksum-manifest | `06a1fffa11a94b58b0702f75eb12c154a0f54d0c5659f47f53fc8bd2cfd8ff01` | `06a1fffa11a94b58b0702f75eb12c154a0f54d0c5659f47f53fc8bd2cfd8ff01` | **identical** |
| `crossarch-governed-b/digests.tsv` | checksum-manifest | `16db5c58f158af791b00d823662d4d9c08529e7028aaf27b937a35db4dce1ee2` | `16db5c58f158af791b00d823662d4d9c08529e7028aaf27b937a35db4dce1ee2` | **identical** |
| `crossarch-governed-b/checksums.txt` | checksum-manifest | `bd9115c27eba63ea00e2aa6c8cfaa4ecb201c8d39139f65953d6b239eb7bddc6` | `bd9115c27eba63ea00e2aa6c8cfaa4ecb201c8d39139f65953d6b239eb7bddc6` | **identical** |
| `crossarch-governed-b/float.txt` | ieee754-float | `71d665e59199296a7ad04a87a6dec9a1e796263eb29384bb179f65f932423dd6` | `71d665e59199296a7ad04a87a6dec9a1e796263eb29384bb179f65f932423dd6` | **identical** |
| `crossarch-governed-b/float_series.txt` | ieee754-float | `9633031545425a1574fd1952b8f4dd36ad75a135aaa2baa8e6d4a0e1c7d54ee3` | `9633031545425a1574fd1952b8f4dd36ad75a135aaa2baa8e6d4a0e1c7d54ee3` | **identical** |
| `crossarch-governed-b/float_fsum.csv` | ieee754-float | `f49e8e422da86bdf1dba1b4ac9e66dc5b911ef0b9c6f9962d79b53056898ebaa` | `f49e8e422da86bdf1dba1b4ac9e66dc5b911ef0b9c6f9962d79b53056898ebaa` | **identical** |
| `crossarch-governed-b/factorial.txt` | integer-arithmetic | `dca230c95c8aa7362ef2ee4de386ab3bc5306a146068a6971bc9bd0c5b27a9b0` | `dca230c95c8aa7362ef2ee4de386ab3bc5306a146068a6971bc9bd0c5b27a9b0` | **identical** |
| `crossarch-governed-b/primes.txt` | integer-arithmetic | `80eb224ba892fac76623089f2b5d4921466627afd5d5aae207573c4ca2ddd61a` | `80eb224ba892fac76623089f2b5d4921466627afd5d5aae207573c4ca2ddd61a` | **identical** |
| `crossarch-governed-b/bigint.txt` | integer-arithmetic | `f37bc9fc6aea0616f9a29eae674d2f338303bae5b7e945402e7ef4b96d2be4c9` | `f37bc9fc6aea0616f9a29eae674d2f338303bae5b7e945402e7ef4b96d2be4c9` | **identical** |
| `crossarch-governed-b/data.json` | json-structured | `1a71d62870e6fcbe16d3627157a13efe702c0daf722c9736f402cdace64cbd9d` | `1a71d62870e6fcbe16d3627157a13efe702c0daf722c9736f402cdace64cbd9d` | **identical** |
| `crossarch-governed-b/summary.json` | json-structured | `bad1cbcd3a5d48ad9dc851bfe161504cc036f7e53b3da6681b3dfcefc0ddd156` | `bad1cbcd3a5d48ad9dc851bfe161504cc036f7e53b3da6681b3dfcefc0ddd156` | **identical** |
| `crossarch-governed-b/matrix.json` | json-structured | `ef1fe569b7215a3517f7a444cbfe4c5079533d742ad998f40245e2800f2f0873` | `ef1fe569b7215a3517f7a444cbfe4c5079533d742ad998f40245e2800f2f0873` | **identical** |

Full digests above are the independent `artifacts.sha256` values,
which the analysis cross-checks against each checkpoint's recorded checksums.
The differing set is **empty**.

## 8. Negative control

| Field | Value |
| :--- | :--- |
| artifact | `crossarch-governed-a/control/os_arch.txt` |
| producer | `uname -m` inside the pinned container |
| arm A content | `aarch64` |
| arm A SHA-256 | `5121ae59314a84da7b4d9587374398f1afa2000b243b3e54f491adefaac54f92` |
| arm B content | `x86_64` |
| arm B SHA-256 | `aaf631698ae5160ceb04a97681a14887fdcab47cd6e0f163c87485b3b1340b62` |
| verdict | **differs** (as required) |

The negative control is a declared output but **not** a primary artifact and
has no class. It was excluded from the primary endpoint and every class
statement. Its differing hash demonstrates that the comparison can detect a
real architecture-dependent difference; the frozen rule makes the campaign
INVALID if this control is identical (a harness failure, never a stable
artifact). The two hashes are exactly the SHA-256 of the literal
`aarch64\n` and `x86_64\n` strings, confirming the recorded content.

## 9. Exact interval and governed category

- Primary endpoint: architecture-stable artifact proportion, **N = 24**,
  **k = 24** identical, point estimate **100.0%**.
- Exact Clopper-Pearson two-sided 95% CI: **[0.857526, 1.000000]**.
- Frozen threshold `tau_ARCH = 0.80`; the exact lower bound 0.857526 is at or
  above the threshold, so the interval partition (PR-06 §0.1 step 4 / §C4
  step 3) decides **POSITIVE**. No eligible class contains a differing
  artifact, so the NEGATIVE precedence does not apply.
- Point estimate: 100.0%. No p-value is computed; the question is estimation,
  not significance.

**Filled positive statement (PR-06 §C4).**

> On the frozen 2-workflow corpus of 24 declared artifacts with identical container digest sha256:5e2bdb1b0d542d5039cffb8181c65930093949194a65c365dfb1b0fe13a4fa5b and matching environment locks, all 24 artifacts were SHA-256 identical across native x86_64 and native aarch64 (100.0%, exact 95% CI [85.75%, 100.00%]), meeting the pre-registered tau_ARCH = 0.80. The claim is limited to these 2 workflows, these tool versions, and these 8 artifact classes; it does not establish bitwise reproducibility for arbitrary tools or workflows.

## 10. Class-level verdicts

| Class | Artifacts | Identical | Eligible (n_min = 3) | Architecture-stable? |
| :--- | :--- | :--- | :--- | :--- |
| pure-text | 3 | 3 | yes | **yes** |
| raw-binary | 3 | 3 | yes | **yes** |
| derived-text | 3 | 3 | yes | **yes** |
| byte-exact-compressed | 3 | 3 | yes | **yes** |
| checksum-manifest | 3 | 3 | yes | **yes** |
| ieee754-float | 3 | 3 | yes | **yes** |
| integer-arithmetic | 3 | 3 | yes | **yes** |
| json-structured | 3 | 3 | yes | **yes** |

All eight classes are eligible (`n_min = 3`, three artifacts each) and
**every artifact in every class is SHA-256 identical**, so all eight classes
are architecture-stable. No class-level NEGATIVE precedence applies.

## 11. Flow table

| Flow item | Count / value |
| :--- | :--- |
| primary artifacts included (both arms) | 24 |
| negative controls excluded from the primary endpoint | 1 |
| missing primary artifacts | 0 |
| unexpected/undeclared artifacts | 0 |
| in-arm provenance verify mismatches | 0 |
| validity conditions failed | 0 |
| arms completed | 2 / 2 |
| workflows completed | 4 / 4 (2 per arm) |
| rules completed | 12 / 12 (workflow A: 2 per arm; workflow B: 4 per arm; 6 per arm) |

The `unexpected/undeclared artifacts` row is scoped to **checkpoint-declared
outputs**. Workflow B intentionally writes `scratch_a.bin`, `scratch_b.bin`,
and `scratch_c.txt` as internal inputs for its checksum manifests; they are
not declared outputs, PR-02 §C4 defines non-declared files as non-artifacts,
and they are excluded from the provenance denominator.

## 12. Limitations

- **Bounded corpus.** Two workflows, 24 declared artifacts, eight classes.
  The result does not establish bitwise reproducibility for arbitrary tools,
  workflows, or artifact types.
- **One host pair, one image digest.** The arm64 leg is Docker Desktop's
  native Linux VM on Apple Silicon, not bare-metal Linux; the x86_64 leg is a
  Linux VM. Both execute the container natively on their host CPU (no qemu),
  but host virtualization and the two host kernels differ. The executed
  environment is the pinned Linux container, which is the axis the
  pre-registration controls; the host kernel is a residual confounder the
  artifacts do not expose.
- **Toolchain held constant.** Both arms share one image digest and identical
  in-container tool versions. The environment-lock matrix with *differing*
  tool versions across architectures is unmeasured.
- **No independent reviewer sign-off.** PR-05 C4 remains PENDING. Per PR-05,
  this is a governed decision under the frozen rule, not a publication-ready
  confirmatory result. No independent replication or external review exists.
- **No third architecture.** RISC-V and other architectures are out of scope.
- **Only the `docker` backend.** `system`, `singularity`, `conda`, `pixi`,
  and the cluster/scheduler path remain out of scope.
- **Harness history disclosed.** The first staging attempt stopped on a
  Docker Desktop `docker rmi` crash (identity only, no workflow artifact);
  the first complete two-arm run had a stem-naming defect and was never
  analyzed as the campaign result. Both are preserved under `attempt-1-*`
  and `attempt-2-*`. The final run used the corrected harness and is the
  result reported here.
- **Host identity fields.** The run-time `identity.txt` did not capture
  `cpu-model` (the Linux awk probe printed nothing) or the Docker version.
  The post-run `host-supplement.txt` files record Apple M5 / Docker 29.8.2 and
  Intel Xeon 8255C / Docker 29.7.2; they are supplementary evidence, not part
  of the sealed run-time identity.
- **Supplementary `wc`.** The sealed `container.txt` omitted `wc`; the
  post-run `container-supplement.txt` records `wc (GNU coreutils) 9.7` on both
  arms from the same digest. The hardened analysis compares it and labels the
  source.
- **Manifest lineage.** The parameter-freeze manifest (`0eaca24b`, commit
  `dd5dcd44`) and the final-run manifest (`770ae8f2`, commit `56b109c1`) carry
  the same scientific hashes. Attempt-2 produced checkpoint/output evidence
  but was not analyzed as the campaign result; both the interrupted staging
  attempt and attempt-2 are preserved and disclosed. The current file is the
  post-run review-hardened revision (`3082b228`).
- **Native-execution assertion.** The hardened analysis requires each arm's
  normalized host architecture to equal its container architecture; an
  emulated arm on an x86 host would now be INVALID, not silently accepted.
- **Per-platform image ids differ** because one manifest list resolves to
  per-platform layer sets. Identity is the shared manifest-list digest; the
  per-platform ids are recorded, not treated as a lock.

The primary claim is limited to this frozen corpus, these tool versions,
these container digests, and these eight classes. It is not a claim about
all bioinformatics tools or hardware.

## 13. Raw-data lineage

- Final-run / review-hardened manifest: `docs/superpowers/audits/data/crossarch-1397/preregistration.json`
  (freeze-time `0eaca24b…`, final-run `770ae8f2…`, post-run hardened `3082b228…`).
- Frozen corpus: `scripts/cross-arch-provenance-governed-corpus.json`,
  `scripts/cross-arch-provenance-governed-a.oxoflow`,
  `scripts/cross-arch-provenance-governed-b.oxoflow`.
- arm A (native aarch64): `docs/superpowers/audits/data/crossarch-1397/arm-local-arm64/`
  (`identity.txt`, `container.txt`, `host-supplement.txt`,
  `container-supplement.txt`, and per-workflow
  `{checkpoint.json,verify.json,artifacts.sha256,rows.tsv}`).
- arm B (native x86_64): `docs/superpowers/audits/data/crossarch-1397/arm-remote-amd64/`
  (same file set).
- Governed analysis output: `docs/superpowers/audits/data/crossarch-1397/crossarch-results.json`.
- Orchestrator log: `docs/superpowers/audits/data/crossarch-1397/campaign.log`.
- Interrupted staging attempt (identity only): `docs/superpowers/audits/data/crossarch-1397/attempt-1-docker-stopped/`.
- First complete run with the stem-naming defect (not analyzed as the
  campaign result): `docs/superpowers/audits/data/crossarch-1397/attempt-2-stem-mismatch/`.
- Governance: `docs/superpowers/preregistration/AMENDMENTS.md` entries 2–6;
  `docs/superpowers/preregistration/MANIFEST.sha256`.

The raw per-artifact digests are the `artifacts.sha256` files in each arm
directory; the checkpoints retain the engine-recorded values; the analysis
script cross-checks the two before deciding. The governed results JSON has
SHA-256 `5dd4ce2813a8059a319983437932a63552f23bfbfe549f850cc5b75fcd988407`;
the orchestrator log has SHA-256
`79f9e26befa195acf75b6c2f639d93cf7f608d7497c05f8d0a47725a4ed47849`.

**Retention / cleanup.** The hardened orchestrator removes the remote raw
evidence directory and the local/remote `work/` artifact dirs after copying
and analyzing the sealed evidence; `--keep-remote` and `--keep-work` retain
them. The remote SSH checkout (`<remote-repo>`) is retained for reproduction.
The committed evidence files (checkpoints, verify JSON, independent hashes,
identity/container records, supplements, analysis JSON, log) are the retained
raw record.

## 14. Reproduction

```bash
# 1. Check out the engine commit used by both arms (or the campaign branch).
git checkout 56b109c17b95e4c610a874aaf63fc0ae583a57a6

# 2. Build the CLI on each host from that commit.
cargo build -p oxo-flow-cli

# 3. Pre-stage the pinned multi-arch image on each host (no `docker rmi`):
#    local aarch64 host:
docker pull --platform linux/arm64 python@sha256:5e2bdb1b0d542d5039cffb8181c65930093949194a65c365dfb1b0fe13a4fa5b
docker tag python@sha256:5e2bdb1b0d542d5039cffb8181c65930093949194a65c365dfb1b0fe13a4fa5b crossarch1397:pin
#    remote x86_64 host (tx-ubuntu):
docker pull --platform linux/amd64 python@sha256:5e2bdb1b0d542d5039cffb8181c65930093949194a65c365dfb1b0fe13a4fa5b
docker tag python@sha256:5e2bdb1b0d542d5039cffb8181c65930093949194a65c365dfb1b0fe13a4fa5b crossarch1397:pin

# 4. Run both arms and the governed analysis.
bash scripts/cross-arch-provenance-governed.sh \
  docs/superpowers/audits/data/crossarch-1397 \
  --repo-root "$PWD" \
  --remote-host tx-ubuntu \
  --remote-repo /home/ubuntu/crossarch-1397-src \
  --expected-commit 56b109c17b95e4c610a874aaf63fc0ae583a57a6

# 5. Re-run only the analysis on the committed evidence:
python3 -I scripts/cross-arch-provenance-governed-analyze.py \
  --corpus scripts/cross-arch-provenance-governed-corpus.json \
  --preregistration docs/superpowers/audits/data/crossarch-1397/preregistration.json \
  --repo-root "$PWD" \
  --arm-a docs/superpowers/audits/data/crossarch-1397/arm-local-arm64 \
  --arm-b docs/superpowers/audits/data/crossarch-1397/arm-remote-amd64 \
  --expected-commit 56b109c17b95e4c610a874aaf63fc0ae583a57a6 \
  --out /tmp/crossarch-results.json
```

The orchestrator records its exact commands and timestamps in
`campaign.log`; the arm harness and analysis are stdlib/POSIX shell and
do not require cloud or paid services. By default the orchestrator removes
the remote raw directory and the local/remote `work/` artifact dirs after the
sealed evidence is copied and analyzed; `--keep-remote` and `--keep-work`
retain them, and the remote SSH checkout is retained for reproduction.
