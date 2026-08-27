//! Reference derivation helpers. (issue #206 extraction).
//! Workflow configuration and `.oxoflow` file parsing.
// Accesses deprecated `Rule::threads` / `Rule::memory` shorthand fields to
// apply defaults and expand rules.  Will be removed once the shorthand
// fields are retired.
#![allow(deprecated)]
//!
//! The `.oxoflow` format is TOML-based with workflow metadata, configuration
//! variables, default settings, and a list of rules.

use super::*;
use crate::error::Result;
use std::collections::HashMap;

/// Matches the namespaced `{values.name}` wildcard form.
///
/// The engine's placeholder regex (`\w+` only) cannot match dotted names, so
/// this namespace is detected and substituted textually — see
impl WorkflowConfig {
    /// Derive standard reference paths from `reference_dir`.
    ///
    /// Returns a map of derived paths for keys that are not explicitly set.
    pub fn derive_reference_paths(&self) -> HashMap<String, String> {
        // Support both top-level `reference_dir` and `[config]` reference_dir
        let base = self
            .reference_dir
            .as_deref()
            .or_else(|| self.config.get("reference_dir").and_then(|v| v.as_str()));
        let Some(base) = base else {
            return HashMap::new();
        };

        let derivations = [
            ("reference_fasta", "genome.fa"),
            ("gene_annotation", "genes.gtf"),
            ("bwa_index", "bwa/genome.fa"),
            ("bwamem2_index", "bwamem2/genome.fa"),
            ("bowtie2_index", "bowtie2/genome.fa"),
            ("star_index", "star"),
            ("hisat2_index", "hisat2/genome.fa"),
            ("minimap2_index", "genome.fa.mmi"),
            ("gatk_dict", "genome.dict"),
            ("samtools_faidx", "genome.fa.fai"),
        ];

        let mut result = HashMap::new();
        for (key, suffix) in derivations {
            // Only derive if not explicitly set
            if !self.config.contains_key(key) {
                result.insert(key.to_string(), format!("{}/{}", base, suffix));
            }
        }
        result
    }

    /// Merge derived reference paths into config, and auto-generate default
    /// `[[references]]` entries for standard indexes when `reference_dir` is set
    /// and no explicit references block exists.
    ///
    /// This connects the Reference Discovery API (`reference_dir`) with the
    /// auto-build system (`[[references]]`), so pipelines using only
    /// `reference_dir` get automatic index building without explicit declarations.
    #[must_use]
    pub fn with_derived_references(mut self) -> Self {
        let derived = self.derive_reference_paths();
        for (key, value) in &derived {
            self.config
                .entry(key.clone())
                .or_insert_with(|| toml::Value::String(value.clone()));
            self.injected_config_keys.insert(key.clone());
        }

        // If reference_dir is set but no [[references]] block exists, auto-derive
        // default index-building references so users don't need to declare them.
        if self.references.is_empty()
            && let Some(ref base) = self
                .reference_dir
                .as_deref()
                .or_else(|| self.config.get("reference_dir").and_then(|v| v.as_str()))
        {
            let defaults: Vec<ReferenceDef> = vec![
                // --- Universal: FASTA index (.fai) — required by virtually every tool ---
                ReferenceDef {
                    name: "samtools_faidx".into(),
                    source: Some(format!("{base}/genome.fa")),
                    output: format!("{base}/genome.fa.fai"),
                    build: format!("samtools faidx {base}/genome.fa"),
                    threads: Some(1),
                    memory: Some("2G".into()),
                    environment: None,
                    description: Some("FASTA index (.fai) — required by IGV, GATK, samtools, and most viewers".into()),
                },
                // --- Short-read DNA alignment: BWA (classic, widely used) ---
                ReferenceDef {
                    name: "bwa_index".into(),
                    source: Some(format!("{base}/genome.fa")),
                    output: format!("{base}/bwa/genome.fa.bwt"),
                    build: format!(
                        "mkdir -p {base}/bwa && bwa index -p {base}/bwa/genome.fa {base}/genome.fa"
                    ),
                    threads: Some(8),
                    memory: Some("8G".into()),
                    environment: None,
                    description: Some("BWA index for short-read DNA alignment (BWA-MEM/BWA-SW)".into()),
                },
                // --- Short-read DNA alignment: BWA-MEM2 (1.3-3.1x faster, identical output) ---
                ReferenceDef {
                    name: "bwamem2_index".into(),
                    source: Some(format!("{base}/genome.fa")),
                    output: format!("{base}/bwamem2/genome.fa.0123"),
                    build: format!(
                        "mkdir -p {base}/bwamem2 && bwa-mem2 index -p {base}/bwamem2/genome.fa {base}/genome.fa"
                    ),
                    threads: Some(8),
                    memory: Some("16G".into()),
                    environment: None,
                    description: Some("BWA-MEM2 index — faster BWA replacement, identical alignment output".into()),
                },
                // --- Short-read DNA alignment: Bowtie2 ---
                ReferenceDef {
                    name: "bowtie2_index".into(),
                    source: Some(format!("{base}/genome.fa")),
                    output: format!("{base}/bowtie2/genome.fa.1.bt2"),
                    build: format!(
                        "mkdir -p {base}/bowtie2 && bowtie2-build --threads 8 {base}/genome.fa {base}/bowtie2/genome.fa"
                    ),
                    threads: Some(8),
                    memory: Some("8G".into()),
                    environment: None,
                    description: Some("Bowtie2 index for short-read DNA alignment".into()),
                },
                // --- Long-read alignment: Minimap2 (Nanopore, PacBio) ---
                ReferenceDef {
                    name: "minimap2_index".into(),
                    source: Some(format!("{base}/genome.fa")),
                    output: format!("{base}/genome.fa.mmi"),
                    build: format!("minimap2 -d {base}/genome.fa.mmi {base}/genome.fa"),
                    threads: Some(4),
                    memory: Some("8G".into()),
                    environment: None,
                    description: Some("Minimap2 index (.mmi) for long-read alignment (Nanopore/PacBio)".into()),
                },
                // --- RNA-seq alignment: STAR (splice-aware, gold standard) ---
                ReferenceDef {
                    name: "star_index".into(),
                    source: Some(format!("{base}/genome.fa")),
                    output: format!("{base}/star/SAindex"),
                    build: format!(
                        "mkdir -p {base}/star && STAR --runMode genomeGenerate --genomeDir {base}/star --genomeFastaFiles {base}/genome.fa --sjdbGTFfile {base}/genes.gtf --runThreadN 16"
                    ),
                    threads: Some(16),
                    memory: Some("64G".into()),
                    environment: None,
                    description: Some("STAR index for splice-aware RNA-seq alignment (~30 GB, 2-6 hours)".into()),
                },
                // --- RNA-seq alignment: HISAT2 (hierarchical indexing, smaller memory) ---
                ReferenceDef {
                    name: "hisat2_index".into(),
                    source: Some(format!("{base}/genome.fa")),
                    output: format!("{base}/hisat2/genome.fa.1.ht2"),
                    build: format!(
                        "mkdir -p {base}/hisat2 && hisat2-build -p 8 {base}/genome.fa {base}/hisat2/genome.fa"
                    ),
                    threads: Some(8),
                    memory: Some("8G".into()),
                    environment: None,
                    description: Some("HISAT2 index for splice-aware RNA-seq alignment (hierarchical, smaller memory)".into()),
                },
                // --- Variant calling: Sequence dictionary (.dict) for GATK/Picard ---
                ReferenceDef {
                    name: "gatk_dict".into(),
                    source: Some(format!("{base}/genome.fa")),
                    output: format!("{base}/genome.dict"),
                    build: format!("samtools dict {base}/genome.fa -o {base}/genome.dict"),
                    threads: Some(1),
                    memory: Some("4G".into()),
                    environment: None,
                    description: Some("Sequence dictionary (.dict) for GATK/Picard variant calling".into()),
                },
            ];
            self.references = defaults;
        }
        self
    }

    /// Expand `[[references]]` builder templates and inject keyed config values.
    ///
    /// Every reference's `build` may name a built-in builder template
    /// (e.g. `build = "bwa_index"`) instead of a handwritten shell command;
    /// this step replaces the template name with its canonical command (see
    /// [`crate::references`] for the registry and the naming standard).
    /// Handwritten shell commands pass through unchanged, and unknown template
    /// names are rejected by [`Self::validate`].
    ///
    /// Each reference also becomes a keyed config value: `config.<name>` is
    /// set to the reference's `output` path (with `{config.x}` placeholders
    /// pre-expanded) unless the key is already declared, so rules reference
    /// the artifact as `{config.genome}`.
    #[must_use = "template expansion returns a Result that must be checked"]
    pub fn with_reference_builder_templates(mut self) -> Result<Self> {
        // Keyed references: config.<name> = output (unless already declared).
        // Iterate to a fixpoint so an output embedding another reference's
        // keyed config (`{config.other}`) resolves regardless of
        // declaration order; at most one expansion per reference per pass,
        // bounded by the reference count (an unresolvable `{config.x}` is
        // left literal and terminates the loop).
        for _ in 0..=self.references.len() {
            let mut changed = false;
            for def in &self.references {
                if def.output.trim().is_empty() {
                    continue;
                }
                // Fill missing keyed values, and re-expand previously
                // INJECTED values that still carry an unresolved
                // `{config.x}` (a reference whose output embeds another
                // reference's key). User-declared values are never touched.
                let needs_fill = !self.injected_config_keys.contains(&def.name)
                    && !self.config.contains_key(&def.name);
                let needs_reexpand = self.injected_config_keys.contains(&def.name)
                    && self
                        .config
                        .get(&def.name)
                        .and_then(toml::Value::as_str)
                        .is_some_and(|v| v.contains("{config."));
                if needs_fill || needs_reexpand {
                    let value = expand_config_vars_in_path(&def.output, &self.config);
                    changed |=
                        self.config.get(&def.name) != Some(&toml::Value::String(value.clone()));
                    self.config
                        .insert(def.name.clone(), toml::Value::String(value));
                    self.injected_config_keys.insert(def.name.clone());
                }
            }
            if !changed {
                break;
            }
        }
        // Builder templates: replace template names with canonical commands.
        for def in &mut self.references {
            def.build = crate::references::expand_build_command(def)?;
        }
        Ok(self)
    }

    /// Validate that a reference genome file path has a recognized extension
    /// (`.fa`, `.fasta`, `.fa.gz`, `.fasta.gz`) and optionally check that
    /// it exists on disk.
    #[must_use]
    pub fn validate_reference(path: &str) -> Vec<String> {
        let mut warnings = Vec::new();
        let valid_extensions = [".fa", ".fasta", ".fa.gz", ".fasta.gz"];
        let has_valid_ext = valid_extensions.iter().any(|ext| path.ends_with(ext));
        if !has_valid_ext {
            warnings.push(format!(
                "Reference path '{}' does not have a recognized extension (.fa, .fasta, .fa.gz, .fasta.gz)",
                path
            ));
        }
        // Check for .fai index
        let fai_path = format!("{}.fai", path);
        let p = std::path::Path::new(&fai_path);
        if !p.exists() && std::path::Path::new(path).exists() {
            warnings.push(format!(
                "Reference index '{}' not found; you may need to run 'samtools faidx'",
                fai_path
            ));
        }
        warnings
    }
}
