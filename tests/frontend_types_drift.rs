//! Frontend API type drift gate: `frontend/src/api/types.ts` hand-mirrors
//! the web API's response structs, so a field added or renamed in Rust
//! would otherwise break pages silently. Each pinned pair must declare the
//! same top-level field names. Nested object shapes are out of scope for
//! this gate — new pinned pairs go in `PINS` as responses gain load-bearing
//! surface.

use std::collections::BTreeSet;
use std::fs;
use std::path::PathBuf;

/// (Rust source file, Rust struct, TS interface). The TS side may mark
/// fields optional — only the name set is compared.
const PINS: &[(&str, &str, &str)] = &[
    (
        "crates/oxo-flow-web/src/domains/observability/types.rs",
        "HealthResponse",
        "HealthResponse",
    ),
    (
        "crates/oxo-flow-web/src/domains/observability/types.rs",
        "SystemInfoResponse",
        "SystemInfo",
    ),
    (
        "crates/oxo-flow-web/src/domains/observability/types.rs",
        "RuntimeMetricsResponse",
        "RuntimeMetrics",
    ),
    (
        "crates/oxo-flow-web/src/domains/workflow/types.rs",
        "ValidateResponse",
        "ValidateResponse",
    ),
    (
        "crates/oxo-flow-web/src/domains/workflow/types.rs",
        "Pipeline",
        "Pipeline",
    ),
    (
        "crates/oxo-flow-web/src/domains/workflow/types.rs",
        "Template",
        "Template",
    ),
    (
        "crates/oxo-flow-web/src/domains/execution/types.rs",
        "RunStatusResponse",
        "RunStatus",
    ),
    (
        "crates/oxo-flow-web/src/domains/execution/types.rs",
        "DiagnosticsResponse",
        "Diagnostics",
    ),
];

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn strip_comments(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '/' if chars.peek() == Some(&'/') => {
                for n in chars.by_ref() {
                    if n == '\n' {
                        out.push('\n');
                        break;
                    }
                }
            }
            '/' if chars.peek() == Some(&'*') => {
                chars.next();
                while let Some(n) = chars.next() {
                    if n == '*' && chars.peek() == Some(&'/') {
                        chars.next();
                        break;
                    }
                }
            }
            _ => out.push(c),
        }
    }
    out
}

/// Body between the braces that open after `anchor` (depth-matched).
fn braced_body(text: &str, anchor: &str) -> Option<String> {
    let start = text.find(anchor)?;
    let open = text[start..].find('{')? + start;
    let mut depth = 0i32;
    for (i, c) in text[open..].char_indices() {
        match c {
            '{' => depth += 1,
            '}' => {
                depth -= 1;
                if depth == 0 {
                    return Some(text[open + 1..open + i].to_string());
                }
            }
            _ => {}
        }
    }
    None
}

fn rust_struct_fields(path: &PathBuf, name: &str) -> BTreeSet<String> {
    let text = strip_comments(&fs::read_to_string(path).unwrap_or_else(|e| {
        panic!("cannot read {}: {e}", path.display());
    }));
    // Drop `#[...]` attributes: their commas would otherwise split fields.
    let mut no_attrs = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '#' && chars.peek() == Some(&'[') {
            let mut depth = 0i32;
            for n in chars.by_ref() {
                match n {
                    '[' => depth += 1,
                    ']' => {
                        depth -= 1;
                        if depth == 0 {
                            break;
                        }
                    }
                    _ => {}
                }
            }
        } else {
            no_attrs.push(c);
        }
    }
    let body = braced_body(&no_attrs, &format!("pub struct {name}"))
        .unwrap_or_else(|| panic!("struct {name} not found in {}", path.display()));
    assert!(
        !body.contains("#[serde("),
        "pin assumption broken for {name}: a serde attribute changes the field contract — \
         extend the extraction or unpin the pair",
    );
    body.split(',')
        .filter_map(|field| {
            let field = field.trim();
            let field = field.strip_prefix("pub ")?;
            Some(field.split(':').next()?.trim().to_string())
        })
        .collect()
}

fn ts_interface_fields(types_ts: &str, name: &str) -> BTreeSet<String> {
    let body = braced_body(types_ts, &format!("export interface {name}"))
        .unwrap_or_else(|| panic!("interface {name} not found in frontend/src/api/types.ts"));
    let mut fields = BTreeSet::new();
    let mut depth = 0i32;
    let mut segment = String::new();
    for c in body.chars() {
        match c {
            '{' | '[' | '(' => {
                depth += 1;
                segment.push(c);
            }
            '}' | ']' | ')' => {
                depth -= 1;
                segment.push(c);
            }
            ';' if depth == 0 => {
                if let Some(field) = ts_field_name(&segment) {
                    fields.insert(field);
                }
                segment.clear();
            }
            _ => segment.push(c),
        }
    }
    if let Some(field) = ts_field_name(&segment) {
        fields.insert(field);
    }
    fields
}

fn ts_field_name(segment: &str) -> Option<String> {
    let segment = segment.trim();
    let segment = segment.strip_prefix("readonly ").unwrap_or(segment);
    let name = segment.split(':').next()?.trim();
    let name = name.strip_suffix('?').unwrap_or(name);
    if name.is_empty() {
        None
    } else {
        Some(name.to_string())
    }
}

#[test]
fn frontend_api_types_match_the_rust_response_contracts() {
    let root = repo_root();
    let types_ts = strip_comments(
        &fs::read_to_string(root.join("frontend/src/api/types.ts"))
            .expect("frontend/src/api/types.ts readable"),
    );
    assert!(
        !PINS.is_empty(),
        "the drift gate pins at least one response pair"
    );
    let mut failures = Vec::new();
    for (rust_file, rust_name, ts_name) in PINS {
        let rust_fields = rust_struct_fields(&root.join(rust_file), rust_name);
        let ts_fields = ts_interface_fields(&types_ts, ts_name);
        let missing_in_ts: Vec<_> = rust_fields.difference(&ts_fields).collect();
        let unknown_to_rust: Vec<_> = ts_fields.difference(&rust_fields).collect();
        if !missing_in_ts.is_empty() || !unknown_to_rust.is_empty() {
            failures.push(format!(
                "{rust_name} ↔ {ts_name}: missing in types.ts {missing_in_ts:?}; unknown to Rust {unknown_to_rust:?}"
            ));
        }
    }
    assert!(
        failures.is_empty(),
        "frontend/src/api/types.ts drifted from the Rust response contracts:\n{}",
        failures.join("\n")
    );
}
