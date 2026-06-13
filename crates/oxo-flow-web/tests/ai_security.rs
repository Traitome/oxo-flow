/// Verify AI service has zero write access.
/// The AI module must NOT import DB write, filesystem write, or process spawn.
///
/// This test reads the source of `service.rs` and asserts that no write-side
/// imports are present. It is a static check (not a runtime test) because
/// the security boundary is enforced at the import level.

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

    // Copilot is pure prompt assembly -- should not touch DB or filesystem
    assert!(!source.contains("std::fs::write"));
    assert!(!source.contains("File::create"));
    assert!(!source.contains("std::process::Command"));
    assert!(!source.contains("tokio::process::Command"));
}
