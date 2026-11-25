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
