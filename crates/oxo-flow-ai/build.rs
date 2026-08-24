//! Emits `knowledge_meta_embedded` when `knowledge_meta.json` exists, so the
//! crate compiles both before the knowledge update pipeline produces the
//! metadata file and after (it is embedded via `include_str!` when present).

use std::path::Path;

fn main() {
    // Declared so the compiler's unexpected_cfgs lint accepts it.
    println!("cargo::rustc-check-cfg=cfg(knowledge_meta_embedded)");
    let meta = Path::new("src/knowledge/knowledge_meta.json");
    if meta.exists() {
        println!("cargo:rustc-cfg=knowledge_meta_embedded");
    }
    println!("cargo:rerun-if-changed=src/knowledge/knowledge_meta.json");
}
