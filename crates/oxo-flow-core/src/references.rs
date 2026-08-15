//! Built-in builder templates for `[[references]]`.
//!
//! A reference entry's `build` field accepts either a handwritten shell
//! command or the *name* of a built-in builder template:
//!
//! ```toml
//! [[references]]
//! name = "genome"
//! source = "refs/genome.fa"
//! output = "refs/genome.fa.fai"
//! build = "samtools_faidx"   # instead of "samtools faidx refs/genome.fa"
//! threads = 2
//! ```
//!
//! A `build` value that is a single bare identifier (ASCII letters, digits,
//! `_` and `-` only — no spaces, slashes or shell syntax) is treated as a
//! template name. Anything else — any real shell command contains whitespace
//! or shell metacharacters — is treated as a handwritten command and passed
//! through unchanged. Unknown template names are rejected during validation
//! (`validate` reports an error) rather than silently running as a command.
//!
//! # Naming standard
//!
//! - Name the primary reference `genome` (or `transcriptome` for the
//!   RNA-seq FASTA/annotation source). Derived indexes get descriptive
//!   names built from it: `genome_faidx`, `genome_bwa_index`,
//!   `genome_star_index`, `genome_dict`.
//! - Keyed references: every reference's `name` is injected into `[config]`
//!   as `config.<name> = <output>` (unless the key is already declared), so
//!   rules reference the artifact as `{config.genome}` and never duplicate
//!   the path.
//! - `output` must name the path the build command actually creates — the
//!   engine skips the build when that path exists and rebuilds when missing.
//!
//! # Templates
//!
//! Each template expands to a canonical command at parse time using the
//! placeholders:
//!
//! - `{input}` — the reference's `source` path (all templates require it)
//! - `{output}` — the reference's `output`, left for the render pipeline
//!   (which expands `{config.x}` inside it and resolves relative paths)
//! - `{threads}` — the reference's `threads` field, defaulting to `1`
//! - `{prefix}` — the BWA index prefix derived from `output` (see
//!   [`bwa_index_prefix`])

use crate::Result;
use crate::config::ReferenceDef;
use crate::error::OxoFlowError;

/// A canonical build recipe that a `[[references]]` entry can name via
/// `build = "<name>"` instead of a handwritten shell command.
pub struct ReferenceTemplate {
    /// Registry key — the value usable as `build = "<name>"`.
    pub name: &'static str,
    /// One-line description of what the template builds.
    pub description: &'static str,
    /// Shell template with `{input}` / `{output}` / `{threads}` placeholders.
    /// `{input}` and `{threads}` are filled at expansion time from the
    /// reference's `source` and `threads` fields; `{output}` (and any
    /// `{config.x}` inside it) is expanded later by the render pipeline.
    pub command: &'static str,
}

/// The built-in builder template registry, in canonical order.
pub fn templates() -> &'static [ReferenceTemplate] {
    &TEMPLATES
}

const TEMPLATES: [ReferenceTemplate; 5] = [
    ReferenceTemplate {
        name: "samtools_faidx",
        description: "FASTA index (.fai) via samtools — required by IGV, GATK and most viewers",
        command: "mkdir -p \"$(dirname {output})\" && samtools faidx {input}",
    },
    ReferenceTemplate {
        name: "picard_dict",
        description: "Sequence dictionary (.dict) via Picard CreateSequenceDictionary for GATK/Picard",
        command: "mkdir -p \"$(dirname {output})\" && picard CreateSequenceDictionary R={input} O={output}",
    },
    ReferenceTemplate {
        name: "bwa_index",
        description: "BWA index (five files .amb/.ann/.bwt/.pac/.sa) for BWA-MEM/BWA-SW short-read alignment",
        command: "mkdir -p \"$(dirname {output})\" && bwa index -p {prefix} {input}",
    },
    ReferenceTemplate {
        name: "star_index",
        description: "STAR genomeGenerate index — output is the --genomeDir directory; add --sjdbGTFfile via a handwritten build when splice annotation is needed",
        command: "mkdir -p {output} && STAR --runMode genomeGenerate --genomeDir {output} --genomeFastaFiles {input} --runThreadN {threads}",
    },
    ReferenceTemplate {
        name: "bismark_index",
        description: "Bismark bisulfite genome preparation — input is the DIRECTORY of FASTA files; output is <input>/Bisulfite_Genome",
        command: "mkdir -p \"$(dirname {output})\" && bismark_genome_preparation --parallel {threads} --verbose {input}",
    },
];

/// Look up a builder template by name.
pub fn template(name: &str) -> Option<&'static ReferenceTemplate> {
    TEMPLATES.iter().find(|t| t.name == name)
}

/// All template names, comma-joined — used in validation error messages.
pub fn template_names() -> String {
    TEMPLATES
        .iter()
        .map(|t| t.name)
        .collect::<Vec<_>>()
        .join(", ")
}

/// True when `build` is a single bare identifier — a builder template name
/// under the `[[references]]` contract. Any real shell command contains
/// whitespace or shell metacharacters and is never treated as a template.
pub fn is_template_name(build: &str) -> bool {
    !build.is_empty()
        && build
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
}

/// Expand `def.build` when it names a builder template, returning the
/// canonical command with `{input}` / `{threads}` / `{prefix}` filled in.
///
/// Handwritten shell commands (and bare identifiers that do not match any
/// template — those are rejected by [`validate_reference_defs`]) pass through
/// unchanged.
pub fn expand_build_command(def: &ReferenceDef) -> Result<String> {
    if !is_template_name(&def.build) {
        return Ok(def.build.clone());
    }
    let Some(tpl) = template(&def.build) else {
        // Unknown bare identifier: validation reports it; keep the value
        // intact here so the error names the field as written.
        return Ok(def.build.clone());
    };
    expand_template(tpl, def)
}

fn expand_template(tpl: &ReferenceTemplate, def: &ReferenceDef) -> Result<String> {
    let input = def.source.as_deref().ok_or_else(|| OxoFlowError::Config {
        message: format!(
            "reference '{}': builder template '{}' requires a 'source' \
             (the FASTA file or directory the index is built from)",
            def.name, tpl.name
        ),
    })?;
    if def.output.trim().is_empty() {
        return Err(OxoFlowError::Config {
            message: format!(
                "reference '{}': builder template '{}' requires an 'output' path",
                def.name, tpl.name
            ),
        });
    }
    let threads = def.threads.unwrap_or(1).to_string();
    let prefix = bwa_index_prefix(&def.output);
    Ok(tpl
        .command
        .replace("{input}", input)
        .replace("{threads}", &threads)
        .replace("{prefix}", &prefix))
}

/// The five files `bwa index -p <prefix>` writes.
const BWA_INDEX_SUFFIXES: [&str; 5] = [".amb", ".ann", ".bwt", ".pac", ".sa"];

/// Derive the shared `-p` prefix of a BWA index from the declared output
/// path. `bwa index` writes five files named `<prefix>.{amb,ann,bwt,pac,sa}`,
/// so an output naming any one of them (convention: the `.bwt`) maps back to
/// the prefix; any other output is treated as the prefix itself.
pub fn bwa_index_prefix(output: &str) -> String {
    BWA_INDEX_SUFFIXES
        .iter()
        .find_map(|suffix| output.strip_suffix(suffix).map(str::to_string))
        .unwrap_or_else(|| output.to_string())
}

/// Validate the declared `[[references]]` entries:
///
/// - `build` naming an unknown builder template is an error (a bare
///   identifier is a template reference, not a shell command)
/// - a template build without an `output` path is an error
/// - duplicate reference names are an error (checkpoint tracking keys on
///   `name`)
pub fn validate_reference_defs(defs: &[ReferenceDef]) -> Result<()> {
    let mut seen = std::collections::HashSet::new();
    for def in defs {
        if is_template_name(&def.build) && template(&def.build).is_none() {
            return Err(OxoFlowError::Config {
                message: format!(
                    "reference '{}': build '{}' is not a known builder template \
                     (use a handwritten shell command, or one of: {})",
                    def.name,
                    def.build,
                    template_names()
                ),
            });
        }
        if is_template_name(&def.build) && def.output.trim().is_empty() {
            return Err(OxoFlowError::Config {
                message: format!(
                    "reference '{}': builder template '{}' requires an 'output' path",
                    def.name, def.build
                ),
            });
        }
        if !seen.insert(def.name.as_str()) {
            return Err(OxoFlowError::Config {
                message: format!(
                    "duplicate reference name '{}' — reference names must be unique \
                     (checkpoint tracking and config.<name> injection key on them)",
                    def.name
                ),
            });
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::WorkflowConfig;
    use std::collections::HashMap;

    fn ref_def(build: &str) -> ReferenceDef {
        ReferenceDef {
            name: "genome".into(),
            source: Some("refs/genome.fa".into()),
            output: "refs/genome.fa.fai".into(),
            build: build.into(),
            threads: None,
            memory: None,
            description: None,
        }
    }

    // ── registry ──────────────────────────────────────────────────────────

    #[test]
    fn registry_lists_five_documented_templates() {
        let names: Vec<&str> = templates().iter().map(|t| t.name).collect();
        assert_eq!(
            names,
            [
                "samtools_faidx",
                "picard_dict",
                "bwa_index",
                "star_index",
                "bismark_index"
            ]
        );
        for tpl in templates() {
            assert!(
                tpl.command.contains("{input}"),
                "{} must use {{input}}",
                tpl.name
            );
            assert!(
                tpl.command.contains("{output}"),
                "{} must use {{output}}",
                tpl.name
            );
            assert!(!tpl.description.is_empty());
        }
    }

    // ── template-name detection ───────────────────────────────────────────

    #[test]
    fn is_template_name_accepts_identifiers() {
        assert!(is_template_name("bwa_index"));
        assert!(is_template_name("samtools_faidx"));
        assert!(is_template_name("genome_bwa-index"));
    }

    #[test]
    fn is_template_name_rejects_shell_syntax() {
        assert!(!is_template_name("samtools faidx x.fa"));
        assert!(!is_template_name("mkdir -p x && bwa index x"));
        assert!(!is_template_name("refs/genome.fa"));
        assert!(!is_template_name("$CMD"));
        assert!(!is_template_name("{output}"));
        assert!(!is_template_name(""));
        assert!(!is_template_name("a;b"));
    }

    // ── expansion ─────────────────────────────────────────────────────────

    #[test]
    fn bwa_index_template_expands_to_canonical_command() {
        let mut def = ref_def("bwa_index");
        def.output = "refs/genome.fa.bwt".into();
        def.threads = Some(8);
        // `{output}` is left for the render pipeline (it expands {config.x}
        // inside the path); input and prefix are filled at expansion time.
        assert_eq!(
            expand_build_command(&def).unwrap(),
            "mkdir -p \"$(dirname {output})\" && bwa index -p refs/genome.fa refs/genome.fa"
        );
    }

    #[test]
    fn samtools_faidx_template_expands() {
        assert_eq!(
            expand_build_command(&ref_def("samtools_faidx")).unwrap(),
            "mkdir -p \"$(dirname {output})\" && samtools faidx refs/genome.fa"
        );
    }

    #[test]
    fn picard_dict_template_expands() {
        let mut def = ref_def("picard_dict");
        def.output = "refs/genome.dict".into();
        assert_eq!(
            expand_build_command(&def).unwrap(),
            "mkdir -p \"$(dirname {output})\" && picard CreateSequenceDictionary R=refs/genome.fa O={output}"
        );
    }

    #[test]
    fn star_index_template_expands_with_threads() {
        let mut def = ref_def("star_index");
        def.output = "refs/star".into();
        def.threads = Some(16);
        assert_eq!(
            expand_build_command(&def).unwrap(),
            "mkdir -p {output} && STAR --runMode genomeGenerate --genomeDir {output} \
             --genomeFastaFiles refs/genome.fa --runThreadN 16"
        );
    }

    #[test]
    fn bismark_index_template_expands_with_directory_input() {
        let mut def = ref_def("bismark_index");
        def.source = Some("refs/bisulfite".into());
        def.output = "refs/bisulfite/Bisulfite_Genome".into();
        def.threads = Some(4);
        assert_eq!(
            expand_build_command(&def).unwrap(),
            "mkdir -p \"$(dirname {output})\" && \
             bismark_genome_preparation --parallel 4 --verbose refs/bisulfite"
        );
    }

    #[test]
    fn template_threads_default_to_one() {
        let mut def = ref_def("star_index");
        def.output = "refs/star".into();
        assert!(
            expand_build_command(&def)
                .unwrap()
                .ends_with("--runThreadN 1"),
            "unset threads must render as 1"
        );
    }

    #[test]
    fn handwritten_shell_passes_through_unchanged() {
        let cmd = "mkdir -p refs && echo built > {output}";
        let def = ref_def(cmd);
        assert_eq!(expand_build_command(&def).unwrap(), cmd);
    }

    #[test]
    fn unknown_bare_identifier_passes_through_expansion() {
        // Expansion only handles known templates; validation rejects the rest.
        let def = ref_def("bogus_index");
        assert_eq!(expand_build_command(&def).unwrap(), "bogus_index");
    }

    #[test]
    fn template_without_source_errors() {
        let mut def = ref_def("bwa_index");
        def.source = None;
        let err = expand_build_command(&def).unwrap_err();
        assert!(err.to_string().contains("requires a 'source'"), "{err}");
    }

    #[test]
    fn template_without_output_errors() {
        let mut def = ref_def("bwa_index");
        def.output = String::new();
        let err = expand_build_command(&def).unwrap_err();
        assert!(err.to_string().contains("requires an 'output'"), "{err}");
    }

    // ── bwa prefix derivation ─────────────────────────────────────────────

    #[test]
    fn bwa_prefix_strips_any_of_the_five_suffixes() {
        assert_eq!(bwa_index_prefix("refs/genome.fa.bwt"), "refs/genome.fa");
        assert_eq!(bwa_index_prefix("refs/genome.fa.pac"), "refs/genome.fa");
        assert_eq!(bwa_index_prefix("refs/genome.fa.sa"), "refs/genome.fa");
    }

    #[test]
    fn bwa_prefix_unknown_output_is_used_as_is() {
        assert_eq!(bwa_index_prefix("refs/genome.fa"), "refs/genome.fa");
        assert_eq!(bwa_index_prefix("refs/genome.fa.bwa"), "refs/genome.fa.bwa");
    }

    // ── validation ────────────────────────────────────────────────────────

    #[test]
    fn validate_rejects_unknown_template_name() {
        let err = validate_reference_defs(&[ref_def("bogus_index")]).unwrap_err();
        assert!(
            err.to_string().contains("not a known builder template"),
            "{err}"
        );
        assert!(err.to_string().contains("bwa_index"), "{err}");
    }

    #[test]
    fn validate_accepts_known_templates_and_shells() {
        let known = ref_def("bwa_index");
        let mut shell = ref_def("mkdir -p refs && echo x > {output}");
        shell.name = "genome_faidx".into();
        validate_reference_defs(&[known, shell]).unwrap();
    }

    #[test]
    fn validate_rejects_template_without_output() {
        let mut def = ref_def("bwa_index");
        def.output = String::new();
        let err = validate_reference_defs(&[def]).unwrap_err();
        assert!(err.to_string().contains("requires an 'output'"), "{err}");
    }

    #[test]
    fn validate_rejects_duplicate_reference_names() {
        let defs = [ref_def("bwa_index"), ref_def("samtools_faidx")];
        let err = validate_reference_defs(&defs).unwrap_err();
        assert!(
            err.to_string()
                .contains("duplicate reference name 'genome'"),
            "{err}"
        );
    }

    // ── parse pipeline wiring ─────────────────────────────────────────────

    #[test]
    fn parse_expands_template_builds() {
        let toml = "[workflow]\nname = \"w\"\nversion = \"1.0.0\"\n\n\
                    [[references]]\nname = \"genome\"\nsource = \"refs/genome.fa\"\n\
                    output = \"refs/genome.fa.bwt\"\nbuild = \"bwa_index\"\nthreads = 8\n";
        let config = WorkflowConfig::parse(toml).unwrap();
        assert_eq!(config.references.len(), 1);
        // The parsed build is the canonical command; `{output}` stays for the
        // render pipeline, which fills in the config-expanded output path.
        assert_eq!(
            config.references[0].build,
            "mkdir -p \"$(dirname {output})\" && bwa index -p refs/genome.fa refs/genome.fa"
        );
    }

    #[test]
    fn parse_rejects_unknown_template_name() {
        let toml = "[workflow]\nname = \"w\"\nversion = \"1.0.0\"\n\n\
                    [[references]]\nname = \"genome\"\nsource = \"refs/genome.fa\"\n\
                    output = \"refs/genome.fa.fai\"\nbuild = \"bogus_index\"\n";
        let err = WorkflowConfig::parse(toml).unwrap_err();
        assert!(
            err.to_string().contains("not a known builder template"),
            "{err}"
        );
    }

    #[test]
    fn parse_requires_source_for_template() {
        let toml = "[workflow]\nname = \"w\"\nversion = \"1.0.0\"\n\n\
                    [[references]]\nname = \"genome\"\noutput = \"refs/genome.fa.bwt\"\n\
                    build = \"bwa_index\"\n";
        let err = WorkflowConfig::parse(toml).unwrap_err();
        assert!(err.to_string().contains("requires a 'source'"), "{err}");
    }

    #[test]
    fn parse_keeps_handwritten_builds_untouched() {
        let cmd = "mkdir -p refs && echo built > {output}";
        let toml = format!(
            "[workflow]\nname = \"w\"\nversion = \"1.0.0\"\n\n\
             [[references]]\nname = \"genome\"\nsource = \"refs/genome.fa\"\n\
             output = \"refs/genome.fa.fai\"\nbuild = \"{cmd}\"\n"
        );
        let config = WorkflowConfig::parse(&toml).unwrap();
        assert_eq!(config.references[0].build, cmd);
    }

    // ── keyed references (config.<name> = output) ─────────────────────────

    #[test]
    fn parse_injects_keyed_config_value_for_reference_name() {
        let toml = "[workflow]\nname = \"w\"\nversion = \"1.0.0\"\n\n\
                    [config]\nref_dir = \"refs\"\n\n\
                    [[references]]\nname = \"genome\"\nsource = \"{config.ref_dir}/genome.fa\"\n\
                    output = \"{config.ref_dir}/genome.fa.fai\"\nbuild = \"samtools_faidx\"\n";
        let config = WorkflowConfig::parse(toml).unwrap();
        // The injected value is config-pre-expanded: rules see the real path.
        assert_eq!(
            config.config.get("genome").and_then(|v| v.as_str()),
            Some("refs/genome.fa.fai")
        );
    }

    #[test]
    fn parse_keeps_explicit_config_value_over_injection() {
        let toml = "[workflow]\nname = \"w\"\nversion = \"1.0.0\"\n\n\
                    [config]\ngenome = \"mine.fa\"\n\n\
                    [[references]]\nname = \"genome\"\nsource = \"refs/genome.fa\"\n\
                    output = \"refs/genome.fa.fai\"\nbuild = \"samtools_faidx\"\n";
        let config = WorkflowConfig::parse(toml).unwrap();
        assert_eq!(
            config.config.get("genome").and_then(|v| v.as_str()),
            Some("mine.fa")
        );
    }

    #[test]
    fn validate_on_raw_config_rejects_unknown_template() {
        // A config constructed without the parse pipeline still validates the
        // template-name contract.
        let toml = "[workflow]\nname = \"w\"\nversion = \"1.0.0\"\n\n\
                    [[references]]\nname = \"genome\"\nsource = \"refs/genome.fa\"\n\
                    output = \"refs/genome.fa.fai\"\nbuild = \"nope\"\n";
        let config: WorkflowConfig = toml::from_str(toml).unwrap();
        let err = config.validate().unwrap_err();
        assert!(
            err.to_string().contains("not a known builder template"),
            "{err}"
        );
    }

    // ── end-to-end build execution (mirrors the CLI's reference build step) ──

    /// Run the exact pipeline the CLI uses for a reference build: parse
    /// (template expansion), render with a synthetic rule, execute via
    /// `sh -c` in the workflow directory.
    fn run_reference_build(
        toml: &str,
        workdir: &std::path::Path,
        extra_path: Option<&std::path::Path>,
    ) -> std::process::Output {
        let config = WorkflowConfig::parse(toml).unwrap();
        let def = &config.references[0];
        let wildcard_values: HashMap<String, String> = HashMap::new();
        let output_path =
            crate::executor::checkpoint::expand_config_in_path(&def.output, &wildcard_values);
        let cmd = crate::executor::process::render_shell_command(
            &def.build,
            &crate::rule::Rule {
                name: format!("ref:{}", def.name),
                output: vec![output_path.clone()].into(),
                ..Default::default()
            },
            &wildcard_values,
        );
        let mut command = std::process::Command::new("sh");
        command.arg("-c").arg(&cmd).current_dir(workdir);
        if let Some(bin) = extra_path {
            command.env(
                "PATH",
                format!(
                    "{}:{}",
                    bin.display(),
                    std::env::var("PATH").unwrap_or_default()
                ),
            );
        }
        command.output().unwrap()
    }

    #[cfg(unix)]
    #[test]
    fn end_to_end_template_build_with_mock_bwa() {
        use std::os::unix::fs::PermissionsExt;

        let dir = tempfile::tempdir().unwrap();
        let bin = dir.path().join("bin");
        std::fs::create_dir_all(&bin).unwrap();
        // Mock `bwa index`: records the -p prefix and input, and
        // materializes the five index files the real tool writes.
        std::fs::write(
            bin.join("bwa"),
            "#!/bin/sh\n\
             prefix=\ninput=\n\
             while [ \"$#\" -gt 0 ]; do\n\
               case \"$1\" in\n\
                 -p) prefix=\"$2\"; shift 2 ;;\n\
                 *) input=\"$1\"; shift ;;\n\
               esac\n\
             done\n\
             printf '%s' \"$input\" > mock_input.log\n\
             printf '%s' \"$prefix\" > mock_prefix.log\n\
             for ext in amb ann bwt pac sa; do : > \"$prefix.$ext\"; done\n",
        )
        .unwrap();
        std::fs::set_permissions(bin.join("bwa"), std::fs::Permissions::from_mode(0o755)).unwrap();
        std::fs::create_dir(dir.path().join("refs")).unwrap();
        std::fs::write(dir.path().join("refs/genome.fa"), ">chr1\nACGT\n").unwrap();

        let toml = "[workflow]\nname = \"e2e\"\nversion = \"1.0.0\"\n\n\
                    [[references]]\nname = \"genome_bwa_index\"\nsource = \"refs/genome.fa\"\n\
                    output = \"refs/genome.fa.bwt\"\nbuild = \"bwa_index\"\nthreads = 4\n";
        let out = run_reference_build(toml, dir.path(), Some(bin.as_path()));
        assert!(
            out.status.success(),
            "build failed: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        // The artifact the engine checks for rebuilds is created.
        assert!(
            dir.path().join("refs/genome.fa.bwt").exists(),
            "mock bwa must create the declared output"
        );
        // The template expanded to the canonical command: input is the
        // source and the prefix was derived from the .bwt output.
        assert_eq!(
            std::fs::read_to_string(dir.path().join("mock_input.log")).unwrap(),
            "refs/genome.fa"
        );
        assert_eq!(
            std::fs::read_to_string(dir.path().join("mock_prefix.log")).unwrap(),
            "refs/genome.fa"
        );
    }

    #[test]
    fn end_to_end_handwritten_build_runs_unchanged() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir(dir.path().join("refs")).unwrap();
        let toml = "[workflow]\nname = \"e2e\"\nversion = \"1.0.0\"\n\n\
                    [[references]]\nname = \"genome\"\nsource = \"refs/genome.fa\"\n\
                    output = \"refs/genome.idx\"\n\
                    build = \"mkdir -p refs && echo built-v1 > {output}\"\n";
        let out = run_reference_build(toml, dir.path(), None);
        assert!(
            out.status.success(),
            "build failed: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        assert_eq!(
            std::fs::read_to_string(dir.path().join("refs/genome.idx")).unwrap(),
            "built-v1\n"
        );
    }
}
