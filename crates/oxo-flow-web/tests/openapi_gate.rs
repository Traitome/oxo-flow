//! OpenAPI drift gate (issue #82 P1-13).
//!
//! The spec is CODE-GENERATED from the `#[utoipa::path]` annotations on the
//! route handlers — there is no static openapi.json to drift from. This test
//! is the gate: it asserts that every route registered in
//! `crate::server::build_router` is present in the generated spec.
//!
//! Adding a new API route REQUIRES:
//!   1. a `#[utoipa::path(...)]` annotation on the handler, and
//!   2. an entry in [`ALL_ROUTES`] below (the test fails otherwise).

/// Every API path registered in `crate::server::build_router`, in the
/// router's own order. Each entry must exist in the generated spec.
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
    "/api/runs/{id}/logs",
    "/api/runs/{id}/results",
    "/api/runs/{id}/retry",
    "/api/runs/{id}/cancel",
    "/api/runs/{id}/pause",
    "/api/runs/{id}/resume",
    "/api/runs/{id}/preview",
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
    // templates
    "/api/templates",
    "/api/templates/{id}",
    // auth
    "/api/auth/login",
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
    "/api/audit",
    "/api/quota",
    "/api/webhook",
    // hpc
    "/api/hpc",
];

/// Serialize the generated spec, parse it back, and assert the shape.
#[test]
fn generated_spec_is_valid_json_and_openapi_3_1() {
    let json = oxo_flow_web::openapi::spec_json();

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
    let spec: serde_json::Value = serde_json::from_str(&oxo_flow_web::openapi::spec_json())
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

/// Body-carrying POSTs must declare their request body (issue #324 F-4):
/// a handler that parses a JSON payload but documents no request body
/// forces API consumers to read source instead of the spec.
#[test]
fn post_runs_documents_its_request_body() {
    let spec: serde_json::Value = serde_json::from_str(&oxo_flow_web::openapi::spec_json())
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
