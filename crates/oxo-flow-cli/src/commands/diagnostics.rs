//! `oxo-flow diagnostics` — read-only enumeration of the diagnostic catalogs.
//!
//! - `diagnostics list` — enumerate the runtime failure patterns (the 31
//!   entries `DiagnosticsEngine::analyze` matches) and/or the SCI-* scientific
//!   preflight rules.
//!
//! The data is pulled from the engine's own single-source tables
//! ([`oxo_flow_core::diagnostics::DiagnosticsEngine::catalog`] and
//! [`oxo_flow_core::scientific_catalog::sci_catalog`]), so the CLI can never
//! enumerate an id the engine does not actually know — and vice versa.
//!
//! JSON goes to stdout by default (mirroring `knowledge list`); `--format
//! text` prints a human-readable summary.

use anyhow::{Result, bail};
use clap::Subcommand;
use oxo_flow_core::diagnostics::DiagnosticsEngine;
use oxo_flow_core::scientific_catalog::sci_catalog;
use serde_json::json;

#[derive(Subcommand, Debug)]
pub enum DiagnosticsAction {
    /// List the diagnostic catalogs (runtime failure patterns + SCI-* rules).
    List {
        #[arg(
            value_name = "WHAT",
            help = "Which catalog: runtime, scientific, or all (default: all)"
        )]
        what: Option<String>,
    },
}

/// Which catalog a `list` invocation selects.
enum Catalog {
    Runtime,
    Scientific,
    All,
}

fn resolve_catalog(what: Option<&str>) -> Result<Catalog> {
    match what {
        None | Some("all") => Ok(Catalog::All),
        Some("runtime") => Ok(Catalog::Runtime),
        Some("scientific") => Ok(Catalog::Scientific),
        Some(other) => {
            bail!("unknown catalog '{other}' (expected 'runtime', 'scientific', or 'all')")
        }
    }
}

fn list_json(what: Option<&str>) -> Result<serde_json::Value> {
    let catalog = resolve_catalog(what)?;
    let runtime = matches!(catalog, Catalog::Runtime | Catalog::All);
    let scientific = matches!(catalog, Catalog::Scientific | Catalog::All);

    let mut out = json!({
        "command": "diagnostics",
        "action": "list",
        "what": what.unwrap_or("all"),
    });

    if runtime {
        let patterns = DiagnosticsEngine::catalog();
        out["runtime"] = json!({
            "count": patterns.len(),
            "patterns": patterns,
        });
    }
    if scientific {
        let rules = sci_catalog();
        out["scientific"] = json!({
            "count": rules.len(),
            "rules": rules,
        });
    }
    Ok(out)
}

fn list_text(what: Option<&str>) -> Result<()> {
    let catalog = resolve_catalog(what)?;
    if matches!(catalog, Catalog::Runtime | Catalog::All) {
        let patterns = DiagnosticsEngine::catalog();
        println!("{} runtime diagnostic patterns:", patterns.len());
        for p in &patterns {
            let fix = if p.auto_fixable {
                " [auto-fixable]"
            } else {
                ""
            };
            println!("- {} ({:?}){} — {}", p.id, p.category, fix, p.summary);
        }
    }
    if matches!(catalog, Catalog::Scientific | Catalog::All) {
        let rules = sci_catalog();
        if matches!(catalog, Catalog::All) {
            println!();
        }
        println!("{} scientific preflight rules:", rules.len());
        for r in &rules {
            println!("- {} — {}", r.code, r.summary);
        }
    }
    Ok(())
}

/// Execute the `diagnostics` subcommand.
pub fn diagnostics_command(action: DiagnosticsAction, format: Option<String>) -> Result<()> {
    let text = match format.as_deref() {
        Some("text") => true,
        Some("json") | None => false,
        Some(other) => bail!("unknown format '{other}' (expected 'json' or 'text')"),
    };

    match action {
        DiagnosticsAction::List { what } => {
            if text {
                list_text(what.as_deref())?;
            } else {
                println!(
                    "{}",
                    serde_json::to_string_pretty(&list_json(what.as_deref())?)?
                );
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeSet;

    #[test]
    fn list_json_all_carries_both_catalogs() {
        let v = list_json(None).expect("list all");
        assert_eq!(v["runtime"]["count"], 31);
        assert_eq!(v["scientific"]["count"], 51);
    }

    #[test]
    fn list_json_runtime_only() {
        let v = list_json(Some("runtime")).expect("list runtime");
        assert!(v.get("runtime").is_some());
        assert!(v.get("scientific").is_none());
    }

    #[test]
    fn list_json_scientific_only() {
        let v = list_json(Some("scientific")).expect("list scientific");
        assert!(v.get("scientific").is_some());
        assert!(v.get("runtime").is_none());
    }

    #[test]
    fn list_json_unknown_catalog_bails() {
        assert!(list_json(Some("recipes")).is_err());
    }

    #[test]
    fn command_output_ids_equal_internal_catalog_ids() {
        // Falsifiable: the ids the command surfaces must be exactly the ids
        // the internal catalogs hold. Add a pattern/rule to the table but not
        // to the surface (or vice versa) and this fails.
        let v = list_json(None).expect("list all");

        let surfaced: BTreeSet<String> = v["runtime"]["patterns"]
            .as_array()
            .expect("runtime patterns")
            .iter()
            .map(|p| p["id"].as_str().expect("id").to_string())
            .collect();
        let internal: BTreeSet<String> = DiagnosticsEngine::catalog()
            .iter()
            .map(|p| p.id.clone())
            .collect();
        assert_eq!(
            surfaced, internal,
            "runtime ids drifted from the engine table"
        );
        assert_eq!(surfaced.len(), 31);

        let surfaced_sci: BTreeSet<String> = v["scientific"]["rules"]
            .as_array()
            .expect("scientific rules")
            .iter()
            .map(|r| r["code"].as_str().expect("code").to_string())
            .collect();
        let internal_sci: BTreeSet<String> = sci_catalog().iter().map(|r| r.code.clone()).collect();
        assert_eq!(
            surfaced_sci, internal_sci,
            "SCI-* codes drifted from the catalog"
        );
        assert_eq!(surfaced_sci.len(), 51);
    }

    #[test]
    fn format_flag_rejects_unknown() {
        let err = diagnostics_command(DiagnosticsAction::List { what: None }, Some("yaml".into()))
            .expect_err("unknown format must fail");
        assert!(err.to_string().contains("unknown format 'yaml'"));
    }

    #[test]
    fn text_list_all_prints() {
        list_text(None).expect("list_text all");
        list_text(Some("runtime")).expect("list_text runtime");
        list_text(Some("scientific")).expect("list_text scientific");
    }
}
