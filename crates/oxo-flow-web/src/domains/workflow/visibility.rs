//! The ONE pipeline visibility predicate (WEB-13 / issue #1138).
//!
//! Every path that lists, reads, or writes a stored pipeline evaluates this
//! module — no handler may retype the ownership/visibility rule inline
//! ("Reuse, never retype"). The policy encoded here is:
//!
//! - **Admins see every tenant's pipeline.** Administrators already list and
//!   read across tenants (the audit/administration behavior this rule
//!   formalizes); this is the documented, deliberate override.
//! - **An owner sees their own pipeline**, whatever its `visibility`.
//! - **A `workspace`-visibility pipeline is visible to every signed-in
//!   user.** `workspace` is the only shared-library ("template") flag the
//!   `pipelines.visibility` column defines; in personal mode there is one
//!   `default` pseudo-user, so this degenerates to the owner rule.
//! - **Another tenant's `private` or `link` pipeline is invisible.**
//!   `link` is NOT a read grant: it exists only so the owner can mint a
//!   share token, and the token (not this predicate) authorizes
//!   [`super::handlers`]-level share reads. Unknown/foreign visibility
//!   values fail closed as private.
//! - **Deleted pipelines are invisible to everyone.** Deletion is a hard
//!   delete (`DELETE FROM pipelines` in both storage backends); there is no
//!   soft-delete tombstone, so a deleted pipeline has no row for this
//!   predicate to see and every read path answers `404 NOT_FOUND` from the
//!   missing-row check. No admin resurrection path exists.
//!
//! Enforcement convention: **list paths FILTER with [`can_read_pipeline`]**;
//! **read/write paths answer `404 NOT_FOUND` — never `403`** — when the
//! predicate returns `false`, because a `403` would leak the existence of
//! another tenant's pipeline (issue #82 P0-4, the crate-wide convention).
//!
//! Deliberate exemptions (verified call sites, not gaps):
//!
//! - `GET /api/share/{token}` and `POST /api/pipelines/import`
//!   (`domains/collaboration/handlers.rs`) resolve a row from the `shares`
//!   table; the share token is the capability, and share rows are minted
//!   only by an owner/admin ([`can_write_pipeline`]).
//! - The `/api/templates` library is a separate, deliberately global
//!   resource: its rows carry no per-tenant visibility column (`is_system` /
//!   `created_by` only), every signed-in user may browse it, and writes are
//!   owner/admin-gated in the template handlers.
//! - The DAG edit endpoints (`/api/pipeline/{id}/command|undo|redo`,
//!   `domains/dag/`) operate on caller-supplied TOML and never read a
//!   stored pipeline; their history is keyed by acting identity (WEB-12).
//! - Pure-TOML analysis endpoints (parse/validate/prepare/dag/format/lint/
//!   stats) receive the document in the request body and touch no stored
//!   row. `diff` and `export` DO accept saved-pipeline ids and are gated by
//!   [`can_read_pipeline`].

use crate::domains::auth::current_user::CurrentUser;
use crate::infra::db::models::PipelineRow;

/// The only `pipelines.visibility` value that shares a pipeline with every
/// signed-in user (the shared-library / "template" flag).
pub const SHARED_VISIBILITY: &str = "workspace";

/// Read/visibility predicate — the single source of truth for "may this
/// viewer see this stored pipeline?".
///
/// See the module docs for the full policy, the 404 convention, and the
/// documented exemptions.
pub fn can_read_pipeline(viewer: &CurrentUser, row: &PipelineRow) -> bool {
    // Admins see everything (documented override, existing behavior).
    viewer.is_admin()
        // Owners always see their own pipeline.
        || row.user_id == viewer.id
        // The shared-library flag: visible to every signed-in user.
        || row.visibility == SHARED_VISIBILITY
}

/// Write predicate — owner or admin only.
///
/// A `workspace`-shared pipeline is readable by every signed-in user but
/// writable only by its owner (or an admin); update/delete/rollback/share
/// all enforce this. Fork and import create a private copy owned by the
/// caller after a [`can_read_pipeline`] check, so they do not need this.
pub fn can_write_pipeline(viewer: &CurrentUser, row: &PipelineRow) -> bool {
    viewer.is_admin() || row.user_id == viewer.id
}

#[cfg(test)]
mod tests {
    use super::*;

    const OWNER: &str = "tenant-a";
    const OTHER: &str = "tenant-b";

    fn viewer(id: &str, role: &str) -> CurrentUser {
        CurrentUser {
            id: id.to_string(),
            role: role.to_string(),
        }
    }

    fn row(owner: &str, visibility: &str) -> PipelineRow {
        PipelineRow {
            id: "p1".into(),
            user_id: owner.into(),
            name: "pipeline".into(),
            version: "1.0.0".into(),
            toml_content: "[workflow]\nname = \"pipeline\"\n".into(),
            rules_count: 0,
            forked_from: None,
            visibility: visibility.into(),
            created_at: "2026-01-01T00:00:00Z".into(),
            updated_at: "2026-01-01T00:00:00Z".into(),
            revision: 1,
        }
    }

    /// Owner sees their own pipeline in every visibility state.
    #[test]
    fn owner_sees_own_pipeline_in_every_visibility() {
        let owner = viewer(OWNER, "user");
        for visibility in ["private", SHARED_VISIBILITY, "link"] {
            assert!(
                can_read_pipeline(&owner, &row(OWNER, visibility)),
                "owner must see their own {visibility} pipeline"
            );
        }
    }

    /// Another tenant's private pipeline is invisible.
    #[test]
    fn other_tenants_private_pipeline_is_invisible() {
        assert!(!can_read_pipeline(
            &viewer(OTHER, "user"),
            &row(OWNER, "private")
        ));
    }

    /// `link` is not a read grant: another tenant cannot see it without a
    /// minted share token (which is a separate capability).
    #[test]
    fn other_tenants_link_pipeline_is_invisible() {
        assert!(!can_read_pipeline(
            &viewer(OTHER, "user"),
            &row(OWNER, "link")
        ));
    }

    /// The `workspace` flag is the shared-library/template grant: visible to
    /// every signed-in user, including personal mode's pseudo-user.
    #[test]
    fn workspace_shared_pipeline_is_visible_to_any_signed_in_user() {
        assert!(can_read_pipeline(
            &viewer(OTHER, "user"),
            &row(OWNER, SHARED_VISIBILITY)
        ));
        assert!(can_read_pipeline(
            &viewer("default", "user"),
            &row(OWNER, SHARED_VISIBILITY)
        ));
    }

    /// Unknown/future visibility values fail closed as private.
    #[test]
    fn unknown_visibility_fails_closed() {
        for visibility in ["", "Public", "org", "workspace "] {
            assert!(
                !can_read_pipeline(&viewer(OTHER, "user"), &row(OWNER, visibility)),
                "unknown visibility {visibility:?} must not be shared"
            );
        }
    }

    /// The documented admin override sees every tenant's pipeline.
    #[test]
    fn admin_sees_every_tenant() {
        let admin = viewer("admin", "admin");
        for visibility in ["private", SHARED_VISIBILITY, "link"] {
            assert!(can_read_pipeline(&admin, &row(OWNER, visibility)));
        }
    }

    /// Write is owner-or-admin only; a workspace reader cannot write.
    #[test]
    fn only_owner_or_admin_may_write() {
        let owner = viewer(OWNER, "user");
        let other = viewer(OTHER, "user");
        let admin = viewer("admin", "admin");
        for visibility in ["private", SHARED_VISIBILITY, "link"] {
            assert!(can_write_pipeline(&owner, &row(OWNER, visibility)));
            assert!(can_write_pipeline(&admin, &row(OWNER, visibility)));
            assert!(!can_write_pipeline(&other, &row(OWNER, visibility)));
        }
    }
}
