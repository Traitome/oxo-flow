//! TOML parsing, includes, profiles. (issue #206 extraction).
//! Workflow configuration and `.oxoflow` file parsing.
// Accesses deprecated `Rule::threads` / `Rule::memory` shorthand fields to
// apply defaults and expand rules.  Will be removed once the shorthand
// fields are retired.
#![allow(deprecated)]
//!
//! The `.oxoflow` format is TOML-based with workflow metadata, configuration
//! variables, default settings, and a list of rules.

use super::*;
use crate::error::{OxoFlowError, Result};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

/// Matches the namespaced `{values.name}` wildcard form.
///
/// The engine's placeholder regex (`\w+` only) cannot match dotted names, so
/// this namespace is detected and substituted textually — see
impl WorkflowConfig {
    /// Parse a workflow configuration from a TOML string.
    #[must_use = "parsing a config returns a Result that must be used"]
    pub fn parse(content: &str) -> Result<Self> {
        let mut config: WorkflowConfig = toml::from_str(content)?;
        config.extract_declarative_config()?;
        config = config.with_reference_builder_templates()?;
        config.validate()?;
        Ok(config)
    }

    /// Extract declarative `ConfigDef` entries from inline-table `[config]` values.
    fn extract_declarative_config(&mut self) -> Result<()> {
        for (key, val) in self.config.clone().iter() {
            let toml::Value::Table(t) = val else {
                continue;
            };
            if (t.contains_key("default")
                || t.contains_key("required")
                || t.contains_key("help")
                || t.contains_key("sensitive"))
                && let Ok(def) = toml::Value::Table(t.clone()).try_into::<ConfigDef>()
            {
                self.config_meta.insert(key.clone(), def);
                let runtime_val = self.config_meta[key].default.clone().unwrap_or_default();
                self.config
                    .insert(key.clone(), toml::Value::String(runtime_val));
            }
        }
        Ok(())
    }

    /// Parse a workflow configuration from a `.oxoflow` file.
    #[must_use = "parsing a config file returns a Result that must be used"]
    pub fn from_file(path: &Path) -> Result<Self> {
        let content = std::fs::read_to_string(path).map_err(|e| {
            if e.kind() == std::io::ErrorKind::NotFound {
                OxoFlowError::WorkflowNotFound(path.to_path_buf())
            } else {
                OxoFlowError::Parse {
                    path: path.to_path_buf(),
                    message: e.to_string(),
                }
            }
        })?;
        let mut config: WorkflowConfig =
            toml::from_str(&content).map_err(|e| OxoFlowError::Parse {
                path: path.to_path_buf(),
                message: e.to_string(),
            })?;

        config.extract_declarative_config()?;

        // input_groups patterns resolve against the workflow root — the
        // workflow file's parent, the same root sample_pattern scans.
        config.base_dir = Some(crate::parent_dir(path).to_path_buf());

        // Format Versioning check (S5)
        const SUPPORTED_FORMAT_VERSION: &str = "1.0";
        if let Some(ref version) = config.workflow.format_version
            && version != SUPPORTED_FORMAT_VERSION
        {
            tracing::warn!(
                "Workflow format version mismatch in {}: expected {}, found {}. Some features may not work as expected.",
                path.display(),
                SUPPORTED_FORMAT_VERSION,
                version
            );
        }

        // Resolve modular includes
        let parent = crate::parent_dir(path);
        config.resolve_includes(parent)?;

        // Load pairs from external file if specified
        if let Some(ref pairs_file) = config.workflow.pairs_file {
            let pairs_path = parent.join(pairs_file);
            let file_pairs = ExperimentControlPair::load_from_file(&pairs_path)?;
            let count = file_pairs.len();
            // Merge with inline pairs
            config.pairs.extend(file_pairs);
            tracing::info!("Loaded {} pairs from {}", count, pairs_file);
        }

        // Discover pairs from pattern if specified
        if let Some(ref pairs_pattern) = config.workflow.pairs_pattern {
            let discovered_pairs =
                ExperimentControlPair::discover_from_pattern(pairs_pattern, parent)?;
            let count = discovered_pairs.len();
            // Merge with inline/file pairs
            config.pairs.extend(discovered_pairs);
            tracing::info!(
                "Discovered {} pairs from pattern '{}'",
                count,
                pairs_pattern
            );
        }

        // Load sample_groups from external file if specified
        if let Some(ref groups_file) = config.workflow.sample_groups_file {
            let groups_path = parent.join(groups_file);
            let file_groups = SampleGroup::load_from_file(&groups_path)?;
            let count = file_groups.len();
            // Merge with inline groups
            config.sample_groups.extend(file_groups);
            tracing::info!("Loaded {} sample groups from {}", count, groups_file);
        }

        // Load the per-sample metadata table from external file if
        // specified (issue #227 item 2): the `{meta.<column>}` lookup
        // vocabulary, keyed by sample id.
        if let Some(ref metadata_file) = config.workflow.metadata_file {
            let meta_path = parent.join(metadata_file);
            let rows = SampleMetadata::load_from_file(&meta_path)?;
            let count = rows.len();
            for row in rows {
                config.metadata.insert(row.sample, row.values);
            }
            tracing::info!("Loaded {} metadata rows from {}", count, metadata_file);
        }

        // Auto-discover samples from filesystem pattern
        if let Some(ref sample_pattern) = config.workflow.sample_pattern {
            // Expand config variables in the pattern (e.g. {config.data_dir}/.../{sample}_R1.fq.gz)
            // Declared config defaults (from `key = { default = … }` entries) are
            // included alongside plain values so the pattern always resolves.
            let mut expand_config = config.config.clone();
            for (key, def) in &config.config_meta {
                if !expand_config.contains_key(key)
                    && let Some(ref default) = def.default
                {
                    expand_config.insert(key.clone(), toml::Value::String(default.clone()));
                }
            }
            let expanded_pattern = expand_config_vars_in_path(sample_pattern, &expand_config);

            // Validate pattern contains {sample} wildcard
            if !expanded_pattern.contains("{sample}") {
                return Err(OxoFlowError::Config {
                    message: format!(
                        "sample_pattern must contain {{sample}} wildcard after expansion: '{}'",
                        expanded_pattern
                    ),
                });
            }

            // Resolve the base directory and filename pattern.
            let sp = std::path::Path::new(&expanded_pattern);
            let (search_dir, file_pattern) = if sp.is_absolute() {
                if let Some(file_name) = sp.file_name() {
                    let dir = sp.parent().unwrap_or(std::path::Path::new("/"));
                    (dir.to_path_buf(), file_name.to_string_lossy().to_string())
                } else {
                    (parent.to_path_buf(), expanded_pattern)
                }
            } else if let Some(file_name) = sp.file_name() {
                let dir = sp.parent().unwrap_or(std::path::Path::new(""));
                (parent.join(dir), file_name.to_string_lossy().to_string())
            } else {
                (parent.to_path_buf(), expanded_pattern)
            };

            let discovered =
                crate::wildcard::discover_wildcards_from_pattern(&search_dir, &file_pattern)?;
            if discovered.is_empty() {
                tracing::warn!(
                    "sample_pattern '{}' matched no files in {}",
                    sample_pattern,
                    search_dir.display()
                );
            } else {
                // Extract sample values from discovered combinations
                let auto_samples: Vec<String> = discovered
                    .iter()
                    .filter_map(|combo| combo.get("sample").cloned())
                    .collect();

                if !auto_samples.is_empty() {
                    let auto_group = SampleGroup {
                        name: "auto-discovered".to_string(),
                        samples: auto_samples.clone(),
                        metadata: HashMap::new(),
                    };
                    config.sample_groups.push(auto_group);
                    tracing::info!(
                        "Auto-discovered {} samples from pattern '{}'",
                        auto_samples.len(),
                        sample_pattern
                    );
                } else {
                    tracing::warn!(
                        "sample_pattern '{}' matched files but no {{sample}} values were extracted",
                        sample_pattern
                    );
                }
            }
        }

        // ── Consolidate all sample sources into samples_list ──────────────
        // Merges auto-discovered samples (from sample_pattern), file-loaded
        // samples (from sample_groups_file), and inline [[sample_groups]].
        let mut all_samples: Vec<String> = Vec::new();
        for group in &config.sample_groups {
            for s in &group.samples {
                if !all_samples.contains(s) {
                    all_samples.push(s.clone());
                }
            }
        }
        // [[pairs]] members are samples too: experiment/control names are
        // what {sample}-wildcarded rules fan out over in pair workflows, so
        // the merged config.samples_list must include them (live: a
        // pairs-only workflow rendered {config.samples_list} as a literal
        // because nothing fed the merged list).
        for pair in &config.pairs {
            for s in std::iter::once(&pair.experiment).chain(pair.control.iter()) {
                if !all_samples.contains(s) {
                    all_samples.push(s.clone());
                }
            }
        }
        if !all_samples.is_empty() {
            let existing = config
                .config
                .get("samples_list")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            let merged = merge_comma_list(existing, &all_samples);
            config.config.insert(
                "samples_list".to_string(),
                toml::Value::String(merged.join(",")),
            );
        }
        // Also inject each sample group's name as a config variable
        for group in &config.sample_groups {
            config
                .config
                .entry(format!("samples_{}", group.name))
                .or_insert_with(|| toml::Value::String(group.samples.join(",")));
        }

        // ── Inject config.pairs_list (symmetric with samples_list) ────────
        // [[pairs]], pairs_file, and pairs_pattern entries are consolidated
        // above; their pair_ids are injected as a sorted, comma-joined list
        // so rules can reference `{config.pairs_list}` instead of keeping a
        // hand-written `[config] pair_ids = [...]` in sync with [[pairs]].
        if !config.pairs.is_empty() {
            let existing = config
                .config
                .get("pairs_list")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            let pair_ids: Vec<String> = config.pairs.iter().map(|p| p.pair_id.clone()).collect();
            let merged = merge_comma_list(existing, &pair_ids);
            config.config.insert(
                "pairs_list".to_string(),
                toml::Value::String(merged.join(",")),
            );
        }

        // Derive standard reference paths from reference_dir (e.g., reference_fasta = reference_dir + "/genome.fa")
        config = config.with_derived_references();

        // Expand [[references]] builder templates (build = "bwa_index" → canonical command)
        // and inject keyed config values (config.<name> = output) for each reference.
        config = config.with_reference_builder_templates()?;

        config.validate()?;
        Ok(config)
    }

    /// Resolve include directives by loading and merging rules from included files.
    /// Rules from included files are optionally prefixed with the namespace.
    #[must_use = "resolving includes returns a Result that must be checked"]
    pub fn resolve_includes(&mut self, base_dir: &Path) -> Result<()> {
        self.resolve_includes_with_depth(base_dir, 0)
    }

    fn resolve_includes_with_depth(&mut self, base_dir: &Path, depth: usize) -> Result<()> {
        if depth >= MAX_INCLUDE_DEPTH {
            return Err(OxoFlowError::Config {
                message: format!(
                    "include depth exceeds maximum of {} — possible circular includes",
                    MAX_INCLUDE_DEPTH
                ),
            });
        }
        let includes = std::mem::take(&mut self.includes);
        for inc in &includes {
            let (content, inc_base_dir) = if let Some(repo) = &inc.repo {
                // Pinned git include (issue #112): clone into the module
                // cache (keyed repo@ref), then resolve `path` inside the
                // checkout. ensure_pinned clones on a cache miss (healing
                // partial dirs), re-fetches branch/tag refs on every
                // activation, and pins full commit SHAs by fetch (issue
                // #136).
                let git_ref = inc.git_ref.as_deref().ok_or_else(|| OxoFlowError::Config {
                    message: format!(
                        "include '{}' declares `repo` without `ref` — pin the module version",
                        inc.path
                    ),
                })?;
                let cache_dir =
                    crate::git::module_cache_root().join(crate::git::cache_dir_name(repo, git_ref));
                std::fs::create_dir_all(cache_dir.parent().unwrap_or(&cache_dir)).map_err(|e| {
                    OxoFlowError::Config {
                        message: format!("cannot create module cache: {e}"),
                    }
                })?;
                crate::git::ensure_pinned(repo, git_ref, &cache_dir).map_err(|e| {
                    OxoFlowError::Config {
                        message: format!(
                            "failed to clone or refresh include repo '{repo}' at '{git_ref}': {e}"
                        ),
                    }
                })?;
                let inc_path = cache_dir.join(&inc.path);
                let text = std::fs::read_to_string(&inc_path).map_err(|e| OxoFlowError::Parse {
                    path: inc_path.clone(),
                    message: format!(
                        "failed to read include '{}' from repo '{repo}': {e}",
                        inc.path
                    ),
                })?;
                let next_base = inc_path.parent().unwrap_or(&cache_dir).to_path_buf();
                (text, next_base)
            } else if inc.path.starts_with("http://") || inc.path.starts_with("https://") {
                // Fetch remote include on a dedicated thread to avoid tokio runtime conflicts
                tracing::info!(url = %inc.path, "fetching remote include");
                let url = inc.path.clone();
                let text =
                    std::thread::spawn(move || reqwest::blocking::get(&url).and_then(|r| r.text()))
                        .join()
                        .map_err(|_| OxoFlowError::Parse {
                            path: PathBuf::from(&inc.path),
                            message: "remote include fetch panicked".to_string(),
                        })?
                        .map_err(|e| OxoFlowError::Parse {
                            path: PathBuf::from(&inc.path),
                            message: format!("failed to fetch remote include: {}", e),
                        })?;
                (text, base_dir.to_path_buf()) // Remote includes don't change base_dir for now
            } else {
                let inc_path = base_dir.join(&inc.path);
                let text = std::fs::read_to_string(&inc_path).map_err(|e| OxoFlowError::Parse {
                    path: inc_path.clone(),
                    message: format!("failed to read include '{}': {}", inc.path, e),
                })?;
                let next_base = inc_path.parent().unwrap_or(base_dir).to_path_buf();
                (text, next_base)
            };

            let mut inc_config: WorkflowConfig =
                toml::from_str(&content).map_err(|e| OxoFlowError::Parse {
                    path: PathBuf::from(&inc.path),
                    message: e.to_string(),
                })?;

            // Declarative `[config]` entries (`key = { default, ... }`)
            // are extracted exactly as they would be standalone, so their
            // defaults merge below as plain values (issue #142 M3).
            inc_config.extract_declarative_config()?;

            // Recursively resolve nested includes
            inc_config.resolve_includes_with_depth(&inc_base_dir, depth + 1)?;
            // Nested contracts merge first (their provenance indices shift
            // by the host's current contract count).
            let offset = self.include_contracts.len();
            self.include_contracts
                .extend(std::mem::take(&mut inc_config.include_contracts));
            for (rule, idx) in std::mem::take(&mut inc_config.module_of) {
                self.module_of.insert(rule, idx + offset);
            }
            // Record THIS include's contract (issue #112) and its rules'
            // provenance.
            let contract_idx = if !inc.inputs.is_empty() || !inc.outputs.is_empty() {
                self.include_contracts.push(ResolvedIncludeContract {
                    inputs: inc.inputs.clone(),
                    outputs: inc.outputs.clone(),
                });
                Some(self.include_contracts.len() - 1)
            } else {
                None
            };
            // Collect original rule names from included file for dependency resolution
            // Module identity: explicit `name` field, else the file stem.
            let module_name = inc.name.clone().unwrap_or_else(|| {
                Path::new(&inc.path)
                    .file_stem()
                    .map(|s| s.to_string_lossy().to_string())
                    .unwrap_or_else(|| inc.path.clone())
            });
            let original_rule_names: std::collections::HashSet<String> =
                inc_config.rules.iter().map(|r| r.name.clone()).collect();
            let mut module_members: Vec<String> = Vec::new();
            for mut rule in inc_config.rules {
                if let Some(ref ns) = inc.namespace {
                    // Prefix rule name with namespace
                    rule.name = format!("{}::{}", ns, rule.name);
                    // Prefix depends_on references that point to rules in the same included file
                    for dep in &mut rule.depends_on {
                        if original_rule_names.contains(dep) {
                            *dep = format!("{}::{}", ns, dep);
                        }
                    }
                }
                if !self.rules.iter().any(|r| r.name == rule.name) {
                    if let Some(idx) = contract_idx {
                        self.module_of.insert(rule.name.clone(), idx);
                    }
                    module_members.push(rule.name.clone());
                    self.rules.push(rule);
                }
            }
            self.module_rules
                .entry(module_name)
                .or_default()
                .extend(module_members);
            // Contract params fill config keys in profile-style — host
            // values win (or_insert). The module's own `[config]`
            // defaults are merged after, so they fill only the gaps
            // params left open (issue #142 M3): a module declaring
            // `[config] trim_quality = "20"` keeps that default when
            // included without host params, while any host value — its
            // own `[config]` table or `[[include]] params` — wins.
            for (key, value) in &inc.params {
                self.config
                    .entry(key.clone())
                    .or_insert_with(|| value.clone());
            }
            for (key, value) in &inc_config.config {
                self.config
                    .entry(key.clone())
                    .or_insert_with(|| value.clone());
            }
            for (key, def) in &inc_config.config_meta {
                self.config_meta
                    .entry(key.clone())
                    .or_insert_with(|| def.clone());
            }
        }
        self.includes = includes;
        Ok(())
    }

    /// The rule-name closure of a module for partial runs (issue #112
    /// elasticity): the module's own rules plus every HOST rule producing
    /// one of its declared concrete inputs. Upstream DAG dependents are
    /// added by the caller through the regular target machinery.
    pub fn module_closure(&self, module: &str) -> Option<Vec<String>> {
        let members = self.module_rules.get(module)?.clone();
        let mut closure: Vec<String> = members.clone();
        // Declared concrete inputs the module needs wired in.
        for (idx, contract) in self.include_contracts.iter().enumerate() {
            let module_uses_this_contract = members
                .iter()
                .any(|r| self.module_of.get(r).is_some_and(|p| *p == idx));
            if !module_uses_this_contract {
                continue;
            }
            let mut producers: HashMap<String, String> = HashMap::new();
            for rule in &self.rules {
                for out in rule.output.to_vec() {
                    producers.entry(out).or_insert_with(|| rule.name.clone());
                }
            }
            for input in &contract.inputs {
                if input.contains('{') {
                    continue;
                }
                if let Some(producer) = producers.get(input)
                    && !members.contains(producer)
                    && !closure.contains(producer)
                {
                    closure.push(producer.clone());
                }
            }
        }
        Some(closure)
    }

    /// Check the resolved include contracts (issue #112 module slice).
    ///
    /// Returns `(errors, warnings)`. Errors cover the fail-fast contract:
    /// a declared concrete input nobody produces, and a declared output no
    /// module rule produces (or produced outside the module). Warnings
    /// cover encapsulation: a host rule reading a module-internal file the
    /// contract does not declare. Wildcarded patterns are checked
    /// structurally — an engine-wildcarded input (`{sample}`, `{config.x}`)
    /// forms no DAG edge (placeholder values only exist at run time), so a
    /// wildcarded host input that could address a module-internal output
    /// pattern (identical literal prefix and suffix) warns when the
    /// contract declares no overlapping pattern; patterns that resolve to
    /// different literals are not statically checkable.
    pub fn check_include_contracts(&self) -> (Vec<String>, Vec<String>) {
        let mut errors = Vec::new();
        let mut warnings = Vec::new();
        if self.include_contracts.is_empty() {
            return (errors, warnings);
        }
        // Producer map: output path → producing rule name (owned — rule
        // outputs are borrowed across iterations).
        let mut producers: HashMap<String, String> = HashMap::new();
        for rule in &self.rules {
            for out in rule.output.to_vec() {
                producers.entry(out).or_insert_with(|| rule.name.clone());
            }
        }
        for (idx, contract) in self.include_contracts.iter().enumerate() {
            for input in &contract.inputs {
                if input.contains('{') {
                    continue; // wildcard wiring: DAG-time, documented
                }
                if !producers.contains_key(input) {
                    errors.push(format!(
                        "include contract: declared input '{input}' is not produced by any rule — wire it to a host rule output"
                    ));
                }
            }
            for output in &contract.outputs {
                if output.contains('{') {
                    continue;
                }
                match producers.get(output) {
                    Some(producer)
                        if self
                            .module_of
                            .get(producer)
                            .is_some_and(|p| *p == idx) => {}
                    Some(producer) => errors.push(format!(
                        "include contract: declared output '{output}' is produced by '{producer}', which is outside the module — module outputs must come from module rules"
                    )),
                    None => errors.push(format!(
                        "include contract: declared output '{output}' is not produced by any module rule"
                    )),
                }
            }
            // Encapsulation: host rules reading undeclared module-internal
            // files.
            let declared: std::collections::HashSet<&str> =
                contract.outputs.iter().map(String::as_str).collect();
            for rule in &self.rules {
                if self.module_of.get(&rule.name).is_some_and(|p| *p == idx) {
                    continue;
                }
                for inp in rule.input.to_vec() {
                    if inp.contains('{') {
                        // Wildcarded inputs cannot be resolved through the
                        // exact-string producer map, and they form no DAG
                        // edge (placeholder values only exist at run time),
                        // so fall back to a structural pattern match: warn
                        // when the pattern could address a module-internal
                        // output and the contract declares no overlapping
                        // pattern.
                        let internal = self.rules.iter().any(|r| {
                            self.module_of.get(&r.name).is_some_and(|p| *p == idx)
                                && r.output
                                    .iter()
                                    .any(|o| Self::wildcarded_patterns_overlap(&inp, o))
                        });
                        let declared_overlap = declared
                            .iter()
                            .any(|d| Self::wildcarded_patterns_overlap(&inp, d));
                        if internal && !declared_overlap {
                            warnings.push(format!(
                                "rule '{}' reads module-internal file pattern '{inp}' which the include contract does not declare — add it to `outputs` to keep the coupling explicit",
                                rule.name
                            ));
                        }
                        continue;
                    }
                    let internal = producers
                        .get(&inp)
                        .is_some_and(|p| self.module_of.get(p).is_some_and(|mp| *mp == idx));
                    if internal && !declared.contains(inp.as_str()) {
                        warnings.push(format!(
                            "rule '{}' reads module-internal file '{inp}' which the include contract does not declare — add it to `outputs` to keep the coupling explicit",
                            rule.name
                        ));
                    }
                }
            }
        }
        (errors, warnings)
    }

    /// Split a wildcarded path into its literal prefix (up to the first
    /// `{`) and literal suffix (after the last `}`). A path without
    /// wildcards yields `(path, "")`.
    fn wildcard_literals(pattern: &str) -> (&str, &str) {
        match (pattern.find('{'), pattern.rfind('}')) {
            (Some(open), Some(close)) if close > open => (&pattern[..open], &pattern[close + 1..]),
            _ => (pattern, ""),
        }
    }

    /// Structural overlap of a wildcarded `pattern` with another path.
    ///
    /// The other side may itself be wildcarded (then both literal prefixes
    /// and both literal suffixes must match) or concrete (then the pattern
    /// must bound it on both sides). `qc/{sample}.html` overlaps
    /// `qc/{sample}.html` but not `qc/{sample}.bcf`; `qc/{sample}.html`
    /// overlaps the concrete `qc/sample1.html`. Deliberately conservative:
    /// patterns whose wildcard sits in different places never overlap.
    fn wildcarded_patterns_overlap(pattern: &str, other: &str) -> bool {
        debug_assert!(pattern.contains('{'));
        let (pre, suf) = Self::wildcard_literals(pattern);
        if other.contains('{') {
            let (o_pre, o_suf) = Self::wildcard_literals(other);
            pre == o_pre && suf == o_suf
        } else {
            other.starts_with(pre) && other.ends_with(suf) && other.len() >= pre.len() + suf.len()
        }
    }

    /// Validate that all execution group references point to existing rules.
    #[must_use = "validation returns a Result that must be checked"]
    pub fn validate_execution_groups(&self) -> Result<()> {
        let rule_names: std::collections::HashSet<&str> =
            self.rules.iter().map(|r| r.name.as_str()).collect();
        for group in &self.execution_groups {
            for rule_ref in &group.rules {
                if !rule_names.contains(rule_ref.as_str()) {
                    return Err(OxoFlowError::Config {
                        message: format!(
                            "execution group '{}' references unknown rule '{}'",
                            group.name, rule_ref
                        ),
                    });
                }
            }
        }
        Ok(())
    }

    /// Apply global defaults to all rules that don't have explicit overrides.
    pub fn apply_defaults(&mut self) {
        for rule in &mut self.rules {
            // Apply default threads if rule doesn't specify one (either field).
            // resources.threads defaults to 1 via serde, and 1 means "unset"
            // in the engine's own convention (skip_serializing_if), so values
            // > 1 are treated as explicit rule-level declarations.
            if rule.threads.is_none() && rule.resources.threads <= 1 {
                rule.threads = self.defaults.threads;
            }
            // Apply default memory if rule doesn't specify one (either field)
            if rule.memory.is_none()
                && rule.resources.memory.is_none()
                && let Some(ref mem) = self.defaults.memory
            {
                rule.memory = Some(mem.clone());
            }
            // Apply default environment if rule doesn't specify one
            if rule.environment.is_empty()
                && let Some(ref env) = self.defaults.environment
            {
                rule.environment = env.clone();
            }
        }
    }

    /// Merge a `profiles/<NAME>.toml` document into this workflow — the
    /// semantics `run --profile` / `dry-run --profile` apply.
    ///
    /// Merges the profile's `[config]` (every key becomes a `{config.key}`
    /// variable) and `[defaults]` (feeds `rules.resources` defaults via
    /// [`WorkflowConfig::apply_defaults`]) sections. The mode comes from
    /// `[workflow] profile_mode`:
    ///
    /// - `"fill"` (default): profile values only FILL IN keys the workflow
    ///   does not set — existing workflow values always win.
    /// - `"override"`: profile values REPLACE workflow values. Nested
    ///   tables deep-merge recursively (keys from both sides survive);
    ///   scalars and arrays replace the workflow value wholesale.
    ///
    /// Callers must pass a profile document parsed with `toml::from_str`
    /// (a document), not `toml::Value::from_str` — in toml 1.x the latter
    /// parses a SINGLE inline value and would reject `[config]` tables.
    pub fn merge_profile(&mut self, profile_toml: &toml::Value) -> Result<()> {
        fn coerce_profile_defaults(table: toml::Table) -> toml::Table {
            // Profiles historically tolerated quoted numerics in [defaults]
            // (e.g. `threads = "16"`). Keep that tolerance: coerce string
            // values for integer fields, let genuinely wrong types fail the
            // typed conversion below with the same clear error.
            let mut table = table;
            if let Some(toml::Value::String(s)) = table.get("threads")
                && let Ok(n) = s.trim().parse::<u32>()
            {
                table.insert("threads".into(), toml::Value::Integer(i64::from(n)));
            }
            table
        }

        let mode = self.workflow.profile_mode.unwrap_or_default();
        if let Some(config_table) = profile_toml.get("config").and_then(toml::Value::as_table) {
            for (key, value) in config_table {
                match mode {
                    ProfileMode::Fill => {
                        self.config
                            .entry(key.clone())
                            .or_insert_with(|| value.clone());
                    }
                    ProfileMode::Override => match self.config.get_mut(key) {
                        Some(existing) => deep_merge_value(existing, value.clone()),
                        None => {
                            self.config.insert(key.clone(), value.clone());
                        }
                    },
                }
            }
        }
        if let Some(defaults_table) = profile_toml.get("defaults").and_then(toml::Value::as_table) {
            let profile_defaults: Defaults =
                toml::Value::Table(coerce_profile_defaults(defaults_table.clone()))
                    .try_into()
                    .map_err(|e| OxoFlowError::Config {
                        message: format!("invalid [defaults] section in profile: {e}"),
                    })?;
            match mode {
                ProfileMode::Fill => {
                    if self.defaults.threads.is_none() {
                        self.defaults.threads = profile_defaults.threads;
                    }
                    if self.defaults.memory.is_none() {
                        self.defaults.memory = profile_defaults.memory;
                    }
                    if self.defaults.environment.is_none() {
                        self.defaults.environment = profile_defaults.environment;
                    }
                }
                ProfileMode::Override => {
                    if profile_defaults.threads.is_some() {
                        self.defaults.threads = profile_defaults.threads;
                    }
                    if profile_defaults.memory.is_some() {
                        self.defaults.memory = profile_defaults.memory;
                    }
                    if profile_defaults.environment.is_some() {
                        self.defaults.environment = profile_defaults.environment;
                    }
                }
            }
        }
        // `[cluster]` — the block whose presence routes `run --profile` to a
        // scheduler (issue #74). Same fill/override semantics as the rest of
        // the merge, so site config lives in one shared profile file.
        if let Some(cluster_table) = profile_toml.get("cluster") {
            let profile_cluster: ClusterProfile =
                cluster_table
                    .clone()
                    .try_into()
                    .map_err(|e| OxoFlowError::Config {
                        message: format!("invalid [cluster] section in profile: {e}"),
                    })?;
            self.cluster
                .get_or_insert_with(ClusterProfile::default)
                .merge_from(&profile_cluster, mode == ProfileMode::Override);
        }
        Ok(())
    }
}
pub fn resolve_rule_templates(rules: &mut [crate::rule::Rule]) -> crate::Result<()> {
    // Build a name→index map
    let name_to_idx: std::collections::HashMap<String, usize> = rules
        .iter()
        .enumerate()
        .map(|(i, r)| (r.name.clone(), i))
        .collect();

    // Detect circular inheritance
    for rule in rules.iter() {
        if let Some(ref base_name) = rule.extends {
            let mut visited = std::collections::HashSet::new();
            visited.insert(rule.name.clone());
            let mut current = base_name.clone();
            while let Some(&idx) = name_to_idx.get(&current) {
                if !visited.insert(current.clone()) {
                    return Err(crate::OxoFlowError::Config {
                        message: format!(
                            "circular extends chain detected: rule '{}' extends '{}' which forms a cycle",
                            rule.name, base_name
                        ),
                    });
                }
                match &rules[idx].extends {
                    Some(next) => current = next.clone(),
                    None => break,
                }
            }
        }
    }

    // Resolve templates (iterate by index to avoid borrow issues)
    let snapshot: Vec<crate::rule::Rule> = rules.to_vec();

    for rule in rules.iter_mut() {
        if let Some(ref base_name) = rule.extends.clone() {
            let base_idx =
                name_to_idx
                    .get(base_name)
                    .ok_or_else(|| crate::OxoFlowError::Config {
                        message: format!(
                            "rule '{}' extends '{}' which does not exist",
                            rule.name, base_name
                        ),
                    })?;
            let base = &snapshot[*base_idx];

            // Inherit fields that are at their default values
            if rule.threads.is_none() && base.threads.is_some() {
                rule.threads = base.threads;
            }
            if rule.memory.is_none() && base.memory.is_some() {
                rule.memory = base.memory.clone();
            }
            if rule.resources == crate::rule::Resources::default()
                && base.resources != crate::rule::Resources::default()
            {
                rule.resources = base.resources.clone();
            }
            if rule.environment.is_empty() && !base.environment.is_empty() {
                rule.environment = base.environment.clone();
            }
            if rule.tags.is_empty() && !base.tags.is_empty() {
                rule.tags = base.tags.clone();
            }
            if rule.retries == 0 && base.retries > 0 {
                rule.retries = base.retries;
            }
            if rule.retry_delay.is_none() && base.retry_delay.is_some() {
                rule.retry_delay = base.retry_delay.clone();
            }
            if rule.group.is_none() && base.group.is_some() {
                rule.group = base.group.clone();
            }
            if rule.log.is_none() && base.log.is_some() {
                rule.log = base.log.clone();
            }
            // Inherit params that are not already set
            for (key, value) in &base.params {
                let k: String = key.clone();
                let v: toml::Value = value.clone();
                rule.params.entry(k).or_insert(v);
            }
        }
    }

    Ok(())
}

/// Merge a comma-joined config value with newly consolidated entries.
///
/// Existing entries keep their order and never duplicate; the combined
/// list is then sorted and deduplicated. Shared by the engine-injected
/// `config.samples_list` and `config.pairs_list` consolidation.
fn merge_comma_list(existing: &str, new: &[String]) -> Vec<String> {
    let existing_set: std::collections::HashSet<&str> =
        existing.split(',').filter(|s| !s.is_empty()).collect();
    let mut merged: Vec<String> = existing
        .split(',')
        .filter(|s| !s.is_empty())
        .map(|s| s.to_string())
        .collect();
    for s in new {
        if !existing_set.contains(s.as_str()) {
            merged.push(s.clone());
        }
    }
    merged.sort();
    merged.dedup();
    merged
}

/// Deep-merge `src` into `dst` in place (profile override semantics):
/// nested tables recurse — keys from both sides survive, `src` wins on
/// conflict — while scalars and arrays replace `dst` wholesale.
fn deep_merge_value(dst: &mut toml::Value, src: toml::Value) {
    match (dst, src) {
        (toml::Value::Table(dst_table), toml::Value::Table(src_table)) => {
            for (key, src_value) in src_table {
                match dst_table.get_mut(&key) {
                    Some(dst_value) => deep_merge_value(dst_value, src_value),
                    None => {
                        dst_table.insert(key, src_value);
                    }
                }
            }
        }
        (dst_value, src_value) => *dst_value = src_value,
    }
}
