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
| Workspace | `workspace/personal/` |

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
| Network | `0.0.0.0:8080` (all interfaces) |
| Database | SQLite (default) or PostgreSQL |
| Auth | ORCID OAuth2 → GitHub OAuth2 → Invite Code → Basic |
| Workspace | `workspace/users/<username>/` |

```bash
# Start with SQLite (default)
oxo-flow serve --mode team

# Set auth credentials
export OXO_ORCID_CLIENT_ID=...
export OXO_ORCID_CLIENT_SECRET=...
export OXO_ADMIN_PASSWORD=...

oxo-flow serve --mode team
```

**Authentication chain**: ORCID OAuth2 is attempted first (preferred — every
scientist has one). Falls back to GitHub OAuth2, then invite codes (for
air-gapped labs), and finally basic auth (for dev mode).

**Workspace isolation**:
```
workspace/
├── users/
│   ├── alice/
│   │   ├── pipelines/
│   │   └── runs/<run_id>/
│   ├── bob/
│   └── shared/          # workspace-shared pipelines
└── templates/           # system templates (read-only)
```

## Mode 3: HPC

**Use when**: The web UI is a submit panel; actual execution happens on a cluster.

| Setting | Value |
|---------|-------|
| Network | `0.0.0.0:8080` |
| Database | SQLite or PostgreSQL |
| Auth | Same as Team mode |
| Executor | SLURM / PBS / LSF / SGE |
| Resources | Scheduler-managed |

```bash
# The scheduler (SLURM / PBS / LSF / SGE) is auto-detected
oxo-flow serve --mode hpc
```

**HPC workflow**:
1. User creates/imports pipeline in Web UI
2. User clicks "Submit to Cluster"
3. oxo-flow generates cluster job script
4. Job is submitted to SLURM/PBS
5. Web UI polls scheduler for status
6. Results available when job completes

## Switching Modes

```bash
# Environment variable (alternative to --mode flag)
export OXO_FLOW_MODE=team
oxo-flow serve
```

## Startup Verification

On startup, oxo-flow prints the version banner, the serve mode and address, and tracing log lines:

```
oxo-flow 0.10.2 — Bioinformatics Pipeline Engine
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
