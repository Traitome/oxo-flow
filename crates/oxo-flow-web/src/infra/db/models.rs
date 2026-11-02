use serde::{Deserialize, Serialize};
use sqlx::FromRow;

#[derive(Debug, Clone, FromRow, Serialize, Deserialize)]
pub struct UserRow {
    pub id: String,
    pub username: String,
    pub role: String,      // "admin" | "user" | "viewer"
    pub auth_type: String, // "password" | "oauth"
    pub os_user: Option<String>,
    pub password_hash: Option<String>,
    pub created_at: String,
}

#[derive(Debug, Clone, FromRow, Serialize, Deserialize)]
pub struct PipelineRow {
    pub id: String,
    pub user_id: String,
    pub name: String,
    pub version: String,
    pub toml_content: String,
    pub rules_count: i64,
    pub forked_from: Option<String>,
    pub visibility: String, // "private" | "workspace" | "link"
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, FromRow, Serialize, Deserialize)]
pub struct RunRow {
    pub id: String,
    pub user_id: String,
    /// Nullable: ad-hoc runs are not linked to a saved pipeline.
    pub pipeline_id: Option<String>,
    pub pipeline_snapshot: String,
    pub workflow_name: Option<String>,
    pub status: String, // "queued"|"running"|"paused"|"completed"|"failed"|"cancelled"
    pub phase: String,  // "parsing"|"validating"|"preparing"|"executing"|"reporting"
    /// Whether the run was launched in dry-run mode (plan-only, no rule
    /// shells). Persisted so run lists can flag dry runs instead of letting
    /// them pose as normal executions (issue #907 P3-14).
    pub dry_run: bool,
    pub pid: Option<i64>,
    pub workdir: Option<String>,
    /// JSON array of the validated `KEY=VALUE` config overrides the run was
    /// created with (`None` when the client sent none). Persisted so retry
    /// re-executes with the same configuration as the original run.
    pub config_overrides: Option<String>,
    pub started_at: Option<String>,
    pub finished_at: Option<String>,
    pub created_at: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct RunFilters {
    pub user_id: Option<String>,
    pub status: Option<String>,
    pub q: Option<String>,
    pub cursor: Option<String>,
    pub limit: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CursorPage<T> {
    pub items: Vec<T>,
    pub next_cursor: Option<String>,
    pub total: u64,
}

/// Placeholder style for assembled run-list fragments: SQLite takes
/// anonymous `?` binds, PostgreSQL numbered `$N` binds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SqlPlaceholder {
    QuestionMark,
    DollarNumber,
}

/// SQL fragments for one run-listing page (`GET /api/runs`, #948 P2-6): the
/// row SELECT (filters + cursor) and the filter-only COUNT (audit #665),
/// each with its ordered binds.
///
/// Lives beside [`RunFilters`] — the filter type crosses the
/// [`super::StorageBackend`] boundary, so the single implementation of the
/// filter semantics is shared by the handler-level tests and by BOTH storage
/// backends instead of being hand-copied per backend.
///
/// All fragments are static strings pushed by the assembler — every piece of
/// user input rides in the binds — which is what makes the assembled SQL safe
/// to pass to `sqlx::AssertSqlSafe` at the call site.
#[derive(Debug, Clone)]
pub struct RunListQuery {
    /// `SELECT * FROM runs<page_where> ORDER BY created_at DESC LIMIT <limit + 1>`
    /// — ready for `format!` substitution of the numeric LIMIT.
    pub page_where_sql: String,
    /// Binds for the SELECT, in placeholder order (user scoping → status
    /// → q → cursor).
    pub page_binds: Vec<String>,
    /// `SELECT COUNT(*) FROM runs<filter_where>` — cursor deliberately
    /// excluded so the total stays stable across pages.
    pub filter_where_sql: String,
    /// Binds for the COUNT (user scoping → status → q).
    pub filter_binds: Vec<String>,
}

/// Assemble the run-listing SELECT and COUNT fragments (`?` binds, SQLite).
///
/// See [`assemble_run_list_query_dialect`] for the shared semantics.
pub fn assemble_run_list_query(filters: &RunFilters) -> RunListQuery {
    assemble_run_list_query_dialect(SqlPlaceholder::QuestionMark, filters)
}

/// Assemble the run-listing SELECT and COUNT fragments (`$N` binds, PostgreSQL).
pub fn assemble_run_list_query_pg(filters: &RunFilters) -> RunListQuery {
    assemble_run_list_query_dialect(SqlPlaceholder::DollarNumber, filters)
}

/// Assemble the run-listing SELECT and COUNT fragments from the filters.
///
/// Clause order is part of the wire-stable behavior the tests pin down:
/// `user_id = ?` (ownership, issue #82 P0-4) → `status = ?` →
/// `(workflow_name LIKE ? OR id LIKE ?)` → page-only `created_at < ?`.
/// An unrecognized `status` is silently dropped (the handler's long-standing
/// contract), as is a whitespace-only `q`. The row cap is NOT folded in
/// here — the caller formats the numeric `LIMIT` itself (fetching
/// `limit + 1`), so the fragments stay cap-agnostic.
pub fn assemble_run_list_query_dialect(
    style: SqlPlaceholder,
    filters: &RunFilters,
) -> RunListQuery {
    // Clauses carry a `{p}` marker per placeholder; rendering swaps in the
    // dialect's token. `q` binds twice (workflow_name + id), so the marker
    // count per clause decides the bind count.
    const USER_CLAUSE: &str = "user_id = {p}";
    const STATUS_CLAUSE: &str = "status = {p}";
    const Q_CLAUSE: &str = "(workflow_name LIKE {p} OR id LIKE {p})";
    const CURSOR_CLAUSE: &str = "created_at < {p}";

    // Clauses in wire-stable order; the cursor, when present, is appended
    // as the trailing page-only clause — it applies to the row SELECT but
    // NOT to the total COUNT, which reports the whole matching population
    // and must not shrink with every page (audit #665).
    let mut clauses: Vec<(&str, Vec<String>)> = Vec::new();
    if let Some(user_id) = &filters.user_id {
        clauses.push((USER_CLAUSE, vec![user_id.clone()]));
    }
    if let Some(status) = &filters.status
        && matches!(
            status.as_str(),
            "queued" | "running" | "paused" | "completed" | "failed" | "cancelled"
        )
    {
        clauses.push((STATUS_CLAUSE, vec![status.clone()]));
    }
    if let Some(q) = &filters.q
        && !q.trim().is_empty()
    {
        let pattern = format!("%{}%", q.trim());
        clauses.push((Q_CLAUSE, vec![pattern.clone(), pattern]));
    }
    let page_len = clauses.len();
    if let Some(cursor) = &filters.cursor {
        clauses.push((CURSOR_CLAUSE, vec![cursor.clone()]));
    }

    // Render a clause list into ` WHERE a AND b` (empty string when no
    // clauses), numbering placeholders per dialect. `counter` continues
    // across clauses; the filter fragment restarts at 1 because the filter
    // binds are a prefix of the page binds.
    fn render_where(clauses: &[(&str, Vec<String>)], upto: usize, style: SqlPlaceholder) -> String {
        if upto == 0 {
            return String::new();
        }
        let mut sql = String::from(" WHERE ");
        let mut counter = 1usize;
        for (i, (clause, _)) in clauses.iter().enumerate().take(upto) {
            if i > 0 {
                sql.push_str(" AND ");
            }
            for (j, part) in clause.split("{p}").enumerate() {
                if j > 0 {
                    match style {
                        SqlPlaceholder::QuestionMark => sql.push('?'),
                        SqlPlaceholder::DollarNumber => {
                            sql.push_str(&format!("${counter}"));
                            counter += 1;
                        }
                    }
                }
                sql.push_str(part);
            }
        }
        sql
    }
    let page_where_sql = render_where(&clauses, clauses.len(), style);
    let filter_where_sql = render_where(&clauses, page_len, style);
    let page_binds = clauses
        .iter()
        .flat_map(|(_, b)| b.iter().cloned())
        .collect();
    let filter_binds = clauses[..page_len]
        .iter()
        .flat_map(|(_, b)| b.iter().cloned())
        .collect();

    RunListQuery {
        page_where_sql,
        page_binds,
        filter_where_sql,
        filter_binds,
    }
}

#[cfg(test)]
mod run_list_query_tests {
    use super::*;

    fn filters(
        user_id: Option<&str>,
        status: Option<&str>,
        q: Option<&str>,
        cursor: Option<&str>,
    ) -> RunFilters {
        RunFilters {
            user_id: user_id.map(str::to_string),
            status: status.map(str::to_string),
            q: q.map(str::to_string),
            cursor: cursor.map(str::to_string),
            limit: 100,
        }
    }

    #[test]
    fn no_filters_yields_empty_where_and_no_binds() {
        let q = assemble_run_list_query(&filters(None, None, None, None));
        assert_eq!(q.page_where_sql, "");
        assert!(q.page_binds.is_empty());
        assert_eq!(q.filter_where_sql, "");
        assert!(q.filter_binds.is_empty());
    }

    #[test]
    fn non_admin_scope_adds_user_id_clause() {
        // Ownership scoping (issue #82 P0-4): non-admins see their own runs
        // only — first clause, before any user-supplied filter.
        let q = assemble_run_list_query(&filters(Some("u1"), Some("failed"), Some("rna"), None));
        assert_eq!(
            q.filter_where_sql,
            " WHERE user_id = ? AND status = ? AND (workflow_name LIKE ? OR id LIKE ?)"
        );
        assert_eq!(q.filter_binds, vec!["u1", "failed", "%rna%", "%rna%"]);
    }

    #[test]
    fn admin_sees_whole_fleet_no_user_clause() {
        let q = assemble_run_list_query(&filters(None, Some("running"), None, None));
        assert_eq!(q.filter_where_sql, " WHERE status = ?");
        assert_eq!(q.filter_binds, vec!["running"]);
    }

    #[test]
    fn unrecognized_status_is_silently_dropped() {
        let q = assemble_run_list_query(&filters(None, Some("DROP TABLE"), None, None));
        assert_eq!(q.filter_where_sql, "");
        assert!(q.filter_binds.is_empty());
    }

    #[test]
    fn empty_or_whitespace_q_is_dropped() {
        let q = assemble_run_list_query(&filters(None, None, Some("   "), None));
        assert_eq!(q.filter_where_sql, "");
        // A meaningful term still binds twice: LIKE pattern for both columns.
        let q2 = assemble_run_list_query(&filters(None, None, Some("  rna  "), None));
        assert_eq!(
            q2.filter_where_sql,
            " WHERE (workflow_name LIKE ? OR id LIKE ?)"
        );
        assert_eq!(q2.filter_binds, vec!["%rna%", "%rna%"]);
    }

    #[test]
    fn cursor_applies_to_select_but_not_count() {
        // Audit #665: the cursor is a page bound, not a filter — the COUNT
        // must not shrink with every page.
        let q = assemble_run_list_query(&filters(
            Some("u1"),
            None,
            None,
            Some("2026-01-01T00:00:00Z"),
        ));
        assert_eq!(q.page_where_sql, " WHERE user_id = ? AND created_at < ?");
        assert_eq!(q.page_binds, vec!["u1", "2026-01-01T00:00:00Z"]);
        assert_eq!(q.filter_where_sql, " WHERE user_id = ?");
        assert_eq!(q.filter_binds, vec!["u1"]);
    }

    #[test]
    fn full_assembled_select_sql_is_parameterized() {
        // The handler concatenates `page_where_sql` into the final SELECT —
        // assert the whole string contains nothing but static fragments and
        // the numeric limit.
        let q =
            assemble_run_list_query(&filters(Some("u1"), Some("queued"), Some("x"), Some("c1")));
        let sql = format!(
            "SELECT * FROM runs{} ORDER BY created_at DESC LIMIT {}",
            q.page_where_sql,
            100 + 1
        );
        assert_eq!(
            sql,
            "SELECT * FROM runs WHERE user_id = ? AND status = ? AND (workflow_name LIKE ? OR id LIKE ?) AND created_at < ? ORDER BY created_at DESC LIMIT 101"
        );
        assert_eq!(q.page_binds, vec!["u1", "queued", "%x%", "%x%", "c1"]);
        // A quote in the search term never reaches the SQL text — it rides
        // in the bind.
        let evil =
            assemble_run_list_query(&filters(None, None, Some("'; DROP TABLE runs;--"), None));
        assert!(!evil.page_where_sql.contains('\''));
        assert_eq!(
            evil.page_binds,
            vec!["%'; DROP TABLE runs;--%", "%'; DROP TABLE runs;--%"]
        );
    }

    #[test]
    fn cursor_only_yields_bare_page_clause() {
        // The one filter-shape the other tests don't pin: a cursor with no
        // other filters — the admin's Load-More path.
        let q = assemble_run_list_query(&filters(None, None, None, Some("2026-01-01T00:00:00Z")));
        assert_eq!(q.page_where_sql, " WHERE created_at < ?");
        assert_eq!(q.page_binds, vec!["2026-01-01T00:00:00Z"]);
        assert_eq!(q.filter_where_sql, "");
        assert!(q.filter_binds.is_empty());
    }

    #[test]
    fn postgres_dialect_numbers_its_binds() {
        // The PostgreSQL twin renders the SAME clause order with numbered
        // `$N` placeholders, sharing every semantic branch with the `?`
        // dialect above. Two q binds consume consecutive numbers inside one
        // clause; the filter fragment restarts numbering at $1 because its
        // binds are a prefix of the page binds.
        let q = assemble_run_list_query_pg(&filters(Some("u1"), Some("failed"), Some("rna"), None));
        assert_eq!(
            q.filter_where_sql,
            " WHERE user_id = $1 AND status = $2 AND (workflow_name LIKE $3 OR id LIKE $4)"
        );
        assert_eq!(q.filter_binds, vec!["u1", "failed", "%rna%", "%rna%"]);
        assert_eq!(
            q.page_where_sql,
            " WHERE user_id = $1 AND status = $2 AND (workflow_name LIKE $3 OR id LIKE $4)"
        );
        assert_eq!(q.filter_binds, q.page_binds);

        // Cursor-only shape: the page clause numbers from $1, the COUNT is
        // filter-only and therefore empty.
        let c =
            assemble_run_list_query_pg(&filters(None, None, None, Some("2026-01-01T00:00:00Z")));
        assert_eq!(c.page_where_sql, " WHERE created_at < $1");
        assert_eq!(c.filter_where_sql, "");
        assert!(c.filter_binds.is_empty());
    }
}

#[derive(Debug, Clone, FromRow, Serialize, Deserialize)]
pub struct RunNodeRow {
    pub run_id: String,
    pub rule_name: String,
    pub status: String,
    pub exit_code: Option<i32>,
    pub started_at: Option<String>,
    pub finished_at: Option<String>,
}

#[derive(Debug, Clone, FromRow, Serialize, Deserialize)]
pub struct SessionRow {
    pub token: String,
    pub user_id: String,
    pub created_at: String,
    pub expires_at: String,
}

#[derive(Debug, Clone, FromRow, Serialize, Deserialize)]
pub struct TemplateRow {
    pub id: String,
    pub name: String,
    pub category: String,
    pub description: String,
    pub tags: String, // JSON array stored as string
    pub toml_content: String,
    pub is_system: i64,
    pub created_by: Option<String>,
    pub usage_count: i64,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, FromRow, Serialize, Deserialize)]
pub struct ShareRow {
    pub id: String,
    pub pipeline_id: String,
    pub owner_id: String,
    pub token: String,
    pub visibility: String,
    pub expires_at: Option<String>,
    pub created_at: String,
}

#[derive(Debug, Clone, FromRow, Serialize, Deserialize)]
pub struct AuditLogRow {
    pub id: String,
    pub user_id: String,
    pub action: String,
    pub target: String,
    pub result: String,
    pub metadata: Option<String>,
    pub timestamp: String,
}

#[derive(Debug, Clone, FromRow, Serialize, Deserialize)]
pub struct ClusterRow {
    pub id: String,
    pub name: String,
    pub ssh_host: String,
    pub ssh_port: i64,
    pub ssh_user: Option<String>,
    pub ssh_key: Option<String>,
    pub scheduler: Option<String>,
    pub remote_dir: Option<String>,
    pub enabled: bool,
    pub created_at: String,
}

#[derive(Debug, Clone, FromRow, Serialize, Deserialize)]
pub struct AiProviderConfigRow {
    pub id: String,
    /// `None` selects the process-wide server configuration; otherwise this
    /// is the owning user. NULL and populated IDs intentionally coexist.
    pub user_id: Option<String>,
    pub provider: String,
    pub api_key: Option<String>,
    pub api_url: Option<String>,
    pub model: Option<String>,
    pub search_enabled: i64,
    pub monitor_enabled: i64,
    pub auto_retry_enabled: i64,
    pub max_correction_rounds: i64,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, FromRow, Serialize, Deserialize)]
pub struct ChatSessionRow {
    pub id: String,
    pub user_id: String,
    pub title: String,
    pub created_at: String,
    pub updated_at: String,
}
// v0.8 AI Companion models

#[derive(Debug, Clone, FromRow, Serialize, Deserialize)]
pub struct ApiKeyRow {
    pub id: String,
    pub user_id: String,
    pub name: String,
    pub key_hash: String,
    pub created_at: String,
    pub last_used_at: Option<String>,
    pub revoked: i64,
}

#[derive(Debug, Clone, FromRow, Serialize, Deserialize)]
pub struct OAuthStateRow {
    pub state: String,
    pub created_at: String,
}

#[derive(Debug, Clone, FromRow, Serialize, Deserialize)]
pub struct ChatMessageRow {
    pub id: String,
    pub session_id: String,
    pub role: String,
    pub content: String,
    pub meta: Option<String>,
    pub created_at: String,
}

#[derive(Debug, Clone, FromRow, Serialize, Deserialize)]
pub struct WebhookConfigRow {
    pub id: i64,
    pub url: String,
    pub secret: Option<String>,
    pub enabled: i64,
    pub events: String,
    pub signature_scheme: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, FromRow, Serialize, Deserialize)]
pub struct PipelineRevisionRow {
    pub id: String,
    pub pipeline_id: String,
    pub user_id: String,
    pub version: String,
    pub toml_content: String,
    pub created_at: String,
}
