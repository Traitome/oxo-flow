//! `oxo-flow knowledge` — read-only access to the embedded knowledge base.
//!
//! Exposes the same data surfaces the AI agent tools use (Bioconda tools,
//! nf-core modules, bio.tools/commercial registry, pipeline skills, full
//! skill docs, pipeline graph, freshness meta) through a deterministic CLI:
//!
//! - `knowledge search <query>` — cross-source unified search
//! - `knowledge get <tool|module|skill|doc> <name>` — exact fetch
//! - `knowledge list docs|domains` — embedded indexes
//! - `knowledge stats` — per-source record counts + freshness
//!
//! JSON output goes to stdout; text output reuses the formatters the AI
//! tool layer uses, so what the model sees is what the CLI prints.

use anyhow::{Result, anyhow, bail};
use chrono::Utc;
use clap::Subcommand;
use oxo_flow_ai::knowledge::bioconda::{format_search_results, get_tool, search_tools, tool_count};
use oxo_flow_ai::knowledge::meta::{embedded_meta, format_source_row};
use oxo_flow_ai::knowledge::pipeline_graph::graph_stats;
use oxo_flow_ai::knowledge::registry::{
    format_registry_hit, format_registry_results, get_registry_tool, registry_count,
};
use oxo_flow_ai::knowledge::skillgraph_docs::{doc_count, doc_ids, format_doc, get_doc, list_docs};
use oxo_flow_ai::knowledge::skills::{
    format_skills, get_skill, list_domains, search_skills, skill_count,
};
use serde_json::json;

/// Maximum rows returned by search/list operations (keeps text output
/// readable and JSON payloads bounded).
const SEARCH_LIMIT: usize = 10;

#[derive(Subcommand, Debug)]
pub enum KnowledgeAction {
    /// Cross-source unified search (Bioconda tools + registry + skills).
    Search {
        #[arg(
            value_name = "QUERY",
            help = "Search keywords, e.g. 'variant calling' or 'samtools'"
        )]
        query: String,
    },
    /// Exact-fetch one record by kind.
    Get {
        #[arg(
            value_name = "KIND",
            help = "Record kind: tool (bioconda), module (nf-core), skill, doc"
        )]
        kind: String,
        #[arg(value_name = "NAME", help = "Record name/id (case-insensitive)")]
        name: String,
    },
    /// List an embedded index.
    List {
        #[arg(
            value_name = "WHAT",
            help = "Index to list: docs (skill documents) or domains (skill domains)"
        )]
        what: String,
    },
    /// Show per-source record counts and freshness.
    Stats,
}

fn meta_or_bail() -> Result<&'static oxo_flow_ai::knowledge::meta::KnowledgeMeta> {
    embedded_meta().ok_or_else(|| {
        anyhow!(
            "knowledge metadata is not embedded in this build (knowledge_meta_embedded cfg missing)"
        )
    })
}

fn registry_hit_json(hit: &oxo_flow_ai::knowledge::registry::RegistryHit) -> serde_json::Value {
    use oxo_flow_ai::knowledge::registry::RegistryHit;
    match hit {
        RegistryHit::NfCore(m) => json!({
            "source": "nf-core",
            "name": m.name,
            "versions": m.versions,
            "summary": m.summary,
            "license": m.license,
            "biotools_id": m.biotools_id,
            "doi": m.doi,
        }),
        RegistryHit::Commercial(t) => json!({
            "source": "commercial",
            "name": t.name,
            "version": t.version,
            "summary": t.summary,
            "url": t.url,
            "note": t.note,
        }),
        RegistryHit::BioTools(b) => json!({
            "source": "bio.tools",
            "name": b.name,
            "description": b.description,
            "license": b.license,
            "homepage": b.homepage,
            "topics": b.topics,
            "operations": b.operations,
            "dois": b.dois,
        }),
    }
}

fn skill_json(skill: &oxo_flow_ai::knowledge::skills::SkillRecord) -> serde_json::Value {
    json!({
        "name": skill.name,
        "domain": skill.domain,
        "description": skill.description,
        "tool_type": skill.tool_type,
        "primary_tool": skill.primary_tool,
        "preview": skill.preview,
    })
}

// ── JSON emitters ───────────────────────────────────────────────────────────

fn search_json(query: &str) -> serde_json::Value {
    let tools = search_tools(query, SEARCH_LIMIT);
    let registry = oxo_flow_ai::knowledge::registry::search_registry(query, SEARCH_LIMIT);
    let skills = search_skills(query, SEARCH_LIMIT);
    json!({
        "command": "knowledge",
        "action": "search",
        "query": query,
        "tools": tools
            .iter()
            .map(|t| json!({
                "name": t.name, "version": t.version,
                "summary": t.summary, "platforms": t.platforms,
            }))
            .collect::<Vec<_>>(),
        "registry": registry.iter().map(registry_hit_json).collect::<Vec<_>>(),
        "skills": skills.iter().map(|s| skill_json(s)).collect::<Vec<_>>(),
    })
}

fn get_json(kind: &str, name: &str) -> Result<serde_json::Value> {
    match kind {
        "tool" => get_tool(name)
            .map(|t| {
                json!({
                    "command": "knowledge", "action": "get", "kind": kind,
                    "found": true,
                    "record": {
                        "name": t.name, "version": t.version,
                        "summary": t.summary, "platforms": t.platforms,
                    },
                })
            })
            .ok_or_else(|| anyhow!("no bioconda tool named '{name}'")),
        "module" => get_registry_tool(name)
            .map(|hit| {
                json!({
                    "command": "knowledge", "action": "get", "kind": kind,
                    "found": true,
                    "record": registry_hit_json(&hit),
                })
            })
            .ok_or_else(|| {
                anyhow!("no registry record named '{name}' (nf-core, commercial, or bio.tools)")
            }),
        "skill" => get_skill(name)
            .map(|s| {
                json!({
                    "command": "knowledge", "action": "get", "kind": kind,
                    "found": true,
                    "record": skill_json(s),
                })
            })
            .ok_or_else(|| anyhow!("no skill named '{name}' (names carry a bio- prefix; see `oxo-flow knowledge search '{name}'`)")),
        "doc" => get_doc(name)
            .map(|d| {
                json!({
                    "command": "knowledge", "action": "get", "kind": kind,
                    "found": true,
                    "record": { "id": d.id, "doc": d.doc },
                })
            })
            .ok_or_else(|| {
                anyhow!("no skill doc for '{name}' (see `oxo-flow knowledge list docs`)")
            }),
        _ => bail!("unknown kind '{kind}' (expected 'tool', 'module', 'skill', or 'doc')"),
    }
}

fn list_json(what: &str) -> Result<serde_json::Value> {
    match what {
        "docs" => {
            let ids = doc_ids();
            let count = ids.len();
            Ok(json!({
                "command": "knowledge", "action": "list", "what": what,
                "count": count,
                "docs": ids,
            }))
        }
        "domains" => {
            let domains = list_domains();
            Ok(json!({
                "command": "knowledge", "action": "list", "what": what,
                "count": domains.len(),
                "domains": domains
                    .iter()
                    .map(|(name, n)| json!({ "name": name, "skills": n }))
                    .collect::<Vec<_>>(),
            }))
        }
        _ => bail!("unknown index '{what}' (expected 'docs' or 'domains')"),
    }
}

fn stats_json() -> Result<serde_json::Value> {
    let meta = meta_or_bail()?;
    let now = Utc::now();
    let (graph_nodes, graph_edges) = graph_stats();
    let sources: Vec<serde_json::Value> = meta
        .sources
        .iter()
        .map(|src| {
            json!({
                "name": src.name,
                "count": src.count,
                "generated_at": src.generated_at,
                "auto": src.auto,
                "stale": src.is_stale(now),
            })
        })
        .collect();
    Ok(json!({
        "command": "knowledge", "action": "stats",
        "sources": sources,
        "live_counts": {
            "bioconda_tools": tool_count(),
            "registry": registry_count(),
            "skills": skill_count(),
            "skill_docs": doc_count(),
            "graph_nodes": graph_nodes,
            "graph_edges": graph_edges,
        },
    }))
}

// ── Text emitters ───────────────────────────────────────────────────────────

fn search_text(query: &str) {
    // format_search_results self-covers the empty case ("No Bioconda CLI
    // tools matched …"), so no is_empty fallback is needed here.
    let bioconda = format_search_results(query, SEARCH_LIMIT);
    println!("{bioconda}");
    let registry = format_registry_results(query, SEARCH_LIMIT);
    if !registry.is_empty() {
        println!("\n{registry}");
    }
    let skills = format_skills(query, SEARCH_LIMIT);
    println!("\n{skills}");
}

fn get_text(kind: &str, name: &str) -> Result<()> {
    match kind {
        "tool" => match get_tool(name) {
            Some(t) => {
                println!("{} {} — {}", t.name, t.version, t.summary);
                if !t.platforms.is_empty() {
                    println!("platforms: {}", t.platforms.join(", "));
                }
            }
            None => bail!("no bioconda tool named '{name}'"),
        },
        "module" => match get_registry_tool(name) {
            Some(hit) => println!("{}", format_registry_hit(&hit)),
            None => bail!("no registry record named '{name}' (nf-core, commercial, or bio.tools)"),
        },
        "skill" => match get_skill(name) {
            Some(s) => {
                println!("[{}] {} — {}", s.domain, s.name, s.description);
                if !s.primary_tool.is_empty() {
                    println!("primary_tool: {}", s.primary_tool);
                }
                if !s.preview.is_empty() {
                    println!("\n{}", s.preview);
                }
            }
            None => bail!(
                "no skill named '{name}' (names carry a bio- prefix; see `oxo-flow knowledge search '{name}'`)"
            ),
        },
        "doc" => match get_doc(name) {
            Some(d) => println!("{}", format_doc(d)),
            None => bail!("no skill doc for '{name}' (see `oxo-flow knowledge list docs`)"),
        },
        _ => bail!("unknown kind '{kind}' (expected 'tool', 'module', 'skill', or 'doc')"),
    }
    Ok(())
}

fn list_text(what: &str) -> Result<()> {
    match what {
        "docs" => println!("{}", list_docs()),
        "domains" => {
            let domains = list_domains();
            println!(
                "{} skill domains ({} embedded skills):",
                domains.len(),
                skill_count()
            );
            for (name, n) in domains {
                println!("- {name}: {n} skills");
            }
        }
        _ => bail!("unknown index '{what}' (expected 'docs' or 'domains')"),
    }
    Ok(())
}

fn stats_text() -> Result<()> {
    let meta = meta_or_bail()?;
    let now = Utc::now();
    println!("Embedded knowledge sources:");
    for src in &meta.sources {
        let mut row = format_source_row(src, now);
        if src.is_stale(now) {
            row.push_str("  [STALE]");
        }
        println!("- {row}");
    }
    let (nodes, edges) = graph_stats();
    println!(
        "\nLive record counts: {} bioconda tools, {} registry records, {} skills, {} skill docs, {} graph nodes / {} transitions",
        tool_count(),
        registry_count(),
        skill_count(),
        doc_count(),
        nodes,
        edges
    );
    Ok(())
}

// ── Entry point ─────────────────────────────────────────────────────────────

/// Execute the `knowledge` subcommand.
pub fn knowledge_command(action: KnowledgeAction, format: Option<String>) -> Result<()> {
    let text = match format.as_deref() {
        Some("text") => true,
        Some("json") | None => false,
        Some(other) => bail!("unknown format '{other}' (expected 'json' or 'text')"),
    };

    match action {
        KnowledgeAction::Search { query } => {
            if text {
                search_text(&query);
            } else {
                println!("{}", serde_json::to_string_pretty(&search_json(&query))?);
            }
        }
        KnowledgeAction::Get { kind, name } => {
            if text {
                get_text(&kind, &name)?;
            } else {
                println!(
                    "{}",
                    serde_json::to_string_pretty(&get_json(&kind, &name)?)?
                );
            }
        }
        KnowledgeAction::List { what } => {
            if text {
                list_text(&what)?;
            } else {
                println!("{}", serde_json::to_string_pretty(&list_json(&what)?)?);
            }
        }
        KnowledgeAction::Stats => {
            if text {
                stats_text()?;
            } else {
                println!("{}", serde_json::to_string_pretty(&stats_json()?)?);
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn get_json_tool_exact() {
        let v = get_json("tool", "samtools").expect("samtools is embedded");
        assert_eq!(v["record"]["name"], "samtools");
        assert_eq!(v["found"], true);
    }

    #[test]
    fn get_json_tool_unknown_is_err() {
        assert!(get_json("tool", "no-such-tool-xyz").is_err());
    }

    #[test]
    fn get_json_module_exact() {
        let v = get_json("module", "fastqc").expect("fastqc module is embedded");
        assert_eq!(v["record"]["source"], "nf-core");
        assert_eq!(v["record"]["name"], "fastqc");
    }

    #[test]
    fn get_json_unknown_kind_bails() {
        assert!(get_json("recipe", "x").is_err());
    }

    #[test]
    fn get_json_skill_top_hit() {
        let v = get_json("skill", "bio-gatk-variant-calling").expect("skill is embedded");
        assert_eq!(v["record"]["domain"], "variant-calling");
    }

    #[test]
    fn get_json_skill_unknown_is_err() {
        // get skill is an exact fetch: a fuzzy-only match must not succeed.
        assert!(get_json("skill", "variant-calling").is_err());
    }

    #[test]
    fn get_json_doc_resolves_display_name() {
        // pipeline-graph display names bridge to doc ids; fall back to a
        // doc id that is verified to exist in the embedded data.
        let v = get_json("doc", "variant-calling").or_else(|_| get_json("doc", "clinical-variant"));
        assert!(v.is_ok(), "at least one canonical doc id must resolve");
        let v = v.unwrap();
        assert!(v["record"]["doc"].as_str().unwrap().len() > 100);
    }

    #[test]
    fn list_json_domains_sorted_and_counted() {
        let v = list_json("domains").expect("domains list");
        let domains = v["domains"].as_array().expect("array");
        assert!(!domains.is_empty());
        let names: Vec<&str> = domains
            .iter()
            .map(|d| d["name"].as_str().expect("name"))
            .collect();
        let mut sorted = names.clone();
        sorted.sort();
        assert_eq!(names, sorted, "domains must be BTreeMap-ordered");
    }

    #[test]
    fn list_json_unknown_index_bails() {
        assert!(list_json("tools").is_err());
    }

    #[test]
    fn stats_json_has_sources_and_live_counts() {
        let v = stats_json().expect("meta embedded");
        assert!(v["sources"].as_array().expect("sources").len() >= 8);
        assert!(v["live_counts"]["bioconda_tools"].as_u64().unwrap() > 6000);
        assert!(v["live_counts"]["skill_docs"].as_u64().unwrap() >= 70);
    }

    #[test]
    fn search_json_groups_three_sources() {
        let v = search_json("alignment");
        assert!(v["tools"].as_array().unwrap().iter().count() <= SEARCH_LIMIT);
        assert!(v["registry"].is_array());
        assert!(v["skills"].is_array());
    }

    #[test]
    fn format_flag_rejects_unknown() {
        let err = knowledge_command(KnowledgeAction::Stats, Some("yaml".into()))
            .expect_err("unknown format must fail");
        assert!(err.to_string().contains("unknown format 'yaml'"));
    }

    #[test]
    fn text_stats_prints_sources() {
        // stats_text must not panic with real embedded meta.
        stats_text().expect("stats_text");
    }

    #[test]
    fn text_list_domains_prints() {
        list_text("domains").expect("list_text domains");
        list_text("docs").expect("list_text docs");
    }

    #[test]
    fn text_get_tool_exact() {
        get_text("tool", "bwa").expect("bwa is embedded");
        assert!(get_text("tool", "no-such-tool-xyz").is_err());
    }
}
