# Final deployment qualification (tx-ubuntu) — 2026-11-26

Operational deployment matrix on the designated `tx-ubuntu` Linux host for
campaign main `93fa06c3` (oxo-flow 0.32.0). Every check below was executed
against the release binaries built from that commit. This is a single-host
operational qualification, not a production-scale or multi-instance campaign;
limits are listed at the end.

## Host and build

| Field | Value |
| --- | --- |
| Hostname | `VM-4-4-ubuntu` |
| OS / kernel | Ubuntu 24.04.4 LTS, Linux 6.8.0-137-generic x86_64 |
| CPU / RAM | 4 vCPU / 3.6 GiB |
| PostgreSQL | 16.15 (local) |
| Rust | rustup toolchain 1.98.0 (`rust-toolchain.toml`) |
| Build | `cargo build --release -p oxo-flow-cli -p oxo-flow-web --features oxo-flow-web/postgres`, `CARGO_BUILD_JOBS=1` |
| Build time | 16m56s |
| Commit | `93fa06c3` (`feat(reproducibility): workflow lockfile and run --locked (#1399)`) |
| CLI binary | 37,502,520 B |
| Web binary | 26,299,320 B |
| Frontend | Vite production build (`npm run build`), served via `OXO_FLOW_FRONTEND_DIR` |

The first parallel build was OOM-killed during two simultaneous LTO links;
the single-job rebuild succeeded. A fresh `~/oxo-campaign` checkout and
`~/oxo-build-target` cache were used.

The final main `make ci` gate was run on the Mac checkout of `93fa06c3`
(the tree also carried the `report.md` note included in this change). Its
first pass stopped at `spa_asset_drift` because the ignored local
`crates/oxo-flow-web/static/index.html` was newer than the asset files it
referenced (a stale partial bundle, not a source drift); `npm run build`
regenerated the bundle and the gate passed. The failure is the guard doing
its job — an index and bundle from different builds white-screen the SPA.

## Qualification result

`/home/ubuntu/final-qual.sh` on the release binaries: **81 checks, 81 PASS,
0 FAIL**. The separate AI live run (`tx-ai-qual.sh`, below) adds **19
checks, 19 PASS**. Sections below list the evidence.

## CLI and feature matrix

- `--version` = 0.32.0; 40 product subcommands (39 before the lockfile
  feature; the implicit `help` excluded).
- Lockfile: `lock` wrote a 549 B companion and `lock --check --json` was
  clean; a workflow edit produced exit 2 with `LOCK-002` content/resolved-hash
  findings; `run --locked --provenance` completed and
  `provenance verify --json` reported a `sha256:ead0e5a3…` lock digest; with
  the lock removed, `run --locked` exited 1 with `LOCK-001`.
- Locked repair: a failing rule plus a reviewed patch under
  `run --locked --apply-patch` exited 1 with `REPAIR_RERUN_FAILED`; the
  patched rule never executed (its output file was absent), so the post-gate
  workflow rewrite was refused before any patched command ran.
- Resource ledger: a successful 1 s rule followed by a failed re-execution
  rendered `status: failed`, `wall_time_secs: 0.002` (its own attempt, not
  the previous 1 s), `rules_executed: 1`, `source.backend: local`,
  `source.tool: sysinfo`. `-f csv --acct <missing>` exited 1 before reading
  the export, with the JSON-only message.
- Signed attestation: `openssl rand -hex 32 > key` (newline-terminated) now
  signs and verifies (exit 0); a tampered output digest exits 2; the wrong
  key exits 3.
- SPA: the rebuilt Vite bundle was served at `/` (200, hashed asset entry).

## Web personal + lock staging

- Health `status: ok`, `mode: personal`; OpenAPI 96 paths / 111 operations;
  `/api/runs` carries a requestBody and per-operation `bearerAuth` + `apiKey`;
  `/api/health` declares `security: []`; `DELETE /api/health` answers the
  four-field `405 METHOD_NOT_ALLOWED` envelope.
- Web lock staging: a `[engine] locked = true` workflow submitted through
  `POST /api/runs` without the lock failed with `LOCK-001`; the same
  `toml_content` plus `include_files: {"oxo-flow.lock": …}` completed and
  wrote its output. This closes the documented web/queued/remote staging
  requirement on the final binary.

## Web team (SQLite)

- admin/user/viewer login returns tokens; viewer `POST /api/runs` 403;
  user `GET /api/audit` 403; admin 200.
- AI key contract (runtime pinned `disabled`): a deepseek save with a key
  returns 200; a same-provider blank-key save returns 200 with
  `applied_to_runtime: false`; a blank-key switch to claude returns 400
  `AI_KEY_REQUIRED` with detail/suggestion and does not echo the stored key.
- Audit chain: export carries events + logs; verify `ok: true`, 0 findings;
  offline `audit verify` exit 0; a tampered export exits 2; a malformed
  export exits 4.

## Web PostgreSQL

- Fresh `oxo_final2` database owned by the `oxo` role: health database
  component ok; a web-spawned run completed; chain verify `ok: true` with 4
  events and 0 findings. Single-host only.
- The pre-existing `oxo_final` database is owned by the `oxo_final` role and
  grants `CREATE` on `public` to `pg_database_owner` only, so the `oxo` role
  cannot bootstrap the schema there. That is an environment/ownership
  condition, not a product defect; the qualification uses a database the
  connecting role owns.

## Web HPC

- `mode: hpc`; scheduler component `ok` (SLURM present); OpenAPI 97
  paths / 112 operations including `/api/hpc`; unauthenticated `GET /api/runs`
  answers 401 (auth middleware active).

## Linux boundaries

- Landlock (`OXO_FLOW_WEB_SANDBOX=landlock`, kernel 6.8): a rule reading
  `/etc/passwd` failed with `Permission denied`; an `echo` rule completed.
  The server logs the documented residual (abstract UNIX sockets/signals
  unscoped by this ABI).
- `OXO_FLOW_WEB_RLIMIT_CPU_SECS=2`: `yes > /dev/null` exited 137
  (`Killed`) after 2.0 s; an `echo` rule completed.
- `OXO_FLOW_TMPDIR`: the rule observed the exact scratch path.

## AI live providers (final release binary)

- `<proxy-1>` (OpenAI-compatible, GLM/DeepSeek/Qwen; `DeepSeek`): `ai test`
  3/3; `template --ai` produced a workflow that validates and dry-runs.
- `<proxy-2>` (Anthropic-compatible, `qwen3.8-flash-next-nothink`): `ai test`
  3/3; `template --ai` produced a workflow that validates and dry-runs.
- Web personal with `<proxy-2>`: health `ai_provider: connected`;
  `POST /api/ai/test` success with provider/model; `POST /api/ai/translate`
  200 with `toml_content`, `provider`, `model`, `fallback_used: false`;
  `POST /api/chat/send/json` answered `mode: "chat"`. Neither response
  contained key bytes.
- Keys were passed only through the process environment, never written to
  the repository; the temporary transport file was mode 600 and shredded
  after the run.

## Independent audit

An independent read-only web/frontend/docs audit on the same head reported
0 Critical, 0 Important, and 3 Minor help-text/doc gaps: the global `--json`
help omitted `export-dsl` and `lock` (both implement it), the
effective-config redaction contract omitted `tiers.server_api_url`, and the
SPA footer claim ignored the standalone `/login` and `/share` routes. All
three are fixed in this change; no functional defect was found.

## Honest limits

- One host, one operator, one local PostgreSQL; no multi-instance,
  crash-recovery, pool-exhaustion, network-partition, or production-scale
  PostgreSQL campaign.
- No full Playwright browser run on tx; focused e2e ran on the Mac.
- Kubernetes image rendering and the two-node k3s/NFS campaign are recorded
  separately (co-located Lima VMs); no cloud-provider volume/HA.
- Cloud batch live campaigns remain blocked (Tencent invalid credentials;
  AWS/GCP credentials/unreachable).
- The AI proxy may route a model name to a different backing model; no paid
  calls in CI. One early local `ai test` probe failed its generation stage
  transiently against `<proxy-1>` and passed on an immediate retry and in
  the recorded tx run (3/3).
- Resource-ledger local I/O is sampled (200 ms), so sub-tick rules report
  unknown; cost/GPU/user-system/peak-memory remain schema-only (no source).
- The lockfile is evidence of declared inputs, not bitwise reproducibility;
  `lock_digest` is not a signature.
