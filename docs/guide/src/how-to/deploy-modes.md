# Deployment Modes

oxo-flow v0.8 supports three deployment modes from a single binary.
Choose the mode that fits your team size and infrastructure.

## Quick Reference

```bash
# Personal workstation (default)
oxo-flow serve

# Team server
oxo-flow serve --mode team

# HPC submit panel
oxo-flow serve --mode hpc --scheduler slurm
```

## Mode 1: Personal (Default)

**Use when**: You're the only user, working on your own machine.

| Setting | Value |
|---------|-------|
| Network | `127.0.0.1:8777` (localhost only) |
| Database | SQLite file (`oxo-flow.db`) |
| Auth | None (single user) |
| Workspace | `workspace/personal/` |

```bash
# Start
oxo-flow serve

# With custom port
oxo-flow serve -p 9090

# Open browser
open http://localhost:8777
```

## Mode 2: Team

**Use when**: Multiple users share a server or cloud instance.

| Setting | Value |
|---------|-------|
| Network | `0.0.0.0:8777` (all interfaces) |
| Database | SQLite (default) or PostgreSQL |
| Auth | ORCID OAuth2 → GitHub OAuth2 → Invite Code → Basic |
| Workspace | `workspace/users/<username>/` |

```bash
# Start with SQLite (default for <15 users)
oxo-flow serve --mode team

# Start with PostgreSQL (recommended for >15 users)
oxo-flow serve --mode team --db postgres://user:pass@localhost/oxoflow

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
| Network | `0.0.0.0:8777` |
| Database | SQLite or PostgreSQL |
| Auth | Same as Team mode |
| Executor | SLURM / PBS / LSF / SGE |
| Resources | Scheduler-managed |

```bash
# Start with SLURM
oxo-flow serve --mode hpc --scheduler slurm

# Start with PBS
oxo-flow serve --mode hpc --scheduler pbs

# The CLI auto-detects the scheduler if --scheduler is omitted
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

On startup, oxo-flow prints the license banner and mode info:

```
oxo-flow v0.8.1
oxo-flow-core, oxo-flow-cli: Apache 2.0
oxo-flow-web: Dual license — LICENSE-ACADEMIC / LICENSE-COMMERCIAL
Contact: Shixiang Wang <w_shixiang@163.com>

Starting oxo-flow-web in team mode on 0.0.0.0:8777
HPC scheduler detected: slurm (version: 23.02.7)
```

## Performance

| Mode | Startup | Memory (idle) | Memory (100 pipelines) |
|------|---------|---------------|------------------------|
| Personal | <0.1s | ~30MB | ~150MB |
| Team | <0.3s | ~40MB | ~200MB |
| HPC | <0.3s | ~40MB | ~200MB |
