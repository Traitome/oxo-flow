# Deployment Modes

oxo-flow supports three deployment modes from a single binary.
Choose the mode that fits your team size and infrastructure.

## Quick Reference

```bash
# Personal workstation (default)
oxo-flow serve

# Team server
oxo-flow serve --mode team

# HPC submit panel
oxo-flow serve --mode hpc
```

## Mode 1: Personal (Default)

**Use when**: You're the only user, working on your own machine.

| Setting | Value |
|---------|-------|
| Network | `127.0.0.1:8080` (localhost only) |
| Database | SQLite file (`oxo-flow.db`) — override with `DATABASE_URL` |
| Auth | None (single user) — management endpoints (Users, Audit) follow the same localhost trust model |
| Workspace | `workspace/users/local_user/runs/<run_id>` |

```bash
# Start
oxo-flow serve

# With custom port
oxo-flow serve -p 9090

# Open browser
open http://localhost:8080
```

## Mode 2: Team

**Use when**: Multiple users share a server or cloud instance.

| Setting | Value |
|---------|-------|
| Network | `127.0.0.1:8080` by default — pass `--host 0.0.0.0` to bind all interfaces |
| Database | SQLite (default) or PostgreSQL — see [Database Selection](#database-selection-database_url) |
| Run execution | Backend-routed code exists for SQLite and PostgreSQL; schema bootstrap + core CRUD are CI-smoke-verified on live PostgreSQL, while concurrency/crash-recovery/multi-instance remain unvalidated. Team mode has no OS-level tenant execution boundary, so expose it only to fully trusted workflow authors |
| Auth | Password env vars (`OXO_FLOW_ADMIN_PASSWORD` / `OXO_FLOW_USER_PASSWORD` / `OXO_FLOW_VIEWER_PASSWORD`) + optional ORCID/GitHub OAuth |
| Workspace | `workspace/users/<username>/runs/<run_id>` |

```bash
# Start with SQLite (default) and bind to all interfaces
oxo-flow serve --mode team --host 0.0.0.0

# Set auth credentials
export OXO_FLOW_ADMIN_PASSWORD=...
export OXO_FLOW_USER_PASSWORD=...
export OXO_FLOW_VIEWER_PASSWORD=...
export ORCID_CLIENT_ID=...
export ORCID_CLIENT_SECRET=...
# Test the ORCID flow against sandbox.orcid.org instead of production (optional)
# export ORCID_SANDBOX=1
export GITHUB_CLIENT_ID=...
export GITHUB_CLIENT_SECRET=...

oxo-flow serve --mode team
```

**Authentication**: role-based password authentication comes from the
`OXO_FLOW_*_PASSWORD` environment variables (with a dev-mode fallback when
`OXO_FLOW_DEV_MODE=1`). ORCID and GitHub OAuth are available as separate
login endpoints when their client credentials are configured — there is no
automatic fallback chain between mechanisms.

**User management**: beyond the env-var credentials, accounts created on
the **Users** page (or `POST /api/users`) carry a bcrypt-hashed password in
the database and sign in through the same `/api/auth/login` endpoint.
State-changing requests attempt an audit entry visible on the **Audit** page
or through `GET /api/audit`, with the acting user and recorded outcome. Audit
writes are not yet a durable, fail-closed compliance ledger: a failed insert
is logged and the request still succeeds, so do not treat a missing entry as
proof that no action occurred (the structured event stream still emits an
`audit` event for machine consumers when the database insert fails). The
trail spans all users and is admin-only outside personal mode.

**Workspace isolation**:
```
workspace/
└── users/
    └── <username>/
        └── runs/<run_id>/
```

## Mode 3: HPC

**Use when**: The web UI is a submit panel; actual execution happens on a cluster.

| Setting | Value |
|---------|-------|
| Network | `127.0.0.1:8080` by default — pass `--host 0.0.0.0` to bind all interfaces |
| Database | SQLite or PostgreSQL — see [Database Selection](#database-selection-database_url) |
| Auth | Same as Team mode |
| Executor | SLURM / PBS / LSF / SGE |
| Resources | Scheduler-managed |

```bash
oxo-flow serve --mode hpc
```

**Connecting the app to a cluster** (the deployment model):

- **App anywhere, cluster elsewhere.** The web UI can run on your
  workstation, a lab portal server, or any cloud VM — it does not need to
  live on the cluster. Cluster execution happens where the CLI runs:
  SSH into the login node (or run the CLI there directly) and use
  `oxo-flow cluster submit` / `oxo-flow run --profile cluster` to dispatch
  work through SLURM/PBS/LSF/SGE.
- **App on the login node (`--mode hpc`).** When the web server itself runs
  on a cluster login node, hpc mode detects the scheduler at startup
  (`sinfo`/`qstat`/`pbsnodes`/`bjobs`) and exposes `GET /api/hpc` with live
  queue/partition status in the UI.
- **App mounted at a sub-path.** On shared portal servers the app can be
  mounted under any prefix with `--base-path /oxo-flow` — the router, the
  SPA, and all asset URLs follow the mount point automatically (the SPA
  index gets a `<base>` tag injected at serve time), so it composes with
  reverse proxies without a rebuild.

```bash
# App on the login node, mounted under /oxo-flow
oxo-flow serve --mode hpc --base-path /oxo-flow

# App on your workstation, cluster execution via the CLI on the login node
ssh login-node
oxo-flow cluster submit workflow.oxoflow -b slurm
```

## Switching Modes

```bash
# Environment variable (alternative to --mode flag)
export OXO_FLOW_MODE=team
oxo-flow serve
```

## Database Selection (`DATABASE_URL`)

All serving entry points — `oxo-flow serve`, the standalone
`oxo-flow-web` binary, and the desktop app — share one database-selection
rule: `DATABASE_URL` starting with `postgres://` (or `postgresql://`)
selects the PostgreSQL backend; anything else (including unset, which
defaults to `sqlite://oxo-flow.db` in the working directory) stays SQLite.

```bash
# PostgreSQL (storage-backed web metadata and backend-routed run records)
export DATABASE_URL=postgres://oxo:secret@db.lab.example.edu:5432/oxo_flow
oxo-flow serve --mode team --host 0.0.0.0
```

Two constraints apply:

- **Build feature**: PostgreSQL support is a compile-time feature. The
  published Docker image is built WITHOUT it — rebuild with
  `cargo build --features postgres` (or the equivalent in your own
  image). On a build without the feature, a `postgres://` `DATABASE_URL`
  aborts startup with a clear error instead of silently falling back.
- **Run execution**: the `StorageBackend` trait has PostgreSQL code paths for
  run-record persistence. CI runs a live-PostgreSQL smoke test (a
  `services: postgres` container) that verifies the schema bootstrap and the
  core `StorageBackend` CRUD surface — users, runs, run nodes, audit logs,
  webhook config, chat sessions — against a real server. The storage
  contract was additionally live-verified against a `postgres:16` container
  (2026-11-05): the full `postgres_storage_contract` round-trip passed and
  the complete web test suite ran 554 passed / 0 failed with the `postgres`
  feature enabled. That evidence does NOT extend to production parity:
  concurrency, crash recovery, and multi-instance behavior remain
  unvalidated, so test them before relying on PostgreSQL for execution
  metadata. The runs gate is backend-agnostic in behavior — any initialized
  storage backend passes — and answers `503 RUNS_REQUIRE_STORAGE_BACKEND`
  only when no backend was initialized at all.
- **Tenant safety**: team/HPC authorization is not an OS or container
  execution boundary. A normal workflow can run shell commands under the
  server account and may access server environment secrets. Do not invite
  untrusted tenants until the isolation gate in
  `docs/superpowers/audits/industrial-scientific-audit-ledger.md` passes.

## Startup Verification

On startup, oxo-flow prints the version banner, the serve mode and address, and tracing log lines:

```
oxo-flow v0.29.0 — Rust-native bioinformatics pipeline engine
Serve: Starting oxo-flow web server in personal mode on 127.0.0.1:8080
 INFO Logging initialized at logs
 INFO AI registry initialized: provider=disabled, model=default, enabled=false
 INFO Building router for mode: personal
 INFO Starting oxo-flow web server in personal mode on 127.0.0.1:8080
```

Log lines carry timestamps in a terminal, and the mode, host, and port follow `--mode`/`--host`/`-p`.

The banner line is interactive-terminal decoration: when stderr is redirected
(`oxo-flow serve > serve.log 2>&1`, nohup, service managers), it is omitted —
the log lines alone identify the running version.

## Platform Configuration

Server-tier settings, AI defaults, and SSH cluster definitions can live in
a **platform config file** — the lowest-precedence layer (CLI flag > env
var > config file > built-in default). The file is looked up at
`OXO_FLOW_CONFIG`, then `oxo-flow.web.toml` in the working directory, then
`~/.config/oxo-flow/web.toml`.

```toml
[server]
mode = "team"            # personal | team | hpc
host = "0.0.0.0"
port = 8080
base_path = "/oxoflow"   # sub-path mounts behind a reverse proxy

[ai]
provider = "deepseek"    # anthropic | openai | deepseek | ollama | disabled
api_url = "https://api.deepseek.com"
model = "deepseek-v4-pro"
api_key_env = "DEEPSEEK_API_KEY"   # secrets stay in env vars, never inline

[[clusters]]
id = "lab-slurm"                  # stable key (import is idempotent)
name = "Lab SLURM cluster"
ssh_host = "login.lab.example.edu"
ssh_port = 22
ssh_user = "bioinf"
ssh_key = "~/.ssh/id_ed25519"
scheduler = "slurm"               # auto | slurm | pbs | lsf | sge
remote_dir = "~/oxo-flow-jobs"
```

The same choices are editable at runtime on the **Clusters & Servers**
page: add an SSH endpoint and press **Probe** — the server performs a
real SSH round-trip (BatchMode, 8 s timeout), reports the remote hostname
and detects the scheduler (`slurm`/`pbs`/`lsf`/`sge`) with its version.
Unknown fields in the config file are rejected loudly, not silently
ignored.

## Multi-Tenancy

Team/HPC modes scope every resource to the acting user:

- **Ownership**: runs and pipelines are owned by the session's user;
  foreign resources 404 (existence never leaks). Admins see and control
  everything; viewers/guests get read access to workspace-visible
  pipelines only.
- **Anonymous surface**: `/api/system`, `/api/metrics`, `/api/ai/test`,
  and `/api/hpc` require authentication; `GET /api/ai/config` stays
  public; `/api/events` requires a one-time `?ticket=` from
  `POST /api/events/ticket` (EventSource cannot set headers) and
  streams only the subscriber's runs; `/api/share/{token}`
  is public by design (the token is the credential).
- **Env-password logins** auto-provision a real users row (previously any
  username shared the `default` pseudo-user's identity).
- **API keys** (`X-API-Key: oxo_…`) are first-class machine credentials
  with the owner's exact permissions; stored hashed, instantly revocable.
- **Shared infrastructure**: the server AI provider, webhook endpoint, and
  SSH cluster connections are admin-managed outside personal mode.

## Server Data Layout (`OXO_FLOW_WORKSPACE`)

The web server keeps all user-scoped data under one workspace root,
`OXO_FLOW_WORKSPACE` (default: a relative `workspace/` directory under the
server process's cwd):

- `users/<username>/inputs/` — files uploaded via `POST /api/files`;
  mirrored into each new run's workdir at Create Run
  (`POST /api/runs`, which stages `include_files` straight into the run
  dir without touching `inputs/`)
- `users/<username>/runs/<run_id>/` — each run's workdir (the spawned CLI
  runs here; checkpoint, logs, and envs live under it)
- `users/<username>/pipelines/<pipeline_id>/` — persistent pipeline
  directories so checkpoints survive web re-runs

Usernames and run ids are traversal-validated before they are joined onto
the root. In container or systemd deployments, point `OXO_FLOW_WORKSPACE`
at a persistent volume — a server started from a different cwd otherwise
silently loses sight of previous runs. The workspace root is server-tier
configuration only; it has no per-user override.

## Per-User AI Credentials

Every user-facing AI call (chat, translate, explain, interpret, optimize)
resolves the **acting user's own saved provider** (`PUT /api/ai/config/user`)
before falling back to the shared runtime:

- resolution order per call: the user's row (unless `disabled`) → the
  server-level provider (env / admin config) → default
- a non-admin's saved key **never reconfigures the shared runtime** — it
  is cached per user and invalidated on every config write
- a per-user row without an api key carries an empty key: their calls
  fail loudly instead of silently borrowing the server's key (no
  shared-secret leakage in either direction)
- server-level writes (`POST /api/ai/config`, `PUT /api/ai/config/server`)
  stay admin-only outside personal mode and invalidate all per-user
  fallbacks

## Testing & Development

`OXO_FLOW_DISABLE_RATE_LIMIT=1` disables the per-IP rate limiter — the
browser e2e suite (and any local automation) legitimately exceeds the
100 req/min budget and must set it. Never set it on a team/hpc
production server: the limiter is the brute-force protection.

## Run Control Status (all modes)

Run control uses process signaling, but lifecycle truth remains
prototype-grade:

- **Cancel** attempts SIGTERM, then SIGKILL after a grace period, against the
  run's process group. Cancellation errors and races with completion are not
  yet reconciled through monotonic compare-and-set transitions.
- **Pause/resume** sends SIGSTOP/SIGCONT, but persistence failures and missing
  process groups can leave the database state inconsistent with the process.
- **Crash restart** attempts to probe and re-attach unfinished runs. The
  backend does not yet provide restart-safe multi-instance ownership or proof
  that a terminal state cannot be overwritten in every race. There is no
  cross-instance lease: two servers pointed at the same database can both
  adopt the same run after a restart.

Do not present these controls as crash-consistent or compliance-grade. The
release blockers and required tests are tracked in the audit ledger.

## Production Deployment

Two pieces complete a production serve: a supervised service and — when
users reach the server over a network — a reverse proxy in front of it.

### systemd service

```ini
# /etc/systemd/system/oxo-flow.service
[Unit]
Description=oxo-flow web server
After=network-online.target
Wants=network-online.target

[Service]
Type=simple
User=oxo
Group=oxo
WorkingDirectory=/opt/oxo-flow
EnvironmentFile=-/etc/oxo-flow/server.env
ExecStart=/usr/local/bin/oxo-flow serve
Restart=on-failure
RestartSec=2
TimeoutStopSec=15

[Install]
WantedBy=multi-user.target
```

Keep secrets in the EnvironmentFile (`chmod 600`), never inline in the
unit:

```bash
# /etc/oxo-flow/server.env
OXO_FLOW_MODE=team
OXO_FLOW_HOST=127.0.0.1
OXO_FLOW_ADMIN_PASSWORD=…
DEEPSEEK_API_KEY=…
```

A stop (`systemctl stop oxo-flow`) delivers SIGTERM; the server drains
connections and exits. Runs in flight are not force-killed. Cancel them from
the UI before a planned stop and verify the terminal state; restart recovery
is not yet qualified for unattended production use (see
[Run Control Status](#run-control-status-all-modes)).

### nginx reverse proxy

Behind a reverse proxy every request arrives at oxo-flow with the
proxy's own IP as the transport peer — the rate limiter would key all
clients into one shared bucket (a single abuser locks everyone out).
Set `OXO_FLOW_TRUSTED_PROXY=1` so the limiter keys on the forwarded
`X-Forwarded-For` / `X-Real-IP` headers instead. Only enable this when
the proxy in front **overwrites** those headers on every request (both
recipes below do, via `X-Real-IP $remote_addr`); if clients can reach
the server without passing through it, a caller could spoof the header
and bypass the limiter.

```nginx
server {
    listen 80;
    server_name flows.lab.example.edu;

    location /oxo-flow/ {
        proxy_pass http://127.0.0.1:8080;   # no trailing slash: keep the mount prefix
        proxy_http_version 1.1;
        proxy_set_header Host $host;
        proxy_set_header X-Real-IP $remote_addr;
        proxy_set_header X-Forwarded-For $proxy_add_x_forwarded_for;
        proxy_set_header X-Forwarded-Proto $scheme;
        # Server-Sent Events (/api/events) require unbuffered streaming.
        proxy_buffering off;
        proxy_read_timeout 300s;
    }
}
```

Start the app with the matching mount point and the trusted-proxy flag
(the headers the recipe sets above are what the limiter then reads):

```bash
oxo-flow serve --mode team --base-path /oxo-flow
```

```bash
# /etc/oxo-flow/server.env (append)
OXO_FLOW_TRUSTED_PROXY=1
```

For TLS-terminating proxies (the common public deployment), also declare
the public origin so minted share URLs tell the truth about the transport
— by default they carry `oxo+http://` with the local bind
host/port, which is unreachable from outside the proxy:

```bash
# /etc/oxo-flow/server.env (append) — public HTTPS entry point
OXO_FLOW_PUBLIC_URL=https://flows.lab.example.edu/oxo-flow
```

Share URLs then become `oxo+https://flows.lab.example.edu/oxo-flow/share/<token>`
(see [Collaboration](collaboration.md)); the import API accepts both
schemes, so previously minted links keep working.

## Performance

| Mode | Startup | Memory (idle) | Memory (100 pipelines) |
|------|---------|---------------|------------------------|
| Personal | <0.1s | ~30MB | ~150MB |
| Team | <0.3s | ~40MB | ~200MB |
| HPC | <0.3s | ~40MB | ~200MB |

> These are indicative figures, not benchmark results — measure on your own
> hardware before capacity planning.

## Deployment Smoke Tests

`scripts/deploy-smoke.sh` is the repeatable acceptance suite for the
deployment shapes above — every scenario runs in an isolated temp dir,
asserts against the live server, and cleans up after itself:

| Scenario | What it verifies |
|---|---|
| 1. Personal mode | API + health + SPA on a source build |
| 2. Standalone web binary | the release-style binary serves independently |
| 3. Sub-path mount | API under `--base-path`, root excluded, SPA `<base>` injection, assets |
| 4. Platform config file | port/base_path defaults + `[[clusters]]` seeding |
| 5. Team mode | 401 without a session; env-credential login → session → authenticated access |
| 6. HPC mode | scheduler endpoint responds with structured status |
| 7. Desktop bundle | the `.app` serves the SPA self-contained (`OXO_APP=…`) |

```bash
# Against the local debug build
scripts/deploy-smoke.sh

# Against a release build / packaged app
OXO_BIN=target/release/oxo-flow \
OXO_APP=target/release/bundle/osx/oxo-flow.app \
scripts/deploy-smoke.sh
```

Run it on every machine you deploy to — it exits non-zero on any failed
assertion, so it doubles as a CI gate for packaging.
