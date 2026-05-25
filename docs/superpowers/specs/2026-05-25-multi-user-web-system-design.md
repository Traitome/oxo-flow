# oxo-flow Multi-User Web System Design

**Date:** 2026-05-25
**Status:** Approved
**Scope:** MVP (P1) - Individual user experience focus

---

## Overview

Design a comprehensive, professional multi-user web system for oxo-flow, targeting:
- **Bioinformatics Lab**: Researchers running pipelines collaboratively
- **Enterprise Platform**: RBAC, quotas, audit compliance
- **HPC Cluster Portal**: Slurm/PBS integration, job submission

MVP focuses on individual user experience; collaboration features deferred to P2.

---

## Core Decisions

| Decision | Choice |
|----------|--------|
| Frontend Architecture | HTMX + Alpine.js CDN (pure Rust binary, no Node.js) |
| UI Layout | Card-based dashboard (深色 GitHub 风格) |
| Module Organization | Refactor lib.rs → handlers/templates modules + maud templates |
| Testing | Rust API tests (existing) + Playwright E2E (separate Node project) |
| Database | SQLite for core data + file system for audit logs with rotation |
| Feature Phasing | MVP individual UX → P2 collaboration → P3 HPC/advanced |

---

## Architecture

### Module Structure

```
crates/oxo-flow-web/src/
├── main.rs              (CLI entry, ~50 lines)
├── lib.rs               (Router build, shared types, ~100 lines)
├── auth.rs              (Login/session/permissions, ~300 lines)
├── handlers/
│   ├── workflow.rs      (validate/parse/run/save, ~400 lines)
│   ├── runs.rs          (list/detail/logs/cancel, ~300 lines)
│   ├── system.rs        (health/metrics/version, ~150 lines)
│   ├── reports.rs       (generate HTML/JSON, ~200 lines)
│   └── partials.rs      (HTMX HTML fragments, ~300 lines)
├── templates/
│   ├── dashboard.rs     (Dashboard page, ~400 lines)
│   ├── workflow.rs      (Editor/list pages, ~400 lines)
│   ├── runs.rs          (Run detail/logs, ~300 lines)
│   ├── auth.rs          (Login/user management, ~200 lines)
│   └── partials.rs      (Shared components, ~300 lines)
├── sse.rs               (Real-time events, ~150 lines)
├── db.rs                (SQLite ops, existing - keep)
├── workspace.rs         (User workspace isolation, existing - keep)
├── executor.rs          (Background executor, existing - keep)
├── rate_limit.rs        (Rate limiting middleware, ~100 lines)
└── audit.rs             (Audit log file rotation, ~150 lines)
```

**Key Refactoring:**
- lib.rs: 3400 lines → ~100 lines (Router + shared types only)
- handlers/: Each endpoint module independent
- templates/: maud templates for type-safe HTML generation
- Keep existing db.rs/workspace.rs/executor.rs unchanged

### File System Structure

```
logs/
├── audit/
│   └── YYYY-MM-DD.log   (Daily rotation, TTL 30 days)
└── runs/
    └── {username}/{run_id}.log  (Per-run archive)
```

---

## Frontend Design

### Dashboard Layout

Card-based dashboard with GitHub dark theme (#0d1117 base):

```
┌─────────────────────────────────────────────────────────────┐
│  ⬡ oxo-flow Command Center          👤 admin  [Sign Out]    │
├─────────────────────────────────────────────────────────────┤
│  ┌─────────────────────┐  ┌─────────────────────┐           │
│  │ 📊 系统资源          │  │ ⚡ 运行状态          │           │
│  │  CPU  内存  活跃运行 │  │  ● running wgs      │           │
│  │  [实时图表]          │  │  ● pending rnaseq   │           │
│  └─────────────────────┘  └─────────────────────┘           │
│  ┌───────────────────────────────────────────────┐          │
│  │ 🚀 快速操作                                     │          │
│  │ [+新建] [模板] [我的Workflow] [运行历史]        │          │
│  └───────────────────────────────────────────────┘          │
│  ┌───────────────────────────────────────────────┐          │
│  │ 📁 最近Workflow                                 │          │
│  │  [wgs-germline] [rnaseq-pipeline] [qc-workflow]│          │
│  └───────────────────────────────────────────────┘          │
└─────────────────────────────────────────────────────────────┘
```

### HTMX Interactions

| Attribute | Usage |
|-----------|-------|
| `hx-get="/api/metrics" hx-trigger="every 5s"` | Resource card auto-refresh |
| `hx-get="/api/events" hx-trigger="sse:run_update"` | SSE-driven run status updates |
| `hx-get="/partials/workflow-editor" hx-target="#modal"` | Open editor in modal |
| `hx-get="/api/runs" hx-target="#main-content"` | SPA-style content swap |

### CDN Libraries

- **Alpine.js** (~15KB): Simple interactivity (dropdowns, modals, tabs)
- **Chart.js** (~60KB): Real-time resource graphs
- No npm/build step required

---

## API Endpoints

### Existing (Keep)

All 30+ existing endpoints remain unchanged:
- `/api/auth/login`, `/api/auth/me`
- `/api/workflows/validate`, `/api/workflows/parse`, `/api/workflows/dag`
- `/api/workflows/run`, `/api/workflows/save`, `/api/workflows/saved`
- `/api/runs`, `/api/runs/{id}`, `/api/runs/{id}/logs`
- `/api/metrics`, `/api/system`, `/api/health`

### New (MVP)

| Endpoint | Method | Purpose |
|----------|--------|---------|
| `/partials/dashboard` | GET | Dashboard HTML fragment |
| `/partials/workflow-editor` | GET | Editor modal HTML |
| `/partials/run-detail/{id}` | GET | Run detail card HTML |
| `/partials/log-stream/{id}` | GET | SSE log stream |
| `/api/workflows/templates` | GET | Template list JSON |
| `/api/workflows/templates/{name}` | GET | Template TOML content |
| `/api/user/profile` | GET/PUT | User profile CRUD |
| `/api/auth/sessions` | GET | Active sessions list |
| `/api/auth/sessions/{id}` | DELETE | Revoke session |
| `/api/audit/recent` | GET | Recent audit logs |
| `/api/logs/archive/{user}/{run}` | GET | Archived log download |

---

## Testing Strategy

### Rust Tests (Existing)

- 858 API unit tests in lib.rs
- 20-user simulation tests in simulation_20users.rs
- DB tests embedded in db.rs

### Playwright E2E (New)

```
tests/browser_e2e/
├── package.json
├── playwright.config.ts
└── tests/
    ├── auth.spec.ts           # Login/session UI
    ├── dashboard.spec.ts      # Card interactions
    ├── workflow.spec.ts       # Create/edit/run
    ├── runs.spec.ts           # Detail/logs/cancel
    ├── templates.spec.ts      # Load/clone templates
    └── scenarios/
        └── 20-users.spec.ts   # 20 expert scenarios E2E
```

### CI Workflow

```yaml
jobs:
  rust-ci:
    - cargo fmt --check
    - cargo clippy --all-targets
    - cargo build
    - cargo test --all

  playwright-ci:
    - npm ci
    - npx playwright install chromium
    - npx playwright test
    needs: rust-ci
```

---

## Feature Phasing

### P1 MVP (4-6 weeks)

| Feature | Priority |
|---------|----------|
| Module refactoring (handlers/templates) | 🔴 Must |
| Dashboard (cards + real-time metrics) | 🔴 Must |
| User management (login/session/profile) | 🔴 Must |
| Workflow editor (TOML edit + validate + save/load) | 🔴 Must |
| Run management (start/detail/logs/cancel) | 🔴 Must |
| Template library (5 predefined templates) | 🟡 Important |
| Playwright tests (core flows + 10 user scenarios) | 🟡 Important |
| Audit logs (file rotation) | 🟢 Optional |

### P2 (2-3 weeks)

- Shared workflow library
- Team workspaces
- RBAC permission matrix
- User quotas
- Batch operations
- Workflow versioning

### P3 (3-4 weeks)

- Slurm/PBS integration
- Scheduled runs (cron-like)
- DAG visualization (Graphviz/D3)
- Custom report templates

---

## User Scenarios (20 Expert Perspectives)

Covered by simulation_20users.rs + Playwright E2E:

1. **Bioinformatics Researcher** - WGS validation
2. **Cancer Genomics** - Paired tumor-normal
3. **Population Geneticist** - Multi-sample cohort
4. **QC Specialist** - Lint and validation
5. **Computational Biologist** - Conditional rules
6. **HPC Specialist** - Cluster resources
7. **Workflow Reliability Engineer** - Checkpoint/resume
8. **Environment Manager** - Multi-env validation
9. **Clinical Reporter** - Report generation
10. **Power User** - All features combined
11. **Transcriptomics Researcher** - RNA-seq
12. **DevOps Engineer** - Docker/Singularity export
13. **API Developer** - Full REST lifecycle
14. **Security Auditor** - Injection prevention
15. **Beginner Student** - Common mistakes
16. **Core Facility Manager** - Batch operations
17. **Container Engineer** - Docker/Singularity
18. **Data Scientist** - Stats and metrics
19. **System Administrator** - Health monitoring
20. **QA Engineer** - Edge cases

---

## Security Model

### Workspace Isolation

- Physical: `workspace/users/<username>/runs/<run_id>`
- Database: All queries scoped by `user_id`
- Ownership verification on all sensitive operations

### Authentication

- Session tokens: UUIDv4, 24-hour expiry
- Bearer header or `oxo_session` cookie
- Passwords via environment variables

### Audit Logging

- File-based: `logs/audit/YYYY-MM-DD.log`
- Rotation: Daily, TTL 30 days
- Format: JSON lines with timestamp, user, action, resource

---

## Implementation Notes

### maud Template Example

```rust
use maud::{html, Markup};

fn dashboard_card(title: &str, content: Markup) -> Markup {
    html! {
        div.card style="background: #161b22; border-radius: 8px; padding: 16px;" {
            div style="color: #58a6ff; font-weight: 500; margin-bottom: 12px;" {
                (title)
            }
            (content)
        }
    }
}
```

### HTMX Partial Handler

```rust
async fn partials_dashboard() -> Markup {
    html! {
        div id="dashboard" hx-get="/api/metrics" hx-trigger="every 5s" hx-swap="innerHTML" {
            (resource_card())
            (runs_card())
            (quick_actions())
        }
    }
}
```

---

## Success Criteria

MVP considered complete when:

1. `make ci` passes (fmt + clippy + build + test)
2. Dashboard displays real-time metrics with <1s latency
3. Workflow editor validates TOML and saves to SQLite
4. Run lifecycle (start → logs → cancel) works end-to-end
5. 10+ Playwright scenarios pass in CI
6. 20-user simulation tests pass
7. Audit logs rotate correctly

---

## References

- [web-api.md](../../guide/src/reference/web-api.md) - Existing API docs
- [simulation_20users.rs](../../../crates/oxo-flow-web/tests/simulation_20users.rs) - Current tests
- [HTMX Documentation](https://htmx.org/docs/)
- [Alpine.js Documentation](https://alpinejs.dev/start-here)
- [maud Documentation](https://maud.lambda.xyz/)