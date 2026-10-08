//! Workdir source linking — issue #837.
//!
//! In the two-directory usage mode (shared workflow repo + separate data
//! workdir) every rule executes with `CWD = workdir`, so a relative path a
//! rule reads — a declared input or a `python scripts/tool.py` interpreter
//! argument — resolves against the WORKDIR copy of the file, not the repo.
//! Historically operators hand-synced those copies; nothing verified them,
//! so a drifted script executed silently.
//!
//! The engine now closes that gap with ZERO content duplication: at run
//! start it materializes each referenced repo file inside the workdir as a
//! **symlink** into the repo. The repo stays the single source of truth and
//! the workdir accumulates no copied bytes. Pre-existing workdir files are
//! honored, never silently discarded:
//!
//! - absent in workdir → create a symlink to the repo file;
//! - already a symlink resolving to the same repo file → idempotent no-op;
//! - a regular file byte-identical to the repo file → converted to a symlink
//!   (the duplicate copy is removed — content is preserved in the repo);
//! - a regular file that DIFFERS → loud warning, the workdir copy wins for
//!   this run (it may be a deliberate per-cohort override) and the drift is
//!   impossible to miss.
//!
//! Two reference classes are linked, both read-by-contract so a write
//! through the link can never corrupt the repo from a rule side effect:
//! declared rule inputs (the DAG owns them) and interpreter script
//! arguments (`python scripts/x.py`) already recognized by the D001 deep
//! check. Any OTHER repo path a shell command mentions is reported as an
//! undeclared-reference note telling the operator to declare it as an input
//! — declaring is what makes it linked and tracked.
//!
//! A third class is invisible to shell-text analysis entirely: companion
//! index files that tools auto-derive from a declared file's path by
//! convention (`samtools faidx`/picard read `ref.fa.fai`/`ref.fa.dict`,
//! `bwa mem ref.fa` reads `ref.fa.amb/.ann/.bwt/.pac/.sa`, bgzip/tabix
//! read `.gzi`/`.tbi`, …). Those paths never appear in the shell text, so
//! after collecting the classes above the pass also links every such
//! SIDECAR that exists beside a linked repo file — unless the DAG itself
//! produces the sidecar (e.g. a dedicated `samtools faidx` indexing rule),
//! in which case it is a generated intermediate and the run builds it in
//! the workdir as usual.

use crate::commands::{compute_sha256, run_preview};
use oxo_flow_core::config::WorkflowConfig;
use oxo_flow_core::dag::WorkflowDag;
use oxo_flow_core::deep_check::{interpreter_script_candidates, rendered_shell_text};
use oxo_flow_core::executor::checkpoint::{rendered_input_patterns, resolve_pattern_files};
use std::collections::{BTreeSet, HashMap, HashSet};
use std::path::Path;

/// Outcome of the linking pass, one entry per touched path (relative to the
/// workdir / repo root), in deterministic order.
#[derive(Debug, Default)]
pub(crate) struct SourceLinkReport {
    /// Repo files newly materialized in the workdir as symlinks.
    pub linked: Vec<String>,
    /// Subset of `linked` that are tool-convention index sidecars
    /// (`ref.fa.fai`, `ref.fa.bwt`, …) rather than declared repo files.
    pub sidecars: Vec<String>,
    /// Workdir copies that were byte-identical duplicates and became symlinks.
    pub converted: Vec<String>,
    /// Workdir copies that differ from the repo file — left in place, warned.
    pub conflicts: Vec<String>,
    /// Repo paths mentioned by shell commands but never declared as inputs.
    pub undeclared: Vec<String>,
    /// Per-path filesystem failures (the run continues without them).
    pub errors: Vec<String>,
    /// Why the pass did not run at all (single-directory mode etc.).
    pub skipped_note: Option<String>,
}

impl SourceLinkReport {
    /// True when the pass did nothing worth narrating.
    pub(crate) fn is_empty(&self) -> bool {
        self.linked.is_empty()
            && self.converted.is_empty()
            && self.conflicts.is_empty()
            && self.undeclared.is_empty()
            && self.errors.is_empty()
    }
}

/// Companion index suffixes that bioinformatics tools auto-derive from a
/// file's own path by convention — never spelled out in the shell text:
/// htslib/samtools/picard/GATK (`.fai`, `.dict`, bgzip/tabix/htsfile index
/// forms), bwa index (`ref.fa` → `.amb/.ann/.bwt/.pac/.sa`), minimap2
/// (`.mmi`). Only ever linked when the sidecar actually exists in the repo
/// beside a file that is being linked anyway, so there are no false
/// positives; tools with a bare-index-prefix convention (bowtie2 `-x
/// index`) declare their index files as inputs instead.
const SIDECAR_SUFFIXES: &[&str] = &[
    ".fai", ".dict", ".gzi", ".csi", ".crai", ".tbi", ".bai", ".amb", ".ann", ".bwt", ".pac",
    ".sa", ".mmi",
];

/// Link every workflow-repo file that rules in `order` reference (declared
/// inputs without a producer + interpreter script paths + tool-convention
/// index sidecars) into `workdir` as symlinks. Returns an empty report for
/// the single-directory mode (`workdir == workflow_dir`), where paths
/// already resolve to the repo.
pub(crate) fn link_workflow_sources(
    config: &WorkflowConfig,
    dag: &WorkflowDag,
    order: &[String],
    workflow_dir: &Path,
    workdir: &Path,
    wildcard_values: &HashMap<String, String>,
) -> SourceLinkReport {
    let mut report = SourceLinkReport::default();
    // Canonicalize so e.g. `/link/to/repo` vs `/real/repo` still compare equal.
    let canon = |p: &Path| std::fs::canonicalize(p).unwrap_or_else(|_| p.to_path_buf());
    if canon(workflow_dir) == canon(workdir) {
        return report; // single-directory mode: paths already resolve to the repo
    }
    // A workdir NESTED inside the repo (example directories, docs) would
    // sprout symlinks inside the git tree — leave those runs alone.
    if workdir.starts_with(workflow_dir) {
        report.skipped_note = Some(format!(
            "workdir {} is inside the workflow repo; source linking skipped",
            workdir.display()
        ));
        return report;
    }

    // Outputs the [[references]] machinery creates during the run — not
    // repo files, never link targets (mirrors the missing-source gate).
    let reference_outputs: HashSet<String> = config
        .references
        .iter()
        .map(|r| {
            oxo_flow_core::executor::checkpoint::expand_config_in_path(&r.output, wildcard_values)
        })
        .collect();

    let (linkable, sidecars, undeclared) = source_link_candidates(
        config,
        dag,
        order,
        workflow_dir,
        wildcard_values,
        &reference_outputs,
    );
    report.undeclared = undeclared;

    for rel in &linkable {
        match link_one(rel, workflow_dir, workdir) {
            Ok(outcome @ (LinkOutcome::Created | LinkOutcome::Converted)) => {
                if sidecars.contains(rel) {
                    report.sidecars.push(rel.clone());
                }
                if outcome == LinkOutcome::Created {
                    report.linked.push(rel.clone());
                } else {
                    report.converted.push(rel.clone());
                }
            }
            Ok(LinkOutcome::Idempotent | LinkOutcome::LeftAlone) => {}
            Ok(LinkOutcome::Conflict) => report.conflicts.push(rel.clone()),
            Err(e) => report.errors.push(format!("{}: {e}", rel)),
        }
    }
    report
}

/// Collect the paths to link: the linked-file set (repo-relative, sorted),
/// the tool-convention index sidecars among them, and the undeclared shell
/// references ("`path` (rule 'x')").
#[allow(clippy::type_complexity)]
fn source_link_candidates(
    config: &WorkflowConfig,
    dag: &WorkflowDag,
    order: &[String],
    workflow_dir: &Path,
    wildcard_values: &HashMap<String, String>,
    reference_outputs: &HashSet<String>,
) -> (BTreeSet<String>, BTreeSet<String>, Vec<String>) {
    let mut linkable = BTreeSet::new();
    // Same rule stances as the missing-source gate: rules that cannot run
    // in this configuration contribute no requirements (issues #493/#616).
    let mut rules = Vec::new();
    for name in order {
        let Some(rule) = config.get_rule(name) else {
            continue;
        };
        if run_preview::when_condition_false(rule, config, wildcard_values) {
            continue;
        }
        if rule.scatter.as_ref().is_some_and(|scatter| {
            if !scatter.values.is_empty() {
                return false;
            }
            match scatter.values_from.as_deref() {
                None => true,
                Some(values_from) => config
                    .resolve_config_list(values_from)
                    .is_some_and(|values| values.is_empty()),
            }
        }) {
            continue;
        }
        if rule.optional.is_optional() {
            continue;
        }
        rules.push(rule);
    }

    for rule in &rules {
        for pattern in rendered_input_patterns(rule, wildcard_values) {
            if dag.producer_of(&pattern).is_some() || reference_outputs.contains(&pattern) {
                continue; // generated intermediate, owned by the DAG
            }
            for file in resolve_pattern_files(&pattern, workflow_dir) {
                if let Some(rel) = rel_under(&file, workflow_dir) {
                    linkable.insert(rel);
                }
            }
        }
        // Interpreter script arguments (`python scripts/tool.py`) are
        // read-by-contract and already vetted by the D001 deep check.
        let shell = rendered_shell_text(rule, config);
        for token in interpreter_script_candidates(&shell) {
            if dag.producer_of(&token).is_some() || reference_outputs.contains(&token) {
                continue;
            }
            for file in resolve_pattern_files(&token, workflow_dir) {
                if let Some(rel) = rel_under(&file, workflow_dir) {
                    linkable.insert(rel);
                }
            }
        }
    }

    // Tools that auto-derive companion indexes from a declared file's path
    // (`bwa mem ref.fa` reads `ref.fa.bwt`; `samtools faidx`-style `.fai`;
    // picard's `.dict`) never mention those paths in the shell text, so no
    // token analysis can see them. Link every sidecar that exists in the
    // repo beside a linked file — unless the DAG produces the sidecar
    // itself (e.g. a dedicated `samtools faidx` indexing rule), in which
    // case it is a generated intermediate like any other.
    let mut sidecars = BTreeSet::new();
    for rel in &linkable {
        for suffix in SIDECAR_SUFFIXES {
            let candidate = format!("{rel}{suffix}");
            if linkable.contains(&candidate)
                || sidecars.contains(&candidate)
                || dag.producer_of(&candidate).is_some()
                || reference_outputs.contains(&candidate)
            {
                continue;
            }
            if workflow_dir.join(&candidate).is_file() {
                sidecars.insert(candidate);
            }
        }
    }
    linkable.extend(sidecars.iter().cloned());

    // Everything else a shell command mentions as a repo path is reported,
    // not linked: with no input declaration there is no read contract, and
    // linking a path the command later writes would write through into the
    // repo.
    let mut undeclared = BTreeSet::new();
    for rule in &rules {
        let shell = rendered_shell_text(rule, config);
        for token in shell.split_whitespace() {
            let token = token.trim_matches(|c: char| {
                matches!(c, '\'' | '"' | ';' | '&' | ')' | ']')
                    || c.is_ascii_punctuation() && !matches!(c, '/' | '.' | '-' | '_' | '+')
            });
            if !is_path_like_token(token) {
                continue;
            }
            if linkable.contains(token)
                || reference_outputs.contains(token)
                || dag.producer_of(token).is_some()
            {
                continue;
            }
            if workflow_dir.join(token).is_file() {
                undeclared.insert(format!("{} (rule '{}')", token, rule.name));
            }
        }
    }
    (linkable, sidecars, undeclared.into_iter().collect())
}

/// Path-like shell token worth checking against the repo: has a directory
/// component, no guard characters, no flags/placeholders (mirrors
/// `deep_check::is_script_candidate`'s conservatism).
fn is_path_like_token(token: &str) -> bool {
    token.len() > 1
        && token.contains('/')
        && !token.starts_with('-')
        && !token.contains([
            '{', '}', '$', '=', '`', '\'', '"', '(', ')', '?', ';', '|', '<', '>', '*',
        ])
        && !token.starts_with(".oxo-flow")
}

fn rel_under(file: &Path, workflow_dir: &Path) -> Option<String> {
    file.strip_prefix(workflow_dir)
        .ok()
        .map(|p| p.to_string_lossy().replace('\\', "/"))
}

#[derive(Clone, Copy, PartialEq)]
enum LinkOutcome {
    Created,
    Converted,
    Idempotent,
    LeftAlone,
    Conflict,
}

/// Materialize one repo file at its relative path inside the workdir.
fn link_one(rel: &str, workflow_dir: &Path, workdir: &Path) -> anyhow::Result<LinkOutcome> {
    let repo_path = workflow_dir.join(rel);
    let wd_path = workdir.join(rel);
    if !repo_path.is_file() {
        return Ok(LinkOutcome::LeftAlone);
    }
    let repo_target = std::fs::canonicalize(&repo_path)?;
    if wd_path.is_symlink() {
        // Idempotent when the link (whatever its textual form) resolves to
        // the same repo file; a link aimed elsewhere is operator state.
        if std::fs::canonicalize(&wd_path).is_ok_and(|resolved| resolved == repo_target) {
            return Ok(LinkOutcome::Idempotent);
        }
        return Ok(LinkOutcome::LeftAlone);
    }
    if wd_path.exists() {
        if wd_path.is_dir() {
            // Never displace an operator-owned directory.
            return Ok(LinkOutcome::LeftAlone);
        }
        // An identical regular file is a redundant duplicate (#837's exact
        // complaint) — replace it with the link its content already is.
        let identical = match (compute_sha256(&wd_path), compute_sha256(&repo_target)) {
            (Ok(wd_hash), Ok(repo_hash)) => wd_hash == repo_hash,
            _ => false,
        };
        if identical {
            std::fs::remove_file(&wd_path)?;
            make_symlink(&repo_target, &wd_path)?;
            return Ok(LinkOutcome::Converted);
        }
        // A diverging workdir copy may be a deliberate override: keep it
        // and let the caller warn — silent staleness was the #837 hazard.
        return Ok(LinkOutcome::Conflict);
    }
    make_symlink(&repo_target, &wd_path)?;
    Ok(LinkOutcome::Created)
}

fn make_symlink(repo_target: &Path, wd_path: &Path) -> anyhow::Result<()> {
    if let Some(parent) = wd_path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    #[cfg(unix)]
    {
        std::os::unix::fs::symlink(repo_target, wd_path)?;
    }
    #[cfg(not(unix))]
    {
        std::fs::symlink(repo_target, wd_path)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(content: &str) -> WorkflowConfig {
        WorkflowConfig::parse(content).expect("workflow parses")
    }

    fn write(path: &Path, content: &str) {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).unwrap();
        }
        std::fs::write(path, content).unwrap();
    }

    const WF: &str = r#"
[workflow]
name = "t"
version = "1"

[config]
tools = "scripts"

[[rules]]
name = "report"
input = ["notes/source.txt"]
output = ["out.txt"]
shell = "python {config.tools}/tool.py --in notes/source.txt > out.txt"
"#;

    fn setup_repo(root: &Path) -> WorkflowConfig {
        write(&root.join("notes/source.txt"), "source-of-truth\n");
        write(&root.join("scripts/tool.py"), "print('hi')\n");
        parse(WF)
    }

    #[test]
    fn candidates_cover_declared_inputs_and_scripts() {
        let tmp = tempfile::tempdir().unwrap();
        let config = setup_repo(tmp.path());
        let dag = WorkflowDag::from_rules(&config.rules).unwrap();
        let order = dag.execution_order().unwrap();
        let (linkable, sidecars, undeclared) = source_link_candidates(
            &config,
            &dag,
            &order,
            tmp.path(),
            &HashMap::new(),
            &HashSet::new(),
        );
        assert_eq!(
            linkable.into_iter().collect::<Vec<_>>(),
            vec!["notes/source.txt", "scripts/tool.py"]
        );
        assert!(sidecars.is_empty());
        // `--in notes/source.txt` is declared, so it is not an undeclared ref.
        assert!(undeclared.is_empty());
    }

    #[test]
    fn undeclared_shell_reference_is_reported_not_linked() {
        let repo = tempfile::tempdir().unwrap();
        write(&repo.path().join("extra/pairs.tsv"), "pairs\n");
        write(&repo.path().join("notes/source.txt"), "source\n");
        write(&repo.path().join("scripts/tool.py"), "print('hi')\n");
        // `extra/pairs.tsv` is referenced by the shell but declared nowhere.
        let wf = WF.replace(
            "--in notes/source.txt > out.txt",
            "--in notes/source.txt --pairs extra/pairs.tsv > out.txt",
        );
        let config = parse(&wf);
        let dag = WorkflowDag::from_rules(&config.rules).unwrap();
        let order = dag.execution_order().unwrap();
        let (linkable, sidecars, undeclared) = source_link_candidates(
            &config,
            &dag,
            &order,
            repo.path(),
            &HashMap::new(),
            &HashSet::new(),
        );
        assert!(!linkable.contains("extra/pairs.tsv"));
        assert!(sidecars.is_empty());
        assert_eq!(undeclared, vec!["extra/pairs.tsv (rule 'report')"]);
    }

    #[test]
    fn tool_convention_index_sidecars_are_linked_without_declaration() {
        let repo = tempfile::tempdir().unwrap();
        let wd = tempfile::tempdir().unwrap();
        // `reference/ref.fa` is the only declared input; the bwa/picard/
        // samtools companion indexes exist beside it but appear in no shell.
        write(&repo.path().join("reference/ref.fa"), "ACGT\n");
        write(&repo.path().join("reference/ref.fa.amb"), "amb\n");
        write(&repo.path().join("reference/ref.fa.bwt"), "bwt\n");
        write(&repo.path().join("reference/ref.fa.fai"), "fai\n");
        write(&repo.path().join("reference/ref.fa.dict"), "dict\n");
        write(&repo.path().join("scripts/tool.py"), "print('hi')\n");
        let wf = r#"
[workflow]
name = "t"
version = "1"

[[rules]]
name = "align"
input = ["reference/ref.fa"]
output = ["out.txt"]
shell = "python3 scripts/tool.py --ref reference/ref.fa > out.txt"
"#;
        let config = parse(wf);
        let dag = WorkflowDag::from_rules(&config.rules).unwrap();
        let order = dag.execution_order().unwrap();
        let report = link_workflow_sources(
            &config,
            &dag,
            &order,
            repo.path(),
            wd.path(),
            &HashMap::new(),
        );
        // ref.fa itself + 4 sidecars + the interpreter script.
        assert_eq!(report.linked.len(), 6, "{report:?}");
        assert_eq!(
            report.sidecars,
            vec![
                "reference/ref.fa.amb",
                "reference/ref.fa.bwt",
                "reference/ref.fa.dict",
                "reference/ref.fa.fai",
            ],
            "{report:?}"
        );
        for rel in &report.sidecars {
            assert!(wd.path().join(rel).is_symlink(), "{rel} not linked");
        }
    }

    #[test]
    fn dag_produced_sidecar_stays_a_generated_intermediate() {
        let repo = tempfile::tempdir().unwrap();
        let wd = tempfile::tempdir().unwrap();
        write(&repo.path().join("notes/source.txt"), "source\n");
        // The DAG builds `notes/source.txt.fai` itself — the linker must
        // not displace it with a repo-side link.
        write(&repo.path().join("notes/source.txt.fai"), "stale\n");
        let wf = r#"
[workflow]
name = "t"
version = "1"

[[rules]]
name = "report"
input = ["notes/source.txt"]
output = ["out.txt"]
shell = "cat notes/source.txt > out.txt"

[[rules]]
name = "index"
input = ["notes/source.txt"]
output = ["notes/source.txt.fai"]
shell = "printf idx > notes/source.txt.fai"
"#;
        let config = parse(wf);
        let dag = WorkflowDag::from_rules(&config.rules).unwrap();
        let order = dag.execution_order().unwrap();
        let report = link_workflow_sources(
            &config,
            &dag,
            &order,
            repo.path(),
            wd.path(),
            &HashMap::new(),
        );
        assert_eq!(report.linked, vec!["notes/source.txt"], "{report:?}");
        assert!(report.sidecars.is_empty(), "{report:?}");
    }

    #[test]
    fn linking_creates_symlinks_and_converts_identical_copies() {
        let repo = tempfile::tempdir().unwrap();
        let wd = tempfile::tempdir().unwrap();
        let config = setup_repo(repo.path());
        let dag = WorkflowDag::from_rules(&config.rules).unwrap();
        let order = dag.execution_order().unwrap();

        // Fresh workdir: both repo files appear as symlinks.
        let report = link_workflow_sources(
            &config,
            &dag,
            &order,
            repo.path(),
            wd.path(),
            &HashMap::new(),
        );
        assert_eq!(report.linked.len(), 2, "{report:?}");
        assert!(report.conflicts.is_empty() && report.errors.is_empty());
        assert!(wd.path().join("notes/source.txt").is_symlink());
        assert!(wd.path().join("scripts/tool.py").is_symlink());

        // Re-run is a no-op.
        let again = link_workflow_sources(
            &config,
            &dag,
            &order,
            repo.path(),
            wd.path(),
            &HashMap::new(),
        );
        assert!(again.is_empty(), "{again:?}");

        // A byte-identical duplicate becomes a symlink; content survives.
        std::fs::remove_file(wd.path().join("notes/source.txt")).unwrap();
        write(&wd.path().join("notes/source.txt"), "source-of-truth\n");
        let report = link_workflow_sources(
            &config,
            &dag,
            &order,
            repo.path(),
            wd.path(),
            &HashMap::new(),
        );
        assert_eq!(report.converted, vec!["notes/source.txt"], "{report:?}");
        assert_eq!(
            std::fs::read_to_string(wd.path().join("notes/source.txt")).unwrap(),
            "source-of-truth\n"
        );

        // A diverging copy is left in place and reported as a conflict.
        std::fs::remove_file(wd.path().join("scripts/tool.py")).unwrap();
        write(&wd.path().join("scripts/tool.py"), "print('local edit')\n");
        let report = link_workflow_sources(
            &config,
            &dag,
            &order,
            repo.path(),
            wd.path(),
            &HashMap::new(),
        );
        assert_eq!(report.conflicts, vec!["scripts/tool.py"], "{report:?}");
        assert_eq!(
            std::fs::read_to_string(wd.path().join("scripts/tool.py")).unwrap(),
            "print('local edit')\n"
        );
    }

    #[test]
    fn single_directory_mode_is_a_no_op() {
        let tmp = tempfile::tempdir().unwrap();
        let config = setup_repo(tmp.path());
        let dag = WorkflowDag::from_rules(&config.rules).unwrap();
        let order = dag.execution_order().unwrap();
        let report = link_workflow_sources(
            &config,
            &dag,
            &order,
            tmp.path(),
            tmp.path(),
            &HashMap::new(),
        );
        assert!(report.is_empty());
    }

    #[test]
    fn workdir_inside_repo_is_skipped() {
        let repo = tempfile::tempdir().unwrap();
        let config = setup_repo(repo.path());
        let dag = WorkflowDag::from_rules(&config.rules).unwrap();
        let order = dag.execution_order().unwrap();
        let wd = repo.path().join("scratch-run");
        std::fs::create_dir_all(&wd).unwrap();
        let report =
            link_workflow_sources(&config, &dag, &order, repo.path(), &wd, &HashMap::new());
        assert!(report.skipped_note.is_some());
        assert!(report.linked.is_empty());
    }

    #[test]
    fn path_like_token_filter_rejects_flags_and_placeholders() {
        assert!(is_path_like_token("scripts/tool.py"));
        assert!(is_path_like_token("config/pairs.tsv"));
        assert!(!is_path_like_token("--output"));
        assert!(!is_path_like_token("{config.data_dir}"));
        assert!(!is_path_like_token("$HOME/data/x.tsv"));
        assert!(!is_path_like_token("KEY=value"));
        assert!(!is_path_like_token(".oxo-flow/state.json"));
        assert!(!is_path_like_token("python"));
    }
}
