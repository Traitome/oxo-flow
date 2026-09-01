//! Software-versions collection and nf-core-style `versions.yml` emission
//! (issue #280).
//!
//! Static mode: the engine never executes anything to produce this data, so
//! it records what the workflow *declares* — docker `image:tag`, module
//! `tool/version`, conda/mamba/pixi env files with content hashes. Resolved
//! runtime package versions depend on the execution environment and are
//! unknowable statically; every entry carries that caveat rather than
//! pretending otherwise (issue #83 P0-6: real facts only).

use crate::config::WorkflowConfig;
use crate::executor::CheckpointState;
use crate::executor::checkpoint::compute_file_checksum;
use std::path::{Path, PathBuf};

/// `schema_version` of the emitted `versions.yml` document.
pub const VERSIONS_SCHEMA_VERSION: u32 = 1;

/// One rule's declared software environment.
#[derive(Debug, Clone, PartialEq)]
pub struct SoftwareVersionEntry {
    /// `"workflow:rule"` composite key (nf-core convention).
    pub key: String,
    /// Rule name.
    pub rule: String,
    /// Environment backend kind (`docker`, `conda`, `modules`, `system`, ...).
    pub environment: String,
    /// Declared spec fields in stable `(field, value)` order (docker image,
    /// env-file path + content hash, bare env names, ...).
    pub specs: Vec<(String, String)>,
    /// Declared environment modules, `(name, Option<version>)`.
    pub modules: Vec<(String, Option<String>)>,
    /// Honest caveats about what this entry cannot tell you.
    pub notes: Vec<String>,
}

/// Full document behind both the report section and the YAML export.
#[derive(Debug, Clone, PartialEq)]
pub struct SoftwareVersionsDoc {
    pub schema_version: u32,
    pub workflow_name: String,
    pub workflow_version: String,
    pub engine_version: String,
    pub workflow_git_sha: Option<String>,
    pub entries: Vec<SoftwareVersionEntry>,
    /// Declared reference databases: `(name, version, checksum)`.
    pub references: Vec<(String, Option<String>, Option<String>)>,
}

impl SoftwareVersionsDoc {
    /// Document-level honesty note.
    pub fn note(&self) -> &'static str {
        "Static declaration extracted from the workflow definition. Resolved \
         runtime package versions depend on the execution environment and are \
         not recorded here; use `oxo-flow report --versions-yml` to export and \
         diff this file in CI."
    }

    /// Render the document as a flat YAML file with no external YAML
    /// dependency (the workspace deliberately has none). Byte-stable for a
    /// byte-stable input: every iteration order is deterministic.
    pub fn to_yaml(&self) -> String {
        let mut out = String::new();
        out.push_str(&format!("schema_version: {}\n", self.schema_version));
        out.push_str("workflow:\n");
        out.push_str(&format!("  name: {}\n", yaml_scalar(&self.workflow_name)));
        out.push_str(&format!(
            "  version: {}\n",
            yaml_scalar(&self.workflow_version)
        ));
        out.push_str(&format!(
            "  oxoflow: {}\n",
            yaml_scalar(&self.engine_version)
        ));
        if let Some(sha) = &self.workflow_git_sha {
            out.push_str(&format!("  git_sha: {}\n", yaml_scalar(sha)));
        }
        out.push_str(&format!("note: {}\n", yaml_scalar(self.note())));

        out.push_str("software:\n");
        for entry in &self.entries {
            out.push_str(&format!("  - key: {}\n", yaml_scalar(&entry.key)));
            out.push_str(&format!("    rule: {}\n", yaml_scalar(&entry.rule)));
            out.push_str(&format!(
                "    environment: {}\n",
                yaml_scalar(&entry.environment)
            ));
            for (field, value) in &entry.specs {
                out.push_str(&format!(
                    "    {}: {}\n",
                    yaml_scalar(field),
                    yaml_scalar(value)
                ));
            }
            if !entry.modules.is_empty() {
                out.push_str("    modules:\n");
                for (name, version) in &entry.modules {
                    out.push_str(&format!("      - name: {}\n", yaml_scalar(name)));
                    if let Some(v) = version {
                        out.push_str(&format!("        version: {}\n", yaml_scalar(v)));
                    }
                }
            }
            if !entry.notes.is_empty() {
                out.push_str("    notes:\n");
                for note in &entry.notes {
                    out.push_str(&format!("      - {}\n", yaml_scalar(note)));
                }
            }
        }

        if !self.references.is_empty() {
            out.push_str("references:\n");
            for (name, version, checksum) in &self.references {
                out.push_str(&format!("  - name: {}\n", yaml_scalar(name)));
                if let Some(v) = version {
                    out.push_str(&format!("    version: {}\n", yaml_scalar(v)));
                }
                if let Some(c) = checksum {
                    out.push_str(&format!("    checksum: {}\n", yaml_scalar(c)));
                }
            }
        }
        out
    }
}

/// Collect declared software versions from the workflow definition,
/// mirroring `WorkflowConfig::resolve_environment` precedence (env_group →
/// rule.environment → defaults.environment) by calling it directly.
pub fn collect_software_versions(
    config: &WorkflowConfig,
    checkpoint: Option<&CheckpointState>,
    workflow_path: Option<&Path>,
) -> SoftwareVersionsDoc {
    // Env-file specs are relative to the workflow file's directory (same
    // resolution deep_check uses for environment candidates).
    let base_dir: Option<PathBuf> = workflow_path
        .map(crate::parent_dir)
        .map(|p| p.to_path_buf());

    let entries: Vec<SoftwareVersionEntry> = config
        .rules
        .iter()
        .map(|rule| {
            let env = config.resolve_environment(rule).unwrap_or_default();
            let mut specs: Vec<(String, String)> = Vec::new();
            let mut modules: Vec<(String, Option<String>)> = Vec::new();
            let mut notes: Vec<String> = Vec::new();

            if let Some(spec) = &env.conda {
                push_env_file_like("conda", spec, base_dir.as_deref(), &mut specs, &mut notes);
            }
            if let Some(spec) = &env.mamba {
                push_env_file_like("mamba", spec, base_dir.as_deref(), &mut specs, &mut notes);
            }
            if let Some(spec) = &env.pixi {
                push_manifest_file(
                    "pixi_manifest",
                    spec,
                    base_dir.as_deref(),
                    &mut specs,
                    &mut notes,
                );
            }
            if let Some(image) = &env.docker {
                // nf-core convention is `image:tag`; keep the stripped
                // registry as its own field so no fact is lost.
                let stripped = strip_registry(image);
                if stripped.len() < image.len() {
                    let registry = &image[..image.len() - stripped.len() - 1];
                    specs.push(("docker_registry".into(), registry.into()));
                }
                specs.push(("docker".into(), stripped.into()));
            }
            if let Some(spec) = &env.singularity {
                specs.push(("singularity".into(), spec.clone()));
            }
            if let Some(dir) = &env.venv {
                specs.push(("venv".into(), dir.clone()));
            }
            if let Some(req) = &env.venv_requirements {
                push_manifest_file(
                    "venv_requirements",
                    req,
                    base_dir.as_deref(),
                    &mut specs,
                    &mut notes,
                );
            }
            for module in &env.modules {
                let (name, version) = split_module(module);
                if version.is_none() {
                    notes.push(format!("module '{module}' declares no version segment"));
                }
                modules.push((name, version));
            }
            if env.is_empty() {
                notes.push(
                    "system environment — no software versions are declared \
                     for this rule"
                        .to_string(),
                );
            }

            SoftwareVersionEntry {
                key: format!("{}:{}", config.workflow.name, rule.name),
                rule: rule.name.clone(),
                environment: env.kind().to_string(),
                specs,
                modules,
                notes,
            }
        })
        .collect();

    let references = config
        .reference_databases
        .iter()
        .map(|db| (db.name.clone(), db.version.clone(), db.checksum.clone()))
        .collect();

    SoftwareVersionsDoc {
        schema_version: VERSIONS_SCHEMA_VERSION,
        workflow_name: config.workflow.name.clone(),
        workflow_version: config.workflow.version.clone(),
        engine_version: env!("CARGO_PKG_VERSION").to_string(),
        workflow_git_sha: checkpoint.and_then(|c| c.workflow_git_sha.clone()),
        entries,
        references,
    }
}

/// True when a conda/mamba spec looks like a repository file (has a
/// directory component or a YAML extension) rather than a bare env name —
/// same distinction deep_check's `looks_file_like` draws.
fn looks_file_like(value: &str) -> bool {
    value.contains('/') || value.ends_with(".yaml") || value.ends_with(".yml")
}

/// Record a conda/mamba spec: file-backed specs get their path plus a
/// content hash (the only version identity available statically); bare env
/// names are recorded honestly with a note instead of a fabricated version.
fn push_env_file_like(
    field: &str,
    spec: &str,
    base_dir: Option<&Path>,
    specs: &mut Vec<(String, String)>,
    notes: &mut Vec<String>,
) {
    if !looks_file_like(spec) {
        specs.push((format!("{field}_env"), spec.to_string()));
        notes.push(format!(
            "{field} is a bare environment name — its package versions are \
             not recorded statically"
        ));
        return;
    }
    let resolved = resolve_under(spec, base_dir);
    match compute_file_checksum(&resolved) {
        Ok(hash) => {
            specs.push((format!("{field}_file"), spec.to_string()));
            specs.push((format!("{field}_file_sha256"), hash));
            specs.push((format!("{field}_env"), env_name_from_file(&resolved, spec)));
        }
        Err(_) => {
            specs.push((format!("{field}_file"), spec.to_string()));
            notes.push(format!(
                "{field} env file could not be read for a content hash"
            ));
        }
    }
}

/// Record a manifest/requirements file spec (pixi.toml, venv requirements):
/// path + content hash when readable.
fn push_manifest_file(
    field: &str,
    spec: &str,
    base_dir: Option<&Path>,
    specs: &mut Vec<(String, String)>,
    notes: &mut Vec<String>,
) {
    let resolved = resolve_under(spec, base_dir);
    match compute_file_checksum(&resolved) {
        Ok(hash) => {
            specs.push((field.to_string(), spec.to_string()));
            specs.push((format!("{field}_sha256"), hash));
        }
        Err(_) => {
            specs.push((field.to_string(), spec.to_string()));
            notes.push(format!("{field} could not be read for a content hash"));
        }
    }
}

/// Join a possibly-relative path onto the workflow directory (absolute
/// paths untouched) — deep_check's resolution rule.
fn resolve_under(spec: &str, base_dir: Option<&Path>) -> PathBuf {
    let path = Path::new(spec);
    match base_dir {
        Some(base) if !path.is_absolute() => base.join(path),
        _ => path.to_path_buf(),
    }
}

/// Environment name for a file-backed conda spec: the `name:` field from
/// the YAML when present, else the file stem (environment.rs's derivation,
/// minus the content-hash suffix that only matters for env-directory naming).
fn env_name_from_file(path: &Path, spec: &str) -> String {
    let from_yaml = std::fs::read(path)
        .ok()
        .and_then(|bytes| String::from_utf8(bytes).ok())
        .and_then(|content| {
            content.lines().find_map(|line| {
                let trimmed = line.trim();
                let is_name = trimmed.starts_with("name:") || trimmed.starts_with("name :");
                if !is_name {
                    return None;
                }
                let value = trimmed
                    .split_once(':')
                    .map(|(_, v)| v)
                    .unwrap_or("")
                    .trim()
                    .trim_matches('"')
                    .trim_matches('\'');
                (!value.is_empty()).then(|| value.to_string())
            })
        });
    from_yaml.unwrap_or_else(|| {
        Path::new(spec)
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or(spec)
            .to_string()
    })
}

/// Strip a leading registry host (`ghcr.io/org/tool:1.0` → `org/tool:1.0`)
/// per Docker's own convention: the first path component is a registry only
/// when it contains `.`/`:` or is `localhost`.
fn strip_registry(image: &str) -> &str {
    match image.split_once('/') {
        Some((registry, rest))
            if registry.contains('.') || registry.contains(':') || registry == "localhost" =>
        {
            rest
        }
        _ => image,
    }
}

/// Split an environment-module string into `(name, Option<version>)` on the
/// last `/`, treating the tail as a version only when it looks like one.
fn split_module(module: &str) -> (String, Option<String>) {
    match module.rsplit_once('/') {
        Some((name, version)) if looks_like_version(version) => {
            (name.to_string(), Some(version.to_string()))
        }
        _ => (module.to_string(), None),
    }
}

fn looks_like_version(value: &str) -> bool {
    let digits = value.trim_start_matches(['v', 'V']);
    !digits.is_empty() && digits.chars().next().is_some_and(|c| c.is_ascii_digit())
}

/// Quote a YAML scalar unless it is made only of safe characters and would
/// not be re-parsed as a number/bool (a bare `version: 4` must stay a
/// string).
fn yaml_scalar(value: &str) -> String {
    let safe = !value.is_empty()
        && value
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.' | '/' | '@' | '+'));
    let unambiguous = !(value.parse::<f64>().is_ok()
        || matches!(
            value.to_ascii_lowercase().as_str(),
            "true" | "false" | "null" | "~" | "yes" | "no" | "on" | "off"
        ));
    if safe && unambiguous {
        value.to_string()
    } else {
        format!("'{}'", value.replace('\'', "''"))
    }
}

// ── Report section generator (issue #280) ─────────────────────────────────

use crate::report::{ReportContent, ReportContext, ReportSection, ReportSectionGenerator};

/// The 13th section: declared software versions per rule, nf-core-style.
/// Filterable like every other section — `[report].sections` calls
/// generate() without consulting applicable(), so guard here.
pub struct SoftwareVersionsGenerator;
impl ReportSectionGenerator for SoftwareVersionsGenerator {
    fn name(&self) -> &str {
        "software-versions"
    }
    fn description(&self) -> &str {
        "Declared software versions per rule (nf-core-style, static)"
    }
    fn applicable(&self, _ctx: &ReportContext) -> bool {
        true
    }
    fn generate(&self, ctx: &ReportContext) -> Vec<ReportSection> {
        let doc = collect_software_versions(ctx.config, ctx.checkpoint, ctx.workflow_path);
        if doc.entries.is_empty() {
            return Vec::new();
        }
        let headers = vec![
            "Task".to_string(),
            "Key".to_string(),
            "Environment".to_string(),
            "Declared Software".to_string(),
            "Notes".to_string(),
        ];
        let rows: Vec<Vec<String>> = doc
            .entries
            .iter()
            .map(|e| {
                let mut declared: Vec<String> =
                    e.specs.iter().map(|(f, v)| format!("{f}={v}")).collect();
                declared.extend(e.modules.iter().map(|(n, v)| match v {
                    Some(v) => format!("{n}/{v}"),
                    None => n.clone(),
                }));
                vec![
                    e.rule.clone(),
                    e.key.clone(),
                    e.environment.clone(),
                    if declared.is_empty() {
                        "-".to_string()
                    } else {
                        declared.join(", ")
                    },
                    if e.notes.is_empty() {
                        "-".to_string()
                    } else {
                        e.notes.join("; ")
                    },
                ]
            })
            .collect();
        vec![ReportSection {
            title: "Software Versions".into(),
            id: "software-versions".into(),
            content: ReportContent::Table { headers, rows },
            subsections: vec![ReportSection {
                title: "About This Section".into(),
                id: "software-versions-about".into(),
                content: ReportContent::Text {
                    text: "Static declaration extracted from the workflow \
                           definition (issue #280): docker image:tag, module \
                           tool/version, and env-file content hashes. Resolved \
                           runtime package versions depend on the execution \
                           environment and are not recorded here. Export a \
                           machine-readable copy with `oxo-flow report \
                           --versions-yml`."
                        .to_string(),
                },
                subsections: vec![],
            }],
        }]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Parse a workflow from a TOML body written into a tempdir (the
    /// config module's test pattern). Rule environments are nested tables
    /// (`[rules.environment]`), so an `env.<field> = …` line shorthand is
    /// rewritten into that shape.
    fn config_from(extra: &str) -> (WorkflowConfig, PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("wf.oxoflow");
        let mut body = String::new();
        // Rule blocks are blank-line separated; `env.<field> = …` shorthands
        // inside a block must become a `[rules.environment]` table, which in
        // TOML swallows every following bare key — so hoist them to the END
        // of their block.
        for block in extra.split("\n\n") {
            let mut plain = String::new();
            let mut env_lines = Vec::new();
            for line in block.lines() {
                match line.strip_prefix("env.") {
                    Some(rest) => env_lines.push(rest.trim().to_string()),
                    None => {
                        plain.push_str(line);
                        plain.push('\n');
                    }
                }
            }
            body.push_str(&plain);
            if !env_lines.is_empty() {
                body.push_str("[rules.environment]\n");
                for line in env_lines {
                    body.push_str(&line);
                    body.push('\n');
                }
            }
            body.push('\n');
        }
        std::fs::write(
            &path,
            format!("[workflow]\nname = \"test\"\nversion = \"0.1\"\n{body}"),
        )
        .unwrap();
        (WorkflowConfig::from_file(&path).unwrap(), path)
    }

    #[test]
    fn docker_registry_stripped_but_kept() {
        let (config, path) = config_from(
            "[[rules]]\nname = \"trim\"\nenv.docker = \"ghcr.io/traitome/fastqk:0.12.1\"\nshell = \"x\"\n",
        );
        let doc = collect_software_versions(&config, None, Some(&path));
        let e = &doc.entries[0];
        assert_eq!(e.key, "test:trim");
        assert_eq!(e.environment, "docker");
        assert!(
            e.specs
                .contains(&("docker_registry".into(), "ghcr.io".into()))
        );
        assert!(
            e.specs
                .contains(&("docker".into(), "traitome/fastqk:0.12.1".into()))
        );
    }

    #[test]
    fn docker_unqualified_image_has_no_registry_field() {
        let (config, path) = config_from(
            "[[rules]]\nname = \"t\"\nenv.docker = \"biocontainers/samtools:1.19\"\nshell = \"x\"\n",
        );
        let doc = collect_software_versions(&config, None, Some(&path));
        assert!(
            !doc.entries[0]
                .specs
                .iter()
                .any(|(f, _)| f == "docker_registry")
        );
    }

    #[test]
    fn conda_env_file_gets_name_and_content_hash() {
        let (config, path) =
            config_from("[[rules]]\nname = \"qc\"\nenv.conda = \"envs/qc.yaml\"\nshell = \"x\"\n");
        std::fs::create_dir_all(path.parent().unwrap().join("envs")).unwrap();
        std::fs::write(
            path.parent().unwrap().join("envs/qc.yaml"),
            "name: qc-tools\nchannels: []\n",
        )
        .unwrap();
        let doc = collect_software_versions(&config, None, Some(&path));
        let e = &doc.entries[0];
        assert!(e.specs.contains(&("conda_env".into(), "qc-tools".into())));
        assert!(
            e.specs
                .contains(&("conda_file".into(), "envs/qc.yaml".into()))
        );
        assert!(
            e.specs
                .iter()
                .any(|(f, v)| f == "conda_file_sha256" && v.starts_with("sha256:"))
        );
        assert!(e.notes.is_empty());
    }

    #[test]
    fn bare_conda_name_recorded_honestly() {
        let (config, path) =
            config_from("[[rules]]\nname = \"t\"\nenv.conda = \"bioinformatics\"\nshell = \"x\"\n");
        let doc = collect_software_versions(&config, None, Some(&path));
        let e = &doc.entries[0];
        assert!(
            e.specs
                .contains(&("conda_env".into(), "bioinformatics".into()))
        );
        assert!(e.notes.iter().any(|n| n.contains("bare environment name")));
    }

    #[test]
    fn env_group_beats_inline_beats_defaults() {
        let (config, path) = config_from(
            "[defaults.environment]\ndocker = \"fallback:1\"\n\n[env_groups.g1]\ndocker = \"group:2\"\n\n\
             [[rules]]\nname = \"a\"\nenv_group = \"g1\"\nenv.docker = \"inline:3\"\nshell = \"x\"\n\n\
             [[rules]]\nname = \"b\"\nenv.docker = \"inline:3\"\nshell = \"x\"\n\n\
             [[rules]]\nname = \"c\"\nshell = \"x\"\n",
        );
        let doc = collect_software_versions(&config, None, Some(&path));
        let docker = |n: &str| {
            doc.entries
                .iter()
                .find(|e| e.rule == n)
                .unwrap()
                .specs
                .iter()
                .find(|(f, _)| f == "docker")
                .unwrap()
                .1
                .clone()
        };
        assert_eq!(docker("a"), "group:2");
        assert_eq!(docker("b"), "inline:3");
        assert_eq!(docker("c"), "fallback:1");
    }

    #[test]
    fn modules_split_name_version() {
        let (config, path) = config_from(
            "[[rules]]\nname = \"m\"\nenv.modules = [\"gatk/4.4.0.0\", \"samtools\"]\nshell = \"x\"\n",
        );
        let doc = collect_software_versions(&config, None, Some(&path));
        let e = &doc.entries[0];
        assert_eq!(e.environment, "modules");
        assert_eq!(
            e.modules,
            vec![
                ("gatk".into(), Some("4.4.0.0".into())),
                ("samtools".into(), None),
            ]
        );
        assert!(e.notes.iter().any(|n| n.contains("samtools")));
    }

    #[test]
    fn yaml_is_byte_stable_and_shaped() {
        let (config, path) =
            config_from("[[rules]]\nname = \"t\"\nenv.docker = \"tool:4\"\nshell = \"x\"\n");
        let a = collect_software_versions(&config, None, Some(&path)).to_yaml();
        let b = collect_software_versions(&config, None, Some(&path)).to_yaml();
        assert_eq!(a, b);
        assert!(a.starts_with("schema_version: 1\nworkflow:"));
        // Keys contain a colon (`workflow:rule`), so they are single-quoted.
        assert!(a.contains("  - key: 'test:t'\n"));
        // A bare numeric version stays a string.
        assert!(a.contains("docker: 'tool:4'"));
    }

    #[test]
    fn yaml_quotes_hostile_rule_names() {
        let (config, path) = config_from(
            "[[rules]]\nname = \"weird: rule'\\\"\"\nenv.docker = \"x:1\"\nshell = \"y\"\n",
        );
        let doc = collect_software_versions(&config, None, Some(&path));
        let yaml = doc.to_yaml();
        // Single-quoted, with the embedded single quote doubled ('' in YAML).
        assert!(yaml.contains("- key: 'test:weird: rule''\"'"));
    }

    #[test]
    fn git_sha_flows_from_checkpoint() {
        let (config, path) = config_from("[[rules]]\nname = \"t\"\nshell = \"x\"\n");
        let cp = CheckpointState {
            workflow_git_sha: Some("abc123".into()),
            ..CheckpointState::default()
        };
        let doc = collect_software_versions(&config, Some(&cp), Some(&path));
        assert_eq!(doc.workflow_git_sha.as_deref(), Some("abc123"));
        assert!(doc.to_yaml().contains("git_sha: abc123"));
    }

    #[test]
    fn generator_emits_table_for_every_rule() {
        let (config, path) = config_from(
            "[[rules]]\nname = \"a\"\nshell = \"x\"\n\n[[rules]]\nname = \"b\"\nshell = \"y\"\n",
        );
        let ctx = crate::report::ReportContext {
            config: &config,
            checkpoint: None,
            domain: crate::report::WorkflowDomain::detect(&config.rules),
            workflow_path: Some(&path),
            checkpoint_path: None,
        };
        let sections = SoftwareVersionsGenerator.generate(&ctx);
        assert_eq!(sections.len(), 1);
        assert_eq!(sections[0].id, "software-versions");
        match &sections[0].content {
            ReportContent::Table { rows, .. } => assert_eq!(rows.len(), 2),
            other => panic!("expected Table, got {other:?}"),
        }
    }

    #[test]
    fn generator_registered_in_default_registry() {
        let registry = crate::report::SectionRegistry::with_defaults();
        let names: Vec<_> = registry.sections().into_iter().map(|(n, _)| n).collect();
        assert!(names.contains(&"software-versions"));
        assert_eq!(names.len(), 13);
    }
}
