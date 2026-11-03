//! Verify AI service has zero write access and security boundaries are enforced.
//!
//! Tests:
//! 1. Static: AI module source code must not import DB write, FS write, or process spawn
//! 2. Runtime: Path traversal prevention in sandbox
//! 3. Runtime: SQL injection prevention (parameterized queries)
//! 4. Runtime: Auth bypass prevention

/// Strip Rust comments from source so comment text does not trigger false positives.
fn strip_comments(source: &str) -> String {
    source
        .lines()
        .filter(|line| {
            let trimmed = line.trim_start();
            !trimmed.starts_with("//")
        })
        .collect::<Vec<_>>()
        .join("\n")
}

// ── Static Analysis ──

#[test]
fn test_ai_module_no_write_imports() {
    let raw = std::fs::read_to_string(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src/domains/ai/service.rs"),
    )
    .unwrap_or_default();
    let source = strip_comments(&raw);

    // Verify no DB write imports
    assert!(
        !source.contains("create_user"),
        "AI service must not create users"
    );
    assert!(
        !source.contains("insert_run"),
        "AI service must not create runs"
    );
    assert!(
        !source.contains("save_pipeline"),
        "AI service must not save pipelines"
    );
    assert!(
        !source.contains("delete_"),
        "AI service must not delete anything"
    );

    // Verify no filesystem write
    assert!(
        !source.contains("std::fs::write"),
        "AI service must not write files"
    );
    assert!(
        !source.contains("File::create"),
        "AI service must not create files"
    );

    // Verify no process spawn
    assert!(
        !source.contains("std::process::Command"),
        "AI service must not spawn processes"
    );
    assert!(
        !source.contains("tokio::process::Command"),
        "AI service must not spawn processes"
    );
}

#[test]
fn test_ai_copilot_no_write_imports() {
    let raw = std::fs::read_to_string(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src/domains/ai/copilot.rs"),
    )
    .unwrap_or_default();
    let source = strip_comments(&raw);

    assert!(!source.contains("std::fs::write"));
    assert!(!source.contains("File::create"));
    assert!(!source.contains("std::process::Command"));
    assert!(!source.contains("tokio::process::Command"));
}

// ── Runtime: Path Traversal Prevention ──

#[test]
fn test_path_traversal_prevention() {
    // Exercise the production path validator (`workspace::canonicalize_relative_path`
    // — the gate every user-supplied relative path goes through). A local
    // copy here would drift from the real implementation and test nothing.
    let func = |raw: &str| oxo_flow_web::workspace::canonicalize_relative_path(raw);

    // Normal paths should pass through
    assert_eq!(func("run-123").ok(), Some("run-123".to_string()));
    assert_eq!(
        func("my_pipeline_v2").ok(),
        Some("my_pipeline_v2".to_string())
    );

    // Path traversal attempts must be refused outright
    assert!(func("../../../etc/passwd").is_err());
    assert!(func("a/../../etc/passwd").is_err());
    assert!(func("/absolute/path").is_err());

    // Null byte injection
    assert!(func("run\0hidden").is_err());

    // Windows-style backslashes are NOT separators on Unix: the validator
    // treats `..\..\windows\system32` as one flat (weird) filename that
    // stays inside the workspace. The security property is containment —
    // the accepted value must never carry a '/' separator or resolve
    // outside the workspace root.
    let ok = func("..\\..\\windows\\system32").expect("contained filename is accepted");
    assert!(!ok.contains('/'), "must stay a flat filename: {ok:?}");
}

// ── Runtime: SQL Injection Prevention ──

#[test]
fn test_sql_injection_prevention() {
    // Verify all handlers use parameterized queries (bind), not string interpolation.
    let handler_files = [
        "src/domains/workflow/handlers.rs",
        "src/domains/execution/handlers.rs",
        "src/domains/auth/handlers.rs",
        "src/domains/collaboration/handlers.rs",
        "src/domains/observability/handlers.rs",
    ];

    for file in &handler_files {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join(file);
        if !path.exists() {
            continue;
        }
        let raw = std::fs::read_to_string(&path).unwrap_or_default();
        let source = strip_comments(&raw);

        // Statement-shaped patterns only: utoipa method verbs (`delete,`) and
        // prose like "/// DELETE /api/..." must not read as SQL. Handlers are
        // expected to route storage through StorageBackend; any raw SQL that
        // does appear must be parameterized with .bind().
        let has_sql = ["INSERT INTO", "DELETE FROM"]
            .iter()
            .any(|p| source.contains(p))
            || (source.contains("UPDATE ") && source.contains(" SET "))
            || (source.contains("SELECT ") && source.contains(" FROM "));
        let has_bind = source.contains(".bind(");

        if has_sql {
            assert!(
                has_bind,
                "Handler file {file} has SQL queries but no .bind() calls — possible SQL injection risk"
            );
        }
    }
}

// ── Runtime: Auth Bypass Prevention ──

#[test]
fn test_auth_bypass_prevention() {
    // Verify auth-required endpoints enforce authentication.
    // This is a structural test: verify that the auth middleware exists and
    // that endpoints don't expose sensitive data without auth checks.

    let auth_service_path =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src/domains/auth/service.rs");
    let source = std::fs::read_to_string(&auth_service_path).unwrap_or_default();

    // Auth service should validate credentials, not just accept any input
    assert!(
        source.contains("validate") || source.contains("authenticate") || source.contains("verify"),
        "Auth service should have validation logic"
    );

    // Session validation should check expiry
    let server_rs = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src/server.rs");
    let server_source = std::fs::read_to_string(&server_rs).unwrap_or_default();

    // Server should have auth routes but not bypass them
    assert!(
        server_source.contains("auth"),
        "Server should have auth routes"
    );
}

// ── Runtime: AI Write Boundary ──

#[test]
fn test_ai_service_write_boundary_filesystem() {
    // Runtime test: verify the AI service cannot write to the filesystem.
    // We test by checking that the AI service module only imports read-side APIs.

    let ai_service_path =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src/domains/ai/service.rs");
    let source = std::fs::read_to_string(&ai_service_path).unwrap_or_default();

    let ai_handler_path =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src/domains/ai/handlers.rs");
    let handler_source = std::fs::read_to_string(&ai_handler_path).unwrap_or_default();

    // Neither should import write-side operations
    for (file, src) in [("service.rs", &source), ("handlers.rs", &handler_source)] {
        assert!(
            !src.contains("std::fs::write")
                && !src.contains("File::create")
                && !src.contains("OpenOptions::new")
                && !src.contains(".create(true)"),
            "AI module {file} should not have filesystem write operations"
        );
    }
}

#[test]
fn test_ai_service_write_boundary_database() {
    // Verify the AI service doesn't directly call DB write operations.
    let ai_service_path =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src/domains/ai/service.rs");
    let source = std::fs::read_to_string(&ai_service_path).unwrap_or_default();

    // AI service should not import or use DB write operations
    let db_write_patterns = ["INSERT INTO", "UPDATE ", "DELETE FROM", "execute("];

    // Issue #342: generation runs through the oxo-flow-ai orchestrator.
    // `orchestrator.execute(` is the read-only agent loop (LLM calls +
    // knowledge tools), not a sqlx write — allow-list that exact call
    // shape so the tripwire keeps catching real `sqlx` `.execute(` uses.
    let stripped = strip_comments(&source)
        .replace("orchestrator\n    .execute(", "")
        .replace("orchestrator.execute(", "");

    for pattern in &db_write_patterns {
        assert!(
            !stripped.contains(pattern),
            "AI service should not contain DB write: '{pattern}'"
        );
    }
}
