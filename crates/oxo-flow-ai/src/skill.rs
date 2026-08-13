//! Skill system — reusable AI capability packages.
//!
//! Skills are versioned, shareable packages that extend the AI agent with
//! custom tools, knowledge, validators, and prompt templates.
//!
//! ## Skill Manifest
//!
//! Skills are defined via `.skill.toml` files, similar to the existing
//! plugin system (`.plugin.toml`). They can be:
//! - Built-in (compiled into the binary)
//! - User-level (`~/.oxo-flow/skills/`)
//! - Project-level (`<project>/.oxo-flow/skills/`)
//!
//! ## Architecture
//!
//! ```text
//! Skill → { tools, knowledge, validators, prompt_additions }
//!   ↓
//! AgentContext ← SkillRegistry.activate(skill_names)
//!   ↓
//! Agent.run() includes skill context
//! ```

use serde::{Deserialize, Serialize};
use std::path::PathBuf;

use crate::error::AiError;

// ── Skill manifest ─────────────────────────────────────────────────────────

/// A skill manifest loaded from a `.skill.toml` file.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SkillManifest {
    /// Unique skill name.
    pub name: String,
    /// Semantic version.
    pub version: String,
    /// Human-readable description.
    pub description: String,
    /// Author information.
    #[serde(default)]
    pub author: Option<String>,
    /// Domains this skill applies to (e.g., "RNA-seq", "variant-calling").
    #[serde(default)]
    pub domains: Vec<String>,
    /// Skill type: "tool", "knowledge", "validator", "composite".
    pub skill_type: String,
    /// Prompt additions to inject into the system prompt when activated.
    #[serde(default)]
    pub prompt_additions: Option<Vec<String>>,
    /// External tool dependencies (MCP server names, URLs).
    #[serde(default)]
    pub requires: Option<Vec<String>>,
    /// Skill entry point (reserved for future use).
    #[serde(default)]
    pub entry: Option<String>,
}

impl SkillManifest {
    /// Verify the skill manifest has required fields.
    pub fn validate(&self) -> Result<(), AiError> {
        if self.name.is_empty() {
            return Err(AiError::Config {
                message: "skill name is required".into(),
            });
        }
        if self.version.is_empty() {
            return Err(AiError::Config {
                message: format!("skill '{}' requires a version", self.name),
            });
        }
        Ok(())
    }
}

// ── Skill registry ─────────────────────────────────────────────────────────

/// Registry of activated skills for an agent session.
#[derive(Default)]
pub struct SkillRegistry {
    /// Loaded skill manifests.
    pub skills: Vec<SkillManifest>,
    /// Prompt additions collected from all activated skills.
    pub prompt_context: String,
}

impl SkillRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    /// Activate a skill by loading its manifest from a file.
    pub fn activate_from_file(&mut self, path: &std::path::Path) -> Result<(), AiError> {
        let content = std::fs::read_to_string(path).map_err(|e| AiError::Config {
            message: format!("cannot read skill at {}: {e}", path.display()),
        })?;
        let manifest: SkillManifest = toml::from_str(&content).map_err(|e| AiError::Config {
            message: format!("invalid skill manifest at {}: {e}", path.display()),
        })?;
        manifest.validate()?;
        self.activate(manifest);
        Ok(())
    }

    /// Activate a skill from a manifest.
    pub fn activate(&mut self, skill: SkillManifest) {
        // Collect prompt additions
        if let Some(ref additions) = skill.prompt_additions {
            for addition in additions {
                self.prompt_context
                    .push_str(&format!("\n## Skill: {}\n{addition}\n", skill.name));
            }
        }
        self.skills.push(skill);
    }

    /// Get the assembled prompt context from all activated skills.
    pub fn prompt_context(&self) -> &str {
        &self.prompt_context
    }

    /// Whether any skills are activated.
    pub fn is_empty(&self) -> bool {
        self.skills.is_empty()
    }

    /// Number of activated skills.
    pub fn len(&self) -> usize {
        self.skills.len()
    }
}

// ── Discovery ──────────────────────────────────────────────────────────────

/// Discover skills from standard directories.
pub fn discover_skills(project_dir: Option<&std::path::Path>) -> Vec<SkillManifest> {
    discover_skills_from(dirs_home(), project_dir)
}

/// Discovery with an injectable home directory (testability; the home is
/// otherwise derived from `$HOME`).
#[doc(hidden)]
pub fn discover_skills_from(
    home: Option<std::path::PathBuf>,
    project_dir: Option<&std::path::Path>,
) -> Vec<SkillManifest> {
    let mut manifests = Vec::new();

    // User-level skills
    if let Some(home) = home {
        let user_dir = home.join(".oxo-flow").join("skills");
        manifests.extend(scan_skill_dir(&user_dir));
        manifests.extend(scan_skill_md_dir(&user_dir));
    }

    // Project-level skills
    if let Some(proj) = project_dir {
        let proj_dir = proj.join(".oxo-flow").join("skills");
        manifests.extend(scan_skill_dir(&proj_dir));
        manifests.extend(scan_skill_md_dir(&proj_dir));
    }

    manifests
}

/// Scan a skills directory for `SKILL.md` files (the emerging skill
/// standard): `<skills>/<name>/SKILL.md` with YAML frontmatter
/// (`name`, `description`) and a markdown body that becomes the skill's
/// prompt content. Complements `.skill.toml` — the TOML form remains the
/// place for activation metadata (MCP `requires`, read-only flags).
fn scan_skill_md_dir(dir: &std::path::Path) -> Vec<SkillManifest> {
    let mut manifests = Vec::new();
    let Ok(entries) = std::fs::read_dir(dir) else {
        return manifests;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }
        let skill_md = path.join("SKILL.md");
        let Ok(content) = std::fs::read_to_string(&skill_md) else {
            continue;
        };
        if let Some(manifest) = parse_skill_md(&content) {
            manifests.push(manifest);
        }
    }
    manifests
}

/// Parse a `SKILL.md` file: YAML frontmatter (top-level scalar keys only)
/// followed by a markdown body. Returns None if the frontmatter lacks the
/// required `name`/`description` fields.
pub fn parse_skill_md(content: &str) -> Option<SkillManifest> {
    let body = content.strip_prefix("---")?;
    let (frontmatter, markdown) = body.split_once("---")?;

    let mut name = None;
    let mut description = None;
    let mut version = None;
    for line in frontmatter.lines() {
        let Some((key, value)) = line.split_once(':') else {
            continue;
        };
        // Frontmatter values: quoted strings or bare scalars. Nested
        // (metadata, license) entries are ignored deliberately.
        let value = value.trim().trim_matches(['"', '\'']);
        match key.trim() {
            "name" => name = Some(value.to_string()),
            "description" => description = Some(value.to_string()),
            "version" => version = Some(value.to_string()),
            _ => {}
        }
    }

    let body = markdown.trim();
    if body.is_empty() {
        return None;
    }

    Some(SkillManifest {
        name: name?,
        version: version.unwrap_or_else(|| "0.1.0".to_string()),
        description: description?,
        author: None,
        domains: Vec::new(),
        skill_type: "knowledge".to_string(),
        // The whole markdown body is the skill's guidance.
        prompt_additions: Some(vec![body.to_string()]),
        requires: None,
        entry: None,
    })
}

fn scan_skill_dir(dir: &std::path::Path) -> Vec<SkillManifest> {
    let mut manifests = Vec::new();
    let Ok(entries) = std::fs::read_dir(dir) else {
        return manifests;
    };

    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().is_some_and(|e| e == "toml")
            && path
                .file_stem()
                .is_some_and(|s| s.to_string_lossy().ends_with(".skill"))
            && let Ok(content) = std::fs::read_to_string(&path)
            && let Ok(manifest) = toml::from_str::<SkillManifest>(&content)
            && manifest.validate().is_ok()
        {
            manifests.push(manifest);
        }
    }

    manifests
}

fn dirs_home() -> Option<PathBuf> {
    std::env::var("HOME")
        .or_else(|_| std::env::var("USERPROFILE"))
        .ok()
        .map(PathBuf::from)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn skill_manifest_validate_passes() {
        let manifest = SkillManifest {
            name: "rnaseq-expert".into(),
            version: "1.0.0".into(),
            description: "RNA-seq expertise".into(),
            author: Some("Traitome".into()),
            domains: vec!["RNA-seq".into()],
            skill_type: "knowledge".into(),
            prompt_additions: Some(vec!["Use STAR for alignment".into()]),
            requires: None,
            entry: None,
        };
        assert!(manifest.validate().is_ok());
    }

    #[test]
    fn skill_manifest_validate_empty_name() {
        let manifest = SkillManifest {
            name: "".into(),
            version: "1.0".into(),
            description: "test".into(),
            author: None,
            domains: vec![],
            skill_type: "tool".into(),
            prompt_additions: None,
            requires: None,
            entry: None,
        };
        assert!(manifest.validate().is_err());
    }

    #[test]
    fn skill_manifest_serialization_roundtrip() {
        let manifest = SkillManifest {
            name: "test-skill".into(),
            version: "1.0.0".into(),
            description: "A test skill".into(),
            author: Some("Test Author".into()),
            domains: vec!["testing".into()],
            skill_type: "composite".into(),
            prompt_additions: Some(vec!["Prompt 1".into(), "Prompt 2".into()]),
            requires: Some(vec!["mcp-server-1".into()]),
            entry: Some("/usr/bin/test-skill".into()),
        };
        let json = serde_json::to_string(&manifest).unwrap();
        let back: SkillManifest = serde_json::from_str(&json).unwrap();
        assert_eq!(back.name, "test-skill");
        assert_eq!(back.prompt_additions.unwrap().len(), 2);
    }

    #[test]
    fn skill_registry_accumulates_context() {
        let mut registry = SkillRegistry::new();
        assert!(registry.is_empty());

        registry.activate(SkillManifest {
            name: "expert".into(),
            version: "1.0".into(),
            description: "Expert knowledge".into(),
            author: None,
            domains: vec!["RNA-seq".into()],
            skill_type: "knowledge".into(),
            prompt_additions: Some(vec!["Always include QC step".into()]),
            requires: None,
            entry: None,
        });

        assert_eq!(registry.len(), 1);
        assert!(registry.prompt_context().contains("QC step"));
    }

    #[test]
    fn skill_manifest_toml_format() {
        let toml_str = r#"
name = "qc-expert"
version = "1.0.0"
description = "QC best practices"
author = "Traitome"
domains = ["QC", "RNA-seq", "DNA-seq"]
skill_type = "knowledge"
prompt_additions = [
    "Always run fastp before alignment",
    "Include multiqc for aggregation"
]
"#;
        let manifest: SkillManifest = toml::from_str(toml_str).unwrap();
        assert_eq!(manifest.name, "qc-expert");
        assert_eq!(manifest.domains.len(), 3);
        assert_eq!(manifest.prompt_additions.unwrap().len(), 2);
    }
    #[test]
    fn parse_skill_md_reads_frontmatter_and_body() {
        let content = "---\nname: rnaseq-reviewer\ndescription: Reviews RNA-seq pipelines for strandedness mistakes\nversion: 1.2.0\nlicense: MIT\n---\n# Guidance\nAlways check featureCounts -s matches the library prep.\n";
        let manifest = parse_skill_md(content).expect("valid SKILL.md");
        assert_eq!(manifest.name, "rnaseq-reviewer");
        assert_eq!(manifest.version, "1.2.0");
        assert_eq!(manifest.skill_type, "knowledge");
        let additions = manifest.prompt_additions.as_ref().unwrap();
        assert_eq!(additions.len(), 1);
        assert!(additions[0].contains("featureCounts -s"));
        assert!(!additions[0].contains("frontmatter"));
    }

    #[test]
    fn parse_skill_md_rejects_missing_required_fields() {
        assert!(parse_skill_md("---\nname: x\n---\nbody").is_none());
        assert!(parse_skill_md("---\ndescription: d\n---\nbody").is_none());
        assert!(parse_skill_md("no frontmatter here").is_none());
        assert!(parse_skill_md("---\nname: x\ndescription: d\n---\n").is_none());
    }

    #[test]
    fn discover_skills_from_finds_skill_md() {
        let home = tempfile::tempdir().unwrap();
        let skill_dir = home
            .path()
            .join(".oxo-flow")
            .join("skills")
            .join("qc-reviewer");
        std::fs::create_dir_all(&skill_dir).unwrap();
        std::fs::write(
            skill_dir.join("SKILL.md"),
            "---\nname: qc-reviewer\ndescription: Checks QC thresholds\n---\nPrefer fastp with phred 20.\n",
        )
        .unwrap();

        let found = discover_skills_from(Some(home.path().to_path_buf()), None);
        let names: Vec<&str> = found.iter().map(|s| s.name.as_str()).collect();
        assert!(names.contains(&"qc-reviewer"));
    }

    #[test]
    fn discover_skills_from_finds_user_and_project_level() {
        let home = tempfile::tempdir().unwrap();
        let project = tempfile::tempdir().unwrap();
        let write = |dir: &std::path::Path, base_name: &str| {
            let skills_dir = dir.join(".oxo-flow").join("skills");
            std::fs::create_dir_all(&skills_dir).unwrap();
            std::fs::write(
            skills_dir.join(format!("{base_name}.skill.toml")),
            format!(
                "name = \"{base_name}\"\nversion = \"1.0.0\"\ndescription = \"test skill\"\nskill_type = \"knowledge\"\n"
            ),
        )
        .unwrap();
        };
        write(home.path(), "qc-expert");
        write(project.path(), "somatic-expert");

        let found = discover_skills_from(Some(home.path().to_path_buf()), Some(project.path()));
        let names: Vec<&str> = found.iter().map(|s| s.name.as_str()).collect();
        assert!(names.contains(&"qc-expert"));
        assert!(names.contains(&"somatic-expert"));
    }

    #[test]
    fn discover_skills_ignores_invalid_manifests() {
        let home = tempfile::tempdir().unwrap();
        let skills_dir = home.path().join(".oxo-flow").join("skills");
        std::fs::create_dir_all(&skills_dir).unwrap();
        // Missing required fields — must be skipped, not fatal.
        std::fs::write(skills_dir.join("broken.skill.toml"), "name = 42\n").unwrap();

        let found = discover_skills_from(Some(home.path().to_path_buf()), None);
        assert!(found.is_empty());
    }
}
