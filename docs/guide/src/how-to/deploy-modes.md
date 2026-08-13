# Deployment Modes

oxo-flow v0.10.x supports three deployment modes from a single binary.
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
| Database | SQLite file (`oxo-flow.db`) |
| Auth | None (single user) |
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
| Database | SQLite (default) or PostgreSQL |
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
export GITHUB_CLIENT_ID=...
export GITHUB_CLIENT_SECRET=...

oxo-flow serve --mode team
```

**Authentication**: role-based password authentication comes from the
`OXO_FLOW_*_PASSWORD` environment variables (with a dev-mode fallback when
`OXO_FLOW_DEV_MODE=1`). ORCID and GitHub OAuth are available as separate
login endpoints when their client credentials are configured — there is no
automatic fallback chain between mechanisms.

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
| Database | SQLite or PostgreSQL |
| Auth | Same as Team mode |
| Executor | SLURM / PBS / LSF / SGE |
| Resources | Scheduler-managed |

```bash
oxo-flow serve --mode hpc
```

**HPC workflow** (current implementation status):
1. User creates/imports pipeline in Web UI
2. oxo-flow can generate cluster job scripts (`oxo-flow cluster submit` on the CLI)
3. The Web UI does not yet submit jobs or poll scheduler status — use the CLI for cluster submission and your scheduler's native tools to monitor

## Switching Modes

```bash
# Environment variable (alternative to --mode flag)
export OXO_FLOW_MODE=team
oxo-flow serve
```

## Startup Verification

On startup, oxo-flow prints the version banner, the serve mode and address, and tracing log lines:

```
oxo-flow 0.11.0 — Bioinformatics Pipeline Engine
Serve: Starting oxo-flow web server in personal mode on 127.0.0.1:8080
 INFO Logging initialized at logs
 INFO AI registry initialized: provider=disabled, model=default, enabled=false
 INFO Building router for mode: personal
 INFO Starting oxo-flow web server in personal mode on 127.0.0.1:8080
```

Log lines carry timestamps in a terminal, and the mode, host, and port follow `--mode`/`--host`/`-p`.

## Performance

| Mode | Startup | Memory (idle) | Memory (100 pipelines) |
|------|---------|---------------|------------------------|
| Personal | <0.1s | ~30MB | ~150MB |
| Team | <0.3s | ~40MB | ~200MB |
| HPC | <0.3s | ~40MB | ~200MB |

> These are indicative figures, not benchmark results — measure on your own
> hardware before capacity planning.
