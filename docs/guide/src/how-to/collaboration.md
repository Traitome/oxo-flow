# Collaboration

oxo-flow v0.10.x introduces collaboration primitives for sharing and versioning
pipelines across users and teams.

## Overview

| Operation | Description | Endpoint |
|-----------|-------------|----------|
| Fork | Copy a pipeline to your workspace | `POST /api/pipelines/{id}/fork` |
| Diff | Compare two pipelines | `POST /api/pipelines/diff` |
| Share | Create a shareable link | `POST /api/pipelines/{id}/share` |
| Import | Import from share link | `POST /api/pipelines/import` |

## Fork

Create an independent copy of a pipeline in your workspace. The fork is a full
copy — changes to the original do not affect the fork, and vice versa.

```bash
# API
curl -X POST http://localhost:8080/api/pipelines/pipeline-abc/fork \
  -H "Content-Type: application/json" \
  -d '{"user_id": "alice"}'

# Response
{
  "forked_id": "pipeline-def",
  "name": "my-analysis (fork)"
}
```

Forks record their lineage — the `forked_from` field tracks the source pipeline.

## Diff

Compare two pipelines and see what changed:

```bash
curl -X POST http://localhost:8080/api/pipelines/diff \
  -H "Content-Type: application/json" \
  -d '{"toml_a": "<workflow-a TOML>", "toml_b": "<workflow-b TOML>"}'

# Response
{
  "diffs": [
    {
      "path": "rules[0].shell",
      "category": "modified",
      "description": "STAR --runThreadN 8 → STAR --runThreadN 16",
      "severity": "medium"
    }
  ]
}
```

## Share

Create a shareable link for a pipeline:

```bash
curl -X POST http://localhost:8080/api/pipelines/pipeline-abc/share \
  -H "Content-Type: application/json" \
  -d '{"visibility": "link", "expires_in_days": 30}'

# Response
{
  "share_url": "oxo+https://lab.example.com:8080/share/abc123",
  "access_token": "abc123",
  "expires_at": "2024-02-12T00:00:00Z"
}
```

**Visibility levels** (`link` / `workspace`) are stored on the share record.
Currently the share link is consumed only through the
[import API](#import) below — there is no browser page at the share URL, and
visibility/membership checks are not yet enforced on import.

## Import

Import a pipeline from an `oxo+https://` share link:

```bash
curl -X POST http://localhost:8080/api/pipelines/import \
  -H "Content-Type: application/json" \
  -d '{"url": "oxo+https://lab.example.com:8080/share/abc123"}'

# Response
{
  "pipeline_id": "pipeline-xyz"
}
```

The `oxo+https://` protocol prefix makes share links explicit and unambiguous —
you can paste them into any tool and the intent is clear.

**Import behavior**:
- Creates a full copy of the shared pipeline
- Records the source via `forked_from`
- Sets visibility to `private` by default
- Checks share expiration before allowing import

## Permissions Model

A simple three-state visibility model:

| Level | Access |
|-------|--------|
| Private | Only the owner can view/edit (the default for imported pipelines) |
| Shared (`link` / `workspace`) | Read-only access via share/import |
| Admin | Can view all, manage users |

No nested RBAC. No group hierarchies. Simple and transparent.

## Audit Trail

Forking a pipeline is recorded in the audit log:

```
fork_pipeline → audit_logs
```

Share and import actions are not yet logged. View audit logs via
`GET /api/audit?days=7`.
