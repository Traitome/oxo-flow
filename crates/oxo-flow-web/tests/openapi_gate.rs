//! OpenAPI drift gate (issue #82 P1-13).
//!
//! The spec is CODE-GENERATED from the `#[utoipa::path]` annotations on the
//! route handlers — there is no static openapi.json to drift from. This test
//! is the gate: it asserts that every route registered in
//! `crate::server::build_router` is present in the generated spec.
//!
//! Adding a new API route REQUIRES:
//!   1. a `#[utoipa::path(...)]` annotation on the handler,
//!   2. an entry in the `paths(...)` list of `src/openapi.rs` (the spec only
//!      documents annotated handlers that are listed there), and
//!   3. an entry in [`ALL_ROUTES`] below (the test fails otherwise).
//!
//! `ALL_ROUTES` itself is mechanically verified against the router source:
//! [`router_api_paths_from_source`] extracts every `path` literal passed to
//! `.route(...)` in `src/server.rs`, so a route registered without a listing
//! (or a stale listing for a removed route) fails even when both sides would
//! otherwise drift consistently. axum does not expose route enumeration, so
//! the source scan is the mechanical truth. Dynamic (`&format!(...)`) mount
//! points are the documented exception: [`router_api_paths_from_source`]
//! collects them separately and the gate pins the exact expected set.

/// Every API path registered in `crate::server::build_router`, in the
/// router's own order. Each entry must exist in the generated spec, and the
/// set must match the `.route(...)` literals in `src/server.rs` exactly
/// (mechanically verified — see [`router_api_paths_from_source`]).
const ALL_ROUTES: &[&str] = &[
    // workflow
    "/api/pipelines/parse",
    "/api/pipelines/validate",
    "/api/pipelines/prepare",
    "/api/pipelines/dag",
    "/api/pipelines/format",
    "/api/pipelines/lint",
    "/api/pipelines/stats",
    "/api/pipelines/diff",
    "/api/pipelines/export",
    "/api/pipelines/search",
    "/api/plugins/validate",
    "/api/pipelines",
    "/api/pipelines/{id}",
    "/api/pipelines/{id}/revisions",
    "/api/pipelines/{id}/revisions/{rev}",
    "/api/pipelines/{id}/rollback",
    // runs
    "/api/runs",
    "/api/runs/{id}",
    "/api/runs/{id}/status",
    "/api/runs/{id}/dag-status",
    "/api/runs/{id}/diagnostics",
    "/api/runs/{id}/instances",
    "/api/runs/{id}/clean",
    "/api/runs/{id}/resume-checkpoint",
    "/api/runs/{id}/apply-repair",
    "/api/runs/{id}/repair",
    "/api/runs/{id}/logs",
    "/api/runs/{id}/results",
    "/api/runs/{id}/retry",
    "/api/runs/{id}/cancel",
    "/api/runs/{id}/pause",
    "/api/runs/{id}/resume",
    "/api/runs/{id}/preview",
    "/api/runs/{id}/protected-drift",
    "/api/runs/{id}/provenance-verify",
    "/api/runs/{id}/ai-status",
    "/api/runs/{id}/report",
    "/api/runs/{id}/report/ask",
    "/api/runs/{id}/report/visualize",
    // files
    "/api/runs/{id}/files",
    "/api/files",
    // data
    "/api/data/analyze",
    "/api/data/reference",
    "/api/data/perceive",
    "/api/data/reference/status",
    "/api/data/samplesheet/parse",
    "/api/data/verify",
    // templates
    "/api/templates",
    "/api/templates/{id}",
    // auth
    "/api/auth/login",
    "/api/auth/logout",
    "/api/auth/me",
    "/api/users",
    "/api/users/{id}",
    "/api/auth/oauth/authorize",
    "/api/auth/oauth/callback",
    "/api/auth/keys",
    "/api/auth/keys/{id}",
    // license
    "/api/license",
    "/api/license/upload",
    // chat
    "/api/chat/send",
    "/api/chat/send/json",
    "/api/chat/sessions",
    // dag edit
    "/api/pipeline/{id}/command",
    "/api/pipeline/{id}/undo",
    "/api/pipeline/{id}/redo",
    // ai
    "/api/ai/translate",
    "/api/ai/translate/stream",
    "/api/ai/explain",
    "/api/ai/interpret",
    "/api/ai/optimize",
    "/api/ai/config",
    "/api/ai/test",
    "/api/knowledge/tools",
    "/api/knowledge/skills",
    "/api/ai/config/user",
    "/api/ai/config/server",
    "/api/ai/config/effective",
    // clusters
    "/api/clusters",
    "/api/clusters/{id}",
    "/api/clusters/{id}/probe",
    // collaboration
    "/api/pipelines/{id}/fork",
    "/api/pipelines/{id}/share",
    "/api/pipelines/import",
    "/api/share/{token}",
    // observability
    "/api/health",
    "/api/system",
    "/api/openapi.json",
    "/api/metrics",
    "/api/events",
    "/api/events/ticket",
    "/api/audit",
    "/api/audit/chain/export",
    "/api/audit/chain/verify",
    "/api/quota",
    "/api/webhook",
    // hpc
    "/api/hpc",
];

/// Dynamic mount points registered with a computed path (`&format!(...)`).
/// These cannot be extracted as literals; the gate pins the exact expected
/// set so a new dynamic registration must be acknowledged here (and its
/// concrete paths hand-listed in [`ALL_ROUTES`]).
const DYNAMIC_MOUNT_POINTS: &[&str] = &["&format!(\"{base}/\")"];

/// Extract every path passed to `.route(...)` in `src/server.rs` by scanning
/// the source text. Returns `(literal paths, dynamic path expressions)` —
/// the former filtered to API routes by the caller, the latter pinned by
/// [`DYNAMIC_MOUNT_POINTS`].
fn router_route_path_literals() -> (Vec<String>, Vec<String>) {
    let src = include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/src/server.rs"));
    let mut literals = Vec::new();
    let mut dynamic = Vec::new();

    for (idx, _) in src.match_indices(".route(") {
        let rest = &src[idx + ".route(".len()..];
        let rest = rest.trim_start();
        if let Some(quoted) = rest.strip_prefix('"') {
            // String literal path: read to the closing quote (paths never
            // contain escapes; `{id}` params are plain braces).
            let end = quoted.find('"').expect("route path literal is terminated");
            literals.push(quoted[..end].to_string());
        } else {
            // Dynamic expression: capture up to the argument-separating
            // comma at paren depth 0 for the pinned-set assertion.
            let mut depth = 0usize;
            let end = rest
                .char_indices()
                .find(|(_, c)| match c {
                    '(' | '[' => {
                        depth += 1;
                        false
                    }
                    ')' | ']' => {
                        depth = depth.saturating_sub(1);
                        false
                    }
                    ',' if depth == 0 => true,
                    _ => false,
                })
                .map(|(i, _)| i)
                .unwrap_or(rest.len());
            dynamic.push(rest[..end].trim().to_string());
        }
    }
    (literals, dynamic)
}

/// The gate's foundation: the mechanically extracted `.route(...)` literals
/// must equal [`ALL_ROUTES`] exactly (both directions), and dynamic mounts
/// must be the documented set. This is what makes "the hand list drifted
/// consistently on both sides" impossible to hide behind.
#[test]
fn all_routes_matches_the_router_source() {
    let (literals, dynamic) = router_route_path_literals();

    let mut dynamic_sorted = dynamic.clone();
    dynamic_sorted.sort();
    let mut expected_dynamic: Vec<&str> = DYNAMIC_MOUNT_POINTS.to_vec();
    expected_dynamic.sort_unstable();
    assert_eq!(
        dynamic_sorted, expected_dynamic,
        "dynamic route mount points changed — update DYNAMIC_MOUNT_POINTS (and hand-list \
         any new API paths in ALL_ROUTES)"
    );

    // API routes = literals under /api (static SPA assets are outside this
    // gate's scope by design).
    let mut source_api: Vec<String> = literals
        .into_iter()
        .filter(|p| p.starts_with("/api"))
        .collect();
    source_api.sort();
    source_api.dedup();

    let mut listed: Vec<&str> = ALL_ROUTES.to_vec();
    listed.sort_unstable();

    let missing: Vec<&str> = source_api
        .iter()
        .filter(|p| !listed.contains(&p.as_str()))
        .map(String::as_str)
        .collect();
    let stale: Vec<&str> = listed
        .iter()
        .filter(|p| !source_api.iter().any(|s| s == *p))
        .copied()
        .collect();
    assert!(
        missing.is_empty() && stale.is_empty(),
        "ALL_ROUTES does not match src/server.rs route registrations.\n\
         registered but unlisted (add to ALL_ROUTES):\n  {}\n\
         listed but not registered (remove from ALL_ROUTES):\n  {}",
        missing.join("\n  "),
        stale.join("\n  ")
    );
}

/// The spec is mode-aware: `/api/hpc` is mounted only in hpc mode, so only
/// that mode may advertise it (a generated client would otherwise call a
/// route that answers 404).
#[test]
fn hpc_route_is_advertised_only_in_hpc_mode() {
    let hpc: serde_json::Value = serde_json::from_str(&oxo_flow_web::openapi::spec_json("hpc"))
        .expect("generated spec must parse as JSON");
    assert!(
        hpc["paths"]["/api/hpc"].is_object(),
        "hpc mode must advertise /api/hpc"
    );

    for mode in ["personal", "team"] {
        let spec: serde_json::Value = serde_json::from_str(&oxo_flow_web::openapi::spec_json(mode))
            .expect("generated spec must parse as JSON");
        assert!(
            spec["paths"].get("/api/hpc").is_none(),
            "{mode} mode must not advertise /api/hpc"
        );
        assert!(
            spec["paths"]["/api/runs"].is_object(),
            "{mode} spec must still carry the common routes"
        );
    }
}

/// Serialize the generated spec, parse it back, and assert the shape.
#[test]
fn generated_spec_is_valid_json_and_openapi_3_1() {
    let json = oxo_flow_web::openapi::spec_json("hpc");

    let spec: serde_json::Value =
        serde_json::from_str(&json).expect("generated spec must parse as JSON");

    assert_eq!(spec["openapi"], "3.1.0", "spec must be OpenAPI 3.1.0");
    // Audit M2: explicit metadata — utoipa's derived contact parsed the
    // workspace authors URL as an email ("http") and the license name was
    // empty because the crate uses `license-file`.
    assert_eq!(
        spec["info"]["contact"]["url"],
        "https://github.com/Traitome/oxo-flow"
    );
    assert!(
        spec["info"]["contact"].get("email").is_none(),
        "the contact must not carry a bogus derived email: {}",
        spec["info"]["contact"]
    );
    assert_eq!(
        spec["info"]["license"]["name"],
        "Academic / Commercial (dual)"
    );
    assert!(
        spec["paths"].is_object(),
        "spec must contain a paths object"
    );
    assert!(
        spec["components"]["schemas"].is_object(),
        "spec must contain a components.schemas object"
    );
}

/// The drift gate: every route in the router must appear in the spec.
#[test]
fn every_router_route_is_in_the_generated_spec() {
    let spec: serde_json::Value = serde_json::from_str(&oxo_flow_web::openapi::spec_json("hpc"))
        .expect("generated spec must parse as JSON");

    let paths = spec["paths"].as_object().expect("paths must be an object");

    let mut missing: Vec<&str> = ALL_ROUTES
        .iter()
        .copied()
        .filter(|route| !paths.contains_key(*route))
        .collect();
    missing.sort_unstable();
    assert!(
        missing.is_empty(),
        "routes registered in build_router but missing from the generated OpenAPI spec \
         (missing #[utoipa::path] annotation or ALL_ROUTES drift):\n  {}",
        missing.join("\n  ")
    );

    // No path may be in the spec that the router does not register (the spec
    // is generated from the same handlers, so this catches stale listings).
    let unexpected: Vec<&str> = paths
        .keys()
        .filter(|route| !ALL_ROUTES.contains(&route.as_str()))
        .map(String::as_str)
        .collect();
    assert!(
        unexpected.is_empty(),
        "spec contains paths not registered in build_router (remove stale entries):\n  {}",
        unexpected.join("\n  ")
    );

    // Total operation count guard: every handler method is documented.
    let operation_count = paths
        .values()
        .flat_map(|item| {
            item.as_object()
                .expect("path item must be an object")
                .keys()
        })
        .filter(|method| {
            matches!(
                method.as_str(),
                "get" | "post" | "put" | "delete" | "patch" | "head" | "options" | "trace"
            )
        })
        .count();
    assert!(
        operation_count >= 102,
        "expected >= 102 documented operations (one per route handler), got {operation_count}"
    );
}

/// Report finding T-F2a: session tokens are opaque server-side ids, not
/// JWTs. The HTTP bearer shape (the part client generators consume) must
/// stay, but the spec must not claim a JWT format.
#[test]
fn bearer_auth_does_not_declare_a_jwt_format() {
    // The full surface lives behind the hpc mode gate (spec_json takes the
    // serving mode since the /api/hpc gating change); the security scheme is
    // mode-independent, so any mode projects it.
    let spec: serde_json::Value = serde_json::from_str(&oxo_flow_web::openapi::spec_json("hpc"))
        .expect("generated spec must parse as JSON");

    let scheme = &spec["components"]["securitySchemes"]["bearerAuth"];
    assert_eq!(scheme["type"], "http", "bearerAuth must remain http auth");
    assert_eq!(scheme["scheme"], "bearer", "bearerAuth must remain bearer");
    assert_ne!(
        scheme["bearerFormat"], "JWT",
        "session tokens are opaque, not JWTs: {scheme}"
    );
}

/// Body-carrying POSTs must declare their request body (issue #324 F-4):
/// a handler that parses a JSON payload but documents no request body
/// forces API consumers to read source instead of the spec.
#[test]
fn post_runs_documents_its_request_body() {
    let spec: serde_json::Value = serde_json::from_str(&oxo_flow_web::openapi::spec_json("hpc"))
        .expect("generated spec must parse as JSON");

    let post = &spec["paths"]["/api/runs"]["post"];
    assert!(
        post.get("requestBody").is_some(),
        "POST /api/runs must declare a requestBody in the OpenAPI spec"
    );
    let schema = &post["requestBody"]["content"]["application/json"]["schema"];
    assert!(
        schema.get("$ref").is_some() || schema.get("properties").is_some(),
        "POST /api/runs requestBody must reference a schema, got {schema}"
    );
}

/// The AI-native surfaces parse JSON payloads on every request; each must
/// document its body schema so generated clients do not have to read the
/// handler source (issue #324 F-4 follow-up).
#[test]
fn ai_routes_document_their_request_bodies() {
    let spec: serde_json::Value = serde_json::from_str(&oxo_flow_web::openapi::spec_json("hpc"))
        .expect("generated spec must parse as JSON");

    for (path, expected_schema) in [
        ("/api/ai/translate", "TranslateRequest"),
        ("/api/ai/translate/stream", "TranslateRequest"),
        ("/api/ai/explain", "ExplainRequest"),
        ("/api/ai/interpret", "InterpretRequest"),
        ("/api/ai/optimize", "OptimizeRequest"),
        ("/api/ai/config", "AiConfigRequest"),
        ("/api/ai/test", "AiConfigRequest"),
        ("/api/chat/send", "ChatRequest"),
        ("/api/chat/send/json", "ChatRequest"),
    ] {
        let post = &spec["paths"][path]["post"];
        let schema = &post["requestBody"]["content"]["application/json"]["schema"];
        let ref_name = schema["$ref"]
            .as_str()
            .unwrap_or_else(|| panic!("POST {path} must declare a requestBody schema: {schema}"));
        assert!(
            ref_name.ends_with(expected_schema),
            "POST {path} must reference {expected_schema}, got {ref_name}"
        );
        assert!(
            spec["components"]["schemas"][expected_schema].is_object(),
            "components.schemas.{expected_schema} must be generated"
        );
    }

    // The translate contract's required field is documented, not implicit.
    assert_eq!(
        spec["components"]["schemas"]["TranslateRequest"]["properties"]["intent"]["type"], "string",
        "TranslateRequest.intent must be a documented string"
    );
    assert_eq!(
        spec["components"]["schemas"]["ChatRequest"]["properties"]["message"]["type"], "string",
        "ChatRequest.message must be a documented string"
    );

    // Attribution fields are part of the documented response contract.
    let translate = &spec["components"]["schemas"]["TranslateResponse"]["properties"];
    assert_eq!(translate["provider"]["type"], "string");
    assert_eq!(translate["fallback_used"]["type"], "boolean");
    assert!(
        translate["attempts"].is_object(),
        "TranslateResponse.attempts must be documented: {translate}"
    );
    assert!(
        spec["components"]["schemas"]["ProviderAttempt"].is_object(),
        "ProviderAttempt must be a generated component schema"
    );
}

/// Task 7b / I3: a handler whose signature parses `ApiJson<T>` is a
/// body-carrying operation, and the generated contract must say so. The
/// check is mechanical over ALL crate sources (not a hand list): for every
/// `ApiJson<` occurrence outside a `#[cfg(test)]` module/function, the
/// nearest preceding `#[utoipa::path(...)]` block must contain
/// `request_body`.
///
/// Review I-1: the scanner used to truncate each file at the FIRST
/// `#[cfg(test)]`, so `execution/handlers.rs` (a `#[cfg(test)] mod
/// quota_tests` appears before the run handlers) was silently never
/// scanned. It now brace-matches every `#[cfg(test)] mod|fn` range and skips
/// only those, and walks every `.rs` under `src/` (the extractor definition
/// is excluded because its `ApiJson` mentions are the type itself, not
/// routes).
#[test]
fn every_api_json_handler_declares_a_request_body() {
    fn collect_sources(dir: &std::path::Path, out: &mut Vec<std::path::PathBuf>) {
        for entry in std::fs::read_dir(dir).expect("source dir is readable") {
            let path = entry.expect("dir entry").path();
            if path.is_dir() {
                collect_sources(&path, out);
            } else if path.extension().and_then(|ext| ext.to_str()) == Some("rs")
                && path.file_name().and_then(|name| name.to_str()) != Some("extract.rs")
            {
                out.push(path);
            }
        }
    }

    /// End offset of the `#[cfg(test)] mod|fn ...` item starting at `start`,
    /// matched by braces (string literals ignored). `None` for `cfg(test)`
    /// attributes on non-braced items (e.g. match arms), which cannot hide
    /// a handler.
    fn cfg_test_range(text: &str, start: usize) -> Option<usize> {
        let after = start + "#[cfg(test)]".len();
        let rest = &text[after..];
        let mut probe = rest.trim_start();
        if probe.starts_with("pub ") || probe.starts_with("pub(crate) ") {
            probe = probe
                .split_once(char::is_whitespace)
                .map_or(probe, |(_, r)| r.trim_start());
        }
        if probe.starts_with("async ") {
            probe = probe["async ".len()..].trim_start();
        }
        if !(probe.starts_with("mod ") || probe.starts_with("fn ")) {
            return None;
        }
        let open = after + rest.find('{')?;
        let mut depth = 0usize;
        let mut in_string = false;
        let mut escaped = false;
        for (offset, ch) in text[open..].char_indices() {
            if in_string {
                if escaped {
                    escaped = false;
                } else if ch == '\\' {
                    escaped = true;
                } else if ch == '"' {
                    in_string = false;
                }
                continue;
            }
            match ch {
                '"' => in_string = true,
                '{' => depth += 1,
                '}' => {
                    depth -= 1;
                    if depth == 0 {
                        return Some(open + offset + 1);
                    }
                }
                _ => {}
            }
        }
        Some(text.len())
    }

    /// End offset of a balanced paren group starting at `open`, ignoring
    /// parens inside string literals (review I-1: an attribute
    /// `description = "...(...)"` must not truncate the block).
    fn matching_paren(text: &str, open: usize) -> usize {
        let mut depth = 0usize;
        let mut in_string = false;
        let mut escaped = false;
        for (offset, ch) in text[open..].char_indices() {
            if in_string {
                if escaped {
                    escaped = false;
                } else if ch == '\\' {
                    escaped = true;
                } else if ch == '"' {
                    in_string = false;
                }
                continue;
            }
            match ch {
                '"' => in_string = true,
                '(' => depth += 1,
                ')' => {
                    depth -= 1;
                    if depth == 0 {
                        return open + offset + 1;
                    }
                }
                _ => {}
            }
        }
        text.len()
    }

    /// Whether `idx` sits in a line or block comment (doc comments mention
    /// `ApiJson<T>` in prose; they are not handler signatures).
    fn in_comment(text: &str, idx: usize) -> bool {
        let line_start = text[..idx].rfind('\n').map_or(0, |line| line + 1);
        if text[line_start..idx].contains("//") {
            return true;
        }
        let before = &text[..idx];
        let last_open = before.rfind("/*");
        let last_close = before.rfind("*/");
        last_open.is_some_and(|open| last_close.is_none_or(|close| open > close))
    }

    let src = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut files = Vec::new();
    collect_sources(&src, &mut files);
    assert!(!files.is_empty(), "no sources found under {src:?}");

    let mut offenders = Vec::new();
    let mut execution_handlers_scanned = 0usize;
    for path in &files {
        let text = std::fs::read_to_string(path).expect("source is UTF-8");
        let mut skipped: Vec<(usize, usize)> = text
            .match_indices("#[cfg(test)]")
            .filter_map(|(idx, _)| cfg_test_range(&text, idx).map(|end| (idx, end)))
            .collect();
        skipped.sort_unstable();
        let is_execution_handlers = path.ends_with("domains/execution/handlers.rs");

        for (idx, _) in text.match_indices("ApiJson<") {
            if in_comment(&text, idx)
                || skipped
                    .iter()
                    .any(|(start, end)| idx >= *start && idx < *end)
            {
                continue;
            }
            if is_execution_handlers {
                execution_handlers_scanned += 1;
            }
            let Some(attr_start) = text[..idx].rfind("#[utoipa::path(") else {
                offenders.push(format!(
                    "{}: `ApiJson<` without a preceding #[utoipa::path]",
                    path.display()
                ));
                continue;
            };
            if skipped
                .iter()
                .any(|(start, end)| attr_start >= *start && attr_start < *end)
            {
                continue;
            }
            let open = attr_start
                + text[attr_start..]
                    .find('(')
                    .expect("attribute has an opening paren");
            let end = matching_paren(&text, open);
            let block = &text[attr_start..end];
            // A free helper after a route must not borrow that route's
            // attribute: the occurrence has to sit in a function signature.
            if !text[end..idx].contains("fn ") {
                offenders.push(format!(
                    "{}: `ApiJson<` is not inside a documented handler signature",
                    path.display()
                ));
                continue;
            }
            if !block.contains("request_body") {
                let route = block
                    .split("path = \"")
                    .nth(1)
                    .and_then(|rest| rest.split('"').next())
                    .unwrap_or("<unknown path>");
                offenders.push(format!("{}: {route}", path.display()));
            }
        }
    }
    // Regression pin for review I-1: execution/handlers.rs has 7 ApiJson
    // handlers after a cfg(test) module and must actually be scanned.
    assert!(
        execution_handlers_scanned >= 7,
        "execution/handlers.rs must contribute its ApiJson handlers to the scan \
         (got {execution_handlers_scanned}); the cfg(test) skipping regressed"
    );
    assert!(
        offenders.is_empty(),
        "handlers that parse ApiJson<T> must declare `request_body` in their \
         #[utoipa::path]:\n  {}",
        offenders.join("\n  ")
    );
}

/// Task 7b / I3: every operation declares its security posture. Protected
/// operations accept either the session bearer token or `X-API-Key`; public
/// ones declare `"security": []` explicitly. The public/protected split is
/// asserted against `server::is_public_route` — the same predicate the auth
/// middleware uses — so the spec cannot drift from the runtime gate.
#[test]
fn every_operation_declares_its_security() {
    let spec: serde_json::Value = serde_json::from_str(&oxo_flow_web::openapi::spec_json("hpc"))
        .expect("generated spec must parse as JSON");
    let paths = spec["paths"].as_object().expect("paths object");
    let methods = [
        "get", "put", "post", "delete", "patch", "options", "head", "trace",
    ];

    let mut offenders = Vec::new();
    for (path, item) in paths {
        for method in methods {
            let Some(operation) = item.get(method) else {
                continue;
            };
            let http_method = axum::http::Method::from_bytes(method.to_uppercase().as_bytes())
                .expect("known method");
            let public = oxo_flow_web::server::is_public_route(path, &http_method, false);
            let security = operation.get("security");
            if public {
                match security {
                    Some(serde_json::Value::Array(entries)) if entries.is_empty() => {}
                    other => offenders.push(format!(
                        "{method_upper} {path}: public operation must declare \
                         \"security\": [], got {other:?}",
                        method_upper = method.to_uppercase()
                    )),
                }
                continue;
            }
            let entries = security
                .and_then(serde_json::Value::as_array)
                .cloned()
                .unwrap_or_default();
            let schemes: Vec<&str> = entries
                .iter()
                .filter_map(serde_json::Value::as_object)
                .flat_map(|entry| entry.keys().map(String::as_str))
                .collect();
            if !schemes.contains(&"bearerAuth") || !schemes.contains(&"apiKey") {
                offenders.push(format!(
                    "{method_upper} {path}: protected operation must declare bearerAuth \
                     and apiKey, got {security:?}",
                    method_upper = method.to_uppercase()
                ));
            }
        }
    }
    assert!(
        offenders.is_empty(),
        "OpenAPI security declarations are incomplete:\n  {}",
        offenders.join("\n  ")
    );
}

/// The `apiKey` scheme must be advertised and referenced wherever the
/// middleware accepts `X-API-Key` (Task 7b / I3).
#[test]
fn api_key_scheme_is_advertised_and_referenced() {
    let spec: serde_json::Value = serde_json::from_str(&oxo_flow_web::openapi::spec_json("hpc"))
        .expect("generated spec must parse as JSON");
    let scheme = &spec["components"]["securitySchemes"]["apiKey"];
    assert_eq!(scheme["type"], "apiKey", "{scheme}");
    assert_eq!(scheme["in"], "header", "{scheme}");
    assert_eq!(scheme["name"], "X-API-Key", "{scheme}");

    let references = spec["paths"]
        .as_object()
        .expect("paths object")
        .values()
        .filter_map(serde_json::Value::as_object)
        .flat_map(|item| item.values())
        .filter_map(|operation| operation.get("security"))
        .filter_map(serde_json::Value::as_array)
        .flatten()
        .filter_map(serde_json::Value::as_object)
        .filter(|requirement| requirement.contains_key("apiKey"))
        .count();
    assert!(
        references > 0,
        "the apiKey scheme is advertised but never referenced by an operation"
    );
}

/// The audit's concrete examples (I3): the AI probe, login, pipeline
/// create, user create, cluster upsert, webhook update, and the AI
/// translate/generation routes must all document a JSON body.
#[test]
fn representative_write_routes_document_request_bodies() {
    let spec: serde_json::Value = serde_json::from_str(&oxo_flow_web::openapi::spec_json("hpc"))
        .expect("generated spec must parse as JSON");
    for (path, method) in [
        ("/api/ai/test", "post"),
        ("/api/ai/translate", "post"),
        ("/api/ai/explain", "post"),
        ("/api/ai/interpret", "post"),
        ("/api/ai/optimize", "post"),
        ("/api/ai/config", "post"),
        ("/api/ai/config/server", "put"),
        ("/api/ai/config/user", "put"),
        ("/api/auth/login", "post"),
        ("/api/pipelines", "post"),
        ("/api/pipelines/{id}", "put"),
        ("/api/users", "post"),
        ("/api/clusters", "post"),
        ("/api/webhook", "put"),
        ("/api/chat/send", "post"),
        ("/api/chat/send/json", "post"),
        ("/api/runs/{id}/retry", "post"),
        ("/api/runs/{id}/report/ask", "post"),
        ("/api/runs/{id}/resume-checkpoint", "post"),
        ("/api/pipelines/{id}/rollback", "post"),
    ] {
        let operation = &spec["paths"][path][method];
        assert!(
            operation.get("requestBody").is_some(),
            "{method} {path} must declare a requestBody"
        );
        assert!(
            operation["requestBody"]["content"]["application/json"].is_object(),
            "{method} {path} requestBody must document an application/json schema"
        );
    }
}

/// #914 review NIT: POST /api/events/ticket must document its response body
/// (TicketResponse), not just an untyped 200 JSON blob.
#[test]
fn events_ticket_documents_its_response_body() {
    let spec: serde_json::Value = serde_json::from_str(&oxo_flow_web::openapi::spec_json("hpc"))
        .expect("generated spec must parse as JSON");

    let post = &spec["paths"]["/api/events/ticket"]["post"];
    let schema = &post["responses"]["200"]["content"]["application/json"]["schema"];
    let ref_name = schema["$ref"].as_str().unwrap_or_else(|| {
        panic!("POST /api/events/ticket 200 must $ref TicketResponse, got {schema}")
    });
    assert!(
        ref_name.ends_with("TicketResponse"),
        "expected TicketResponse $ref, got {ref_name}"
    );
    let component = &spec["components"]["schemas"]["TicketResponse"];
    assert_eq!(
        component["properties"]["ticket"]["type"], "string",
        "TicketResponse.ticket must be a string: {component}"
    );
    assert_eq!(
        component["properties"]["expires_in_secs"]["type"], "integer",
        "TicketResponse.expires_in_secs must be an integer: {component}"
    );
}

/// Task 7b / I3 review Minor 2: the security declarations are derived from
/// `server::is_public_route`, the same predicate the auth middleware uses —
/// by construction the spec cannot disagree with itself. This behavioural
/// test pins the outcome in team mode so a wrong classification (e.g. a
/// protected path added to the public list) fails even though the
/// source-scanning assertions share the predicate.
#[tokio::test]
async fn public_route_predicate_matches_team_mode_middleware() {
    use axum::body::Body;
    use tower::ServiceExt;

    let app = oxo_flow_web::server::build_router("team");
    async fn status(app: &axum::Router, uri: &str) -> u16 {
        let response = app
            .clone()
            .oneshot(
                axum::http::Request::builder()
                    .uri(uri)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        response.status().as_u16()
    }

    for uri in [
        "/api/health",
        "/api/openapi.json",
        "/api/ai/config",
        "/api/license",
    ] {
        assert_eq!(status(&app, uri).await, 200, "{uri} must stay public");
    }
    for uri in [
        "/api/pipelines",
        "/api/runs",
        "/api/audit",
        "/api/ai/config/effective",
    ] {
        assert_eq!(
            status(&app, uri).await,
            401,
            "{uri} must require authentication in team mode"
        );
    }
}
