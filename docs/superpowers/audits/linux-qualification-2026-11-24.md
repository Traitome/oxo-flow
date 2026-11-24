# Linux qualification wave (ledger §7 wave 6) — 2026-11-24

Tracked Linux qualification gate for issue #1169 on the designated `tx-ubuntu`
host. This page records the host identity, the commit, every gate result, and
the raw logs; the qualification is **Linux + source build + one scheduler
script-generation path**, not a claim about live cluster/cloud schedulers.

## Host identity (§8)

| Field | Value |
| --- | --- |
| Hostname | `VM-4-4-ubuntu` |
| OS / kernel | Ubuntu, `Linux 6.8.0-137-generic #137-Ubuntu SMP PREEMPT_DYNAMIC x86_64` |
| CPU | 4 vCPU (`nproc` = 4) |
| RAM | 3.6 GiB |
| Disk | 59 GiB root (≈15 GiB free at qualification time) |
| Rust | `cargo 1.99.1 (5f94df478 2026-08-27)` / `rustc 1.99.1 (58cf8f9c3 2026-10-13)` |
| Docker | 29.7.2 |
| Nextflow | 26.10.6 (build 15032) |
| Snakemake | 9.27.2 (venv) |
| gitleaks | 8.28.0 |
| Commit | `293761205` (`fix(web): make the Landlock run sandbox usable end-to-end (#1129)`) |

Raw `make ci` log (SHA-256 prefix `f8a3b4a4a79530e1`):
[`data/linux-qualification-make-ci-2026-11-24.log`](data/linux-qualification-make-ci-2026-11-24.log).

## `make ci` result — PASS

Built with `CARGO_BUILD_JOBS=2 CARGO_PROFILE_DEV_DEBUG=0
CARGO_PROFILE_TEST_DEBUG=0 CARGO_INCREMENTAL=0` (the host cannot hold a
default debug-info target tree). The suite was run twice: the first pass
stopped at the `secrets` gate only because `gitleaks` was not installed
(environment prerequisite, not a code failure); after installing gitleaks
8.28.0 the full suite completed with no `make: ***` line.

Per-gate evidence present in the raw log:

- `fmt --check`, `clippy --workspace --all-targets -D warnings`, `build --workspace` — clean
- `test` (workspace, `--test-threads=1`) — all binaries green, including the
  RB-01 two-tenant adversarial suite (`rb01_sandbox_adversarial`) on this
  Landlock-capable kernel
- drift gates — `schema-drift`, `skill-drift`, `version-check`
- `audit`, `deny`, `aws-legacy-tripwire`, `issue-id-tripwire`, `npmmirror-tripwire`
- `truthset-stamp` — stamp `b7a75ed` is an ancestor and contains all 54 cases
- `secrets` (gitleaks), `docs` (rustdoc `-D warnings`), `frontend-lint`
- `eval-tests`, `eval-lint`
- `provenance-check` — manifest well-formed, every measured report has provenance
- `reports-regen` — all deterministic generator outputs reproduce
- `extension-test` — VSIX packaged (`oxo-flow-0.32.0.vsix`)

## Headless VS Code integration gate — PASS (10/10)

`xvfb-run -a make extension-integration` with
`OXOFLOW_TEST_BIN=<repo>/target/debug/oxo-flow`: activation/commands, CodeLens,
language recognition, schema completions (root + `depends_on`), missing-input
diagnostic, validate-error anchoring, unsaved-buffer diagnosis, quick actions,
and hover docs — **10 passing**, extension host exit 0.

Raw log:
[`data/linux-qualification-extension-integration-2026-11-24.log`](data/linux-qualification-extension-integration-2026-11-24.log).

## End-to-end engine run with a real environment backend

Two-rule workflow, both rules `environment = { docker = "ubuntu:22.04" }`,
run with `--provenance` on this host:

```
$ oxo-flow run e2e.oxoflow --provenance -j 2
Done: 2 succeeded, 0 skipped, 0 failed
✓ 2 output files verified (8B total)
$ oxo-flow --json provenance verify .oxo-flow/checkpoint.json
matched 2  mismatched 0  missing 0
```

## Cluster submit-script generation (SLURM minimum)

```
$ oxo-flow cluster submit e2e.oxoflow -b slurm -o slurm-scripts
Done: 2 scripts written to slurm-scripts
```

Generated scripts carry `#SBATCH --chdir/--job-name/--cpus-per-task/--output/--error`
and run the rule under `env -i … bash -c '<command>'` (the RB-15 allowlist).

## Not qualified here (explicit)

- Live SLURM/PBS/SGE/LSF schedulers and **all** cloud batch backends
  (kubernetes/aws-batch/gcp-batch): no scheduler or cloud credentials on this
  host — tracked by #1158 and the per-backend maturity labels in
  `docs/guide/src/how-to/run-on-cluster.md`.
- Cross-architecture reproduction (#1166) — a different measurement.
- Desktop shell lifecycle (`make desktop-smoke`) — separate target/CI job.

## Re-run on the final released main (`6215a0f7`)

After the remaining PRs merged (attestation #1377, #1378 pruning fix #1380, web reliability
#1375), the full gate suite was re-run on `main` @ `6215a0f76` with the same host settings:

- `make ci` — **PASS** (no `make: ***` line; ends at the extension VSIX packaging step).
  Raw log: [`data/linux-qualification-make-ci-final-main-2026-11-24.log`](data/linux-qualification-make-ci-final-main-2026-11-24.log).
- `make extension-integration` under xvfb — **10 passing**, extension host exit 0.
  Raw log: [`data/linux-qualification-extension-integration-final-main-2026-11-24.log`](data/linux-qualification-extension-integration-final-main-2026-11-24.log).
- Signed attestation smoke on a real checkpoint (docker two-rule run from the section above):
  `provenance attest --sign --key <64-hex seed>` emitted `oxo-flow.attestation/v1` with
  workflow/content/resolved hashes, per-output checksums, environment identity and outcome;
  `attest-verify` returned `signature_status: "valid"` (exit 0), and a mutated document
  failed verification.

The qualification claims in this page therefore hold for the final main commit as well as
for `293761205`.

## Readiness statement

The source build, full gate suite, headless extension integration, a real
container-backed multi-rule run with provenance verification, and SLURM script
generation are qualified on this host at the recorded commit. Per ledger §8,
this does not constitute industrial-grade or production-ready status for the
unqualified backends above.
