//! Print the generated default-mode OpenAPI path/operation counts as JSON.
//!
//! `scripts/check-doc-claims.py` (`make claims-check`) runs this example so
//! the documented counts are derived from the real generated spec instead of
//! being retyped in the docs or the gate. Default mode mirrors
//! [`oxo_flow_web::openapi::spec_json`]'s removal of the HPC-only
//! `/api/hpc` route, which the non-HPC router does not mount.

use utoipa::OpenApi;
use utoipa::openapi::PathItem;

fn operation_count(item: &PathItem) -> usize {
    [
        item.get.is_some(),
        item.put.is_some(),
        item.post.is_some(),
        item.delete.is_some(),
        item.options.is_some(),
        item.head.is_some(),
        item.patch.is_some(),
        item.trace.is_some(),
    ]
    .into_iter()
    .filter(|present| *present)
    .count()
}

fn main() {
    let mut doc = oxo_flow_web::openapi::ApiDoc::openapi();
    doc.paths.paths.remove("/api/hpc");
    let operations: usize = doc.paths.paths.values().map(operation_count).sum();
    println!(
        "{{\"paths\":{},\"operations\":{}}}",
        doc.paths.paths.len(),
        operations
    );
}
