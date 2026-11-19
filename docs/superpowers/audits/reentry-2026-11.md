# Hybrid re-entry + `[[values]]` expansion — cohort replay and crash matrix (#1245)

> **Status**: measured evidence for ROADMAP Stage 12.2, "Hybrid re-entry and
> `[[values]]` expansion". Two independent proofs of the same contract live
> here: (1) a **cohort-scale deterministic replay** (cold vs resume), and (2)
> an **adversarial crash/interruption matrix** (SIGTERM, SIGKILL, truncated
> checkpoint, stale/unknown engine-version). Every crash case is checked to
> **fail safe** — the resumed run either refuses with a clear diagnostic, or
> re-runs the affected instance; it **never** silently skips a rule that did not
> actually complete. PR references issue #1245. It **Closes** only the
> cohort-replay + crash-matrix exit evidence; the boundaries in §7 stay open.

## 1. What was measured

Checkpoint re-entry (issue #78 P3) lets a `checkpoint = true` rule write a TOML
re-entry manifest at run time; the engine merges new samples / pairs /
`[[values]]` axes and re-expands every template deterministically
(`MAX_REENTRY_ROUNDS = 32`). This audit exercises that path over a synthetic
cohort **built deterministically** (fixed sample numbering and
`[[values]]` strata, sorted iteration, no `HashMap`-order dependence — the
"Persisted artifacts are order-independent" invariant) and asserts three
things:

1. **COLD vs RESUME instance-identity set is identical** — the resumed run
   reconstructs exactly the set the cold run produced (or the difference is
   explained/bounded with a precise reason; none was needed here).
2. **No drop, no duplicate** — the resumed set equals the cold set exactly (as
   sets; both are order-independent `BTreeSet`s).
3. **Repeated resume is stable** — a second resume yields the same set again.

The identity surface is the instance name in `dry-run --json`
(`plan[].name`, sorted, de-duplicated), corroborated by `checkpoint.json`
`completed_rules`.

Then an adversarial matrix drives the **real CLI binary** through abnormal
stops and asserts fail-safe resumption (§5).

## 2. Cohort fixture parameters

The generator is `scripts/reentry_cohort_replay.py` (Python; report-only, always
exits 0) with a thin wrapper `scripts/reentry-cohort-replay.sh`. The same
contract is pinned fast in CI by `tests/reentry_cohort_replay.rs` (N=12, M=3).

| Parameter | Value |
| :--- | :--- |
| samples (`[[sample_groups]]`) | N (measured N=200; CI N=12) |
| `[[values]]` axis | `stratum`, M values (measured M=3: `s00`,`s01`,`s02`; CI `sA`,`sB`,`sC`) |
| rules | `discover` (producer, `checkpoint = true`) → `stage1` → `stage2` (2 fan-out rules deep) |
| re-entry contribution | the `discover` manifest adds **1 new sample** (`SNEW`) **and 1 new stratum** (`s03`/`sD`) |
| cold instance count | `(N+1)·(M+1)·2 + 1` = `201·4·2 + 1` = **1609** (measured N=200) |
| CI instance count | `(12+1)·(3+1)·2 + 1` = **105** |
| parallelism | `-j 4` |
| seed | none needed — sample/stratum ordering is fully deterministic |

Note the cold **plan** (a `dry-run --json` before any run) is the *static*
Cartesian product `N·M·2 + 1` because the re-entry axes are not known until the
`discover` rule runs; the run's **completed** set is the *grown* product
`(N+1)·(M+1)·2 + 1`. The resume must reproduce the grown set — that is the
whole point of the checkpoint manifest.

## 3. Per-arm identity (ledger §8)

| Field | Value |
| :--- | :--- |
| hostname | `shixiangwangdeiMac.local` |
| `uname -a` | `Darwin 25.6.0 … root:xnu-12377.161.14~5/RELEASE_ARM64_T8122 arm64` |
| arch | `arm64` |
| CPU | Apple M3, 8 CPUs (`hw.ncpu = 8`) |
| OS | macOS 26.6.2 (Build 25G83) |
| baseline commit | `2d355f6b84032a13b74ef7865e3f81460275a86f` (`origin/main` at branch point) |
| harness + tests | introduced in this PR's head on branch `research/1245-reentry-cohort-replay` |
| oxo-flow | `0.32.0` (`target/debug/oxo-flow`) |
| rustc / cargo | `1.98.0` / `1.98.0` |
| python3 | `3.12.4` |
| shell | `/bin/zsh` |
| date (UTC) | 2026-11-19 |

## 4. Exact commands

```bash
# from the worktree root, after `cargo build -p oxo-flow-cli`
scripts/reentry-cohort-replay.sh 200 3          # measured cohort (report-only)
cargo test --test reentry_cohort_replay         # fast CI proof (N=12)
cargo test --test reentry_crash_matrix          # adversarial crash matrix
```

## 5. Measured results

### 5.1 Cohort replay (cold vs resume)

| cohort | cold instances | cold wall | resume wall | identity set equal | plan==resume | resume stable | speedup |
| :--- | :--- | :--- | :--- | :--- | :--- | :--- | :--- |
| **N=200 × M=3** | 1609 | **166.95 s** | **0.59 s** | yes | yes | yes | **283.3×** |

> Wall-clock numbers are N=1 per cell on one host and vary with machine load: an independent re-run of the same harness reproduced every contract claim (instances 1609, identity/plan set equality, resume stability) with cold 251 s / resume 1.11 s (≈226×). The load-bearing claims are the set-equality and the ~2-orders-of-magnitude resume speedup; treat the absolute seconds as indicative only.
| N=50 × M=3 | 409 | 12.60 s | 0.16 s | yes | yes | yes | 78.8× |

Interpretation: the resume reconstructs the **identical** grown instance set
(cold `completed_rules` == resume `completed_rules` == `dry-run --json`
`plan[].name`), skips every already-completed rule (narrated "already
completed"), and is stable across repeated resumes. No instance is dropped or
duplicated.

### 5.2 Adversarial crash / interruption matrix

Each row is a `#[test]` in `tests/reentry_crash_matrix.rs`; all are
CI-exercisable and fast (full file ≈ 5.5 s). Verdict = the resumed run **fails
safe**.

| case | injection | observed behavior | resumed run | verdict |
| :--- | :--- | :--- | :--- | :--- |
| **SIGTERM mid-run** | `kill -TERM` while `slow` is in flight | exit **143**; in-flight rule recorded in `failed_rules` (signal 15), **not** in `completed_rules`; no completed output left | `run --resume-failed` clears `failed_rules` and **re-runs** `slow` to completion | **PASS** |
| **SIGKILL mid-run** | `kill -KILL` (no graceful flush) | no exit code (signal 9); `.oxo-flow/checkpoint.json` keeps the in-flight rule in `running` | resume warns "they died with it and will re-run", clears the stale `running` entry, and **re-runs** `slow` | **PASS** |
| **truncated / partial checkpoint** | truncate `checkpoint.json` to `len/3` (mid-file) | Execute `run` fails: names the corruption and prints **"refusing to start — every completed rule would silently re-run"** | — (fail-closed by design) | **PASS** |
| **truncated checkpoint (read-only preview)** | same corrupt file, `dry-run` | degrades with a warning ("corrupt … planning as if every rule never completed") | plan is the full set (nothing silently treated as done) | **PASS** |
| **unknown / older engine-version field** | inject `engine_version = "0.0.1-ancient"`, `schema_version = 1` | checkpoint loads (schema-additive; unknown JSON keys ignored); a genuinely-completed rule still skips ("already completed") | resume **skips** completed rules — correct, nothing was lost | **PASS** |
| **checkpoint missing a required field** | remove `benchmarks` | treated as **corrupt** → Execute refuses ("refusing to start") | — (fail-closed; never silently re-runs or silently skips) | **PASS** |

Key invariant confirmed: a checkpoint is only **skipped** when it genuinely
completed; every uncertain state (truncated, missing required field, stale
`running` from SIGKILL) either **refuses** or **re-runs** the affected
instance(s). No case silently skipped a rule that had not completed.

### 5.3 `NOT_VALIDATED` list

Cases the brief lists or that are adjacent but **cannot be exercised in CI**,
with the reason (also recorded as a comment block at the bottom of
`tests/reentry_crash_matrix.rs`):

- **Real power loss / kernel panic between the staged `*.tmp` write and the
  atomic rename.** The atomic-rename design (staged temp + fsync + rename +
  parent-dir fsync) is covered by code inspection and by the truncated-file
  case, but a true mid-rename power cut requires hardware fault injection, not a
  CI-runnable test.
- **A checkpoint written by a genuinely OLDER oxo-flow binary** (a Rust struct
  that lacks a field this build *requires*). Simulated here by removing the
  required `benchmarks` field, which proves the fail-closed path; a real
  cross-version binary matrix is out of scope for this campaign. Conversely, a
  checkpoint with *extra/unknown* fields is proven harmless (§5.2).
- **SIGTERM/SIGKILL arriving in the narrow window before signal handlers are
  installed** (between `exec` and `run`'s select loop). Covered in spirit by the
  SIGKILL case (which needs no handler) but not separately pinned.

## 6. Maturity assessment (ledger §1 vocabulary)

- **"Deterministic replay at cohort scale"** — the ROADMAP exit evidence for
  this row — is now **measured**: cold vs resume instance-identity sets are
  equal at N=200 × M=3 (1609 instances), repeated resume is stable, and the fast
  CI test pins the same contract at N=12.
- **"Crash/interruption tests"** — also listed exit evidence — are now
  **implemented and passing** for the CI-exercisable classes (SIGTERM, SIGKILL,
  truncated checkpoint, unknown/older engine-version, missing required field),
  each demonstrated **fail-safe**.
- The two remaining boundaries (real mid-rename power cut; a real cross-version
  binary matrix) are **explicitly NOT_VALIDATED** (§5.3) and are not claimed.

## 7. Honesty boundary

**What this is.** A measured, reproducible cohort replay plus an adversarial
crash matrix for the checkpoint-re-entry + `[[values]]` expansion path, driven
through the real CLI binary on the host in §3.

**What this is NOT, stated loudly.**

- **Not a container / cluster run.** Everything ran locally with `-j 4`; no
  container runtime, no scheduler, no real data — the rules are synthetic
  (`echo`/`sleep`). It proves re-entry *identity* and *crash safety*, not
  scientific output.
- **The cohort is synthetic and self-selected.** N=200 × M=3 is a scaling
  fixture, not a sample of real pipelines; it bounds the harness, not the
  ecosystem.
- **Some crash classes are NOT_VALIDATED** (§5.3): real power-cut mid-rename
  and a genuine cross-version binary matrix. Do not read "crash matrix PASS" as
  "every crash class proven".
- **No production re-entry change.** No fail-unsafe bug was found, so the
  production path is untouched; this PR adds tests + a harness + this doc only.
