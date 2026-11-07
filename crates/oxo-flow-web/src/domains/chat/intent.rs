//! Deterministic intent routing for the chat surface (issue #1040 P2-4).
//!
//! The chat page is advertised as an "AI copilot chat", but every message
//! used to be forced through the pipeline generator: a turn like
//! `"Reply with the single word OK."` burned a full workflow-generation run
//! (163 s in the audit) and returned a DNA-seq pipeline with no
//! conversational text at all.
//!
//! Routing is a keyword/pattern classifier, deliberately NOT a model call:
//! chat is the cheapest surface, and spending a second completion just to
//! decide whether the first one should run would double the token cost of
//! every request. The heuristics are conservative in the direction the
//! product needs — an imperative that plausibly asks for a workflow goes to
//! generation — while obvious conversational turns (greetings, questions
//! that ask for an explanation, meta-instructions like "reply with OK") do
//! not. An explicit run scope or attached data always wins: those requests
//! need the tool loop regardless of phrasing.
//!
//! The classifier lives beside its only caller (the chat domain) so the two
//! chat routes share one decision; pipeline generation itself remains in
//! [`super::service`].

use super::types::ChatContext;

/// What the caller is asking the chat surface for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChatMode {
    /// Answer conversationally in plain text; no workflow generation.
    Chat,
    /// Run the pipeline-generation agent (validated `.oxoflow` TOML).
    Pipeline,
}

impl ChatMode {
    /// The value carried in the API `mode` field.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Chat => "chat",
            Self::Pipeline => "pipeline",
        }
    }
}

/// Words that name the engine's own artifacts and environment vocabulary.
/// These are pipeline intent on their own ("WGS variant calling workflow").
const PIPELINE_ARTIFACTS: &[&str] = &[
    "pipeline",
    "pipelines",
    "workflow",
    "workflows",
    "oxoflow",
    "oxo-flow",
    "toml",
    "rule",
    "rules",
    "dag",
    "wildcard",
    "wildcards",
    "samplesheet",
    "sample-sheet",
    "conda",
    "mamba",
    "pixi",
    "docker",
    "singularity",
    "apptainer",
    "slurm",
    "checkpoint",
    "provenance",
    "depends",
    "depends_on",
    "dry-run",
    "lint",
];

/// Assays, file formats, and tools. A message naming one of these is
/// describing work the engine exists to run.
const OMICS_TERMS: &[&str] = &[
    "rna-seq",
    "rnaseq",
    "rna",
    "dna-seq",
    "dnaseq",
    "wgs",
    "wes",
    "chip-seq",
    "chipseq",
    "atac-seq",
    "atacseq",
    "scrna",
    "scrna-seq",
    "single-cell",
    "transcriptome",
    "transcriptomic",
    "variant",
    "variants",
    "germline",
    "somatic",
    "snv",
    "snvs",
    "indel",
    "indels",
    "alignment",
    "alignments",
    "differential",
    "expression",
    "quantification",
    "assembly",
    "annotation",
    "metagenomics",
    "metagenome",
    "microbiome",
    "proteomics",
    "spatial",
    "sequencing",
    "seq",
    "fastq",
    "fq",
    "fasta",
    "bam",
    "cram",
    "vcf",
    "bcf",
    "gtf",
    "gff",
    "hg38",
    "grch38",
    "mm10",
    "genome",
    "fastqc",
    "multiqc",
    "fastp",
    "trimmomatic",
    "cutadapt",
    "bwa",
    "bwa-mem2",
    "hisat2",
    "bowtie",
    "bowtie2",
    "salmon",
    "kallisto",
    "featurecounts",
    "htseq",
    "samtools",
    "bcftools",
    "gatk",
    "picard",
    "markduplicates",
    "cellranger",
    "seurat",
    "scanpy",
    "macs2",
    "deeptools",
    "kraken2",
    "metaphlan",
    "qiime2",
    "dada2",
    "nextflow",
    "snakemake",
    "nf-core",
    "spades",
    "megahit",
    "prokka",
    "abricate",
    "trinity",
    "stringtie",
    "minimap2",
    "nanopore",
    "pacbio",
    "illumina",
    "10x",
];

/// Generic words that only imply a pipeline when paired with an action
/// ("run my samples", "make it 8 threads"); on their own they are chat
/// topics ("is my run done?").
const WEAK_TERMS: &[&str] = &[
    "data",
    "dataset",
    "datasets",
    "file",
    "files",
    "sample",
    "samples",
    "run",
    "runs",
    "job",
    "jobs",
    "step",
    "steps",
    "task",
    "tasks",
    "node",
    "nodes",
    "assay",
    "experiment",
    "threads",
    "thread",
    "cpu",
    "cpus",
    "memory",
    "resource",
    "resources",
    "parameter",
    "parameters",
    "config",
    "configuration",
];

/// Imperatives that ask the engine to do something. Presence alone is not
/// enough — the message must also carry pipeline vocabulary.
const ACTION_VERBS: &[&str] = &[
    "build",
    "create",
    "design",
    "generate",
    "write",
    "make",
    "construct",
    "convert",
    "translate",
    "import",
    "run",
    "execute",
    "diagnose",
    "debug",
    "fix",
    "optimize",
    "optimise",
    "add",
    "modify",
    "edit",
    "update",
    "configure",
    "setup",
    "resume",
    "validate",
    "lint",
    "scaffold",
    "draft",
    "produce",
    "develop",
    "implement",
    "automate",
    "port",
    "migrate",
    "replace",
    "analyze",
    "analyse",
    "process",
    "align",
    "map",
    "quantify",
    "assemble",
    "annotate",
    "trim",
    "filter",
    "merge",
    "sort",
    "index",
    "call",
    "tune",
    "increase",
    "decrease",
    "reduce",
    "set",
    "rerun",
    "restart",
    "submit",
    "install",
    "extract",
    "export",
    "continue",
    "remove",
    "delete",
    "change",
    "switch",
    "enable",
    "disable",
    "include",
    "exclude",
];

/// Run-diagnosis vocabulary. Any of these next to a run/job/rule word means
/// the caller needs the tool loop (run status, logs), not small talk.
const DIAGNOSIS_TERMS: &[&str] = &[
    "diagnose",
    "debug",
    "troubleshoot",
    "failure",
    "failed",
    "fail",
    "fails",
    "error",
    "errors",
    "crash",
    "crashed",
    "stderr",
    "traceback",
    "exit",
];

/// The context a diagnosis term must attach to.
const DIAGNOSIS_CONTEXT: &[&str] = &[
    "run",
    "runs",
    "rule",
    "rules",
    "step",
    "steps",
    "job",
    "jobs",
    "pipeline",
    "workflow",
    "process",
    "execution",
    "task",
    "node",
    "sample",
    "samples",
];

const GREETING_TOKENS: &[&str] = &[
    "hi", "hello", "hey", "thanks", "thank", "thx", "cheers", "bye", "goodbye",
];

const QUESTION_PREFIXES: &[&str] = &[
    "what ",
    "what's ",
    "whats ",
    "why ",
    "how ",
    "who ",
    "when ",
    "where ",
    "which ",
    "is ",
    "are ",
    "was ",
    "were ",
    "do ",
    "does ",
    "did ",
    "can ",
    "could ",
    "should ",
    "would ",
    "will ",
    "explain",
    "describe",
    "define",
    "tell me",
    "please ",
    "difference between ",
    "is it ",
    "are there ",
];

/// Small-talk phrases that are chat even when they mention an artifact
/// ("thanks, that pipeline was great"). A co-occurring action verb disables
/// this short-circuit so "thanks, build an RNA-seq pipeline" still routes
/// to generation.
const SMALL_TALK_PHRASES: &[&str] = &[
    "how are you",
    "who are you",
    "what can you do",
    "what are you",
    "good morning",
    "good afternoon",
    "good evening",
    "nice to meet",
    "thank you",
];

/// Classify a chat message.
///
/// Precedence (first match wins):
///
/// 1. explicit run scope or attached data → generation;
/// 2. run-diagnosis vocabulary next to a run/rule/job word → generation
///    (the diagnosis tools live in the agent loop);
/// 3. pure small talk → chat;
/// 4. no pipeline vocabulary at all → chat;
/// 5. explanation questions without an action verb → chat;
/// 6. otherwise → generation.
pub fn classify_message(
    message: &str,
    context: Option<&ChatContext>,
    run_scoped: bool,
) -> ChatMode {
    if run_scoped || context_requests_generation(context) {
        return ChatMode::Pipeline;
    }

    let normalized = normalize(message);
    if normalized.is_empty() {
        return ChatMode::Chat;
    }
    let tokens: Vec<&str> = normalized.split_whitespace().collect();
    let has_action = contains_any(&tokens, ACTION_VERBS);
    let has_artifact = contains_any(&tokens, PIPELINE_ARTIFACTS);
    let has_omics = contains_any(&tokens, OMICS_TERMS);
    let has_weak = contains_any(&tokens, WEAK_TERMS);

    if is_run_diagnosis(&tokens) {
        return ChatMode::Pipeline;
    }
    if is_small_talk(&normalized, &tokens) && !has_action {
        return ChatMode::Chat;
    }

    // Weak vocabulary only counts when the caller asks to do something
    // ("analyze my data"); a bare "my data looks odd" is a chat turn.
    let has_pipeline_vocabulary = has_artifact || has_omics || (has_weak && has_action);
    if !has_pipeline_vocabulary {
        return ChatMode::Chat;
    }

    // "What is a DAG?" asks for an explanation; "How do I build a DAG?"
    // carries an action verb and still routes to generation.
    if is_question(&normalized) && !has_action {
        return ChatMode::Chat;
    }

    ChatMode::Pipeline
}

/// Attached data is an explicit request to analyze it — generation, not
/// conversation, regardless of the message wording.
fn context_requests_generation(context: Option<&ChatContext>) -> bool {
    let Some(ctx) = context else {
        return false;
    };
    let has_paths = ctx
        .data_paths
        .as_ref()
        .is_some_and(|paths| !paths.is_empty());
    let has_samplesheet = ctx
        .samplesheet
        .as_ref()
        .is_some_and(|sheet| !sheet.trim().is_empty());
    has_paths || has_samplesheet
}

/// Lowercase and turn punctuation into separators while keeping the
/// characters that carry meaning in this vocabulary (`-`, `_`, `{`, `}`,
/// `.`, digits), so `RNA-seq` and `{sample}` survive tokenization.
fn normalize(message: &str) -> String {
    let mut out = String::with_capacity(message.len());
    for ch in message.chars().flat_map(char::to_lowercase) {
        if ch.is_alphanumeric() || matches!(ch, '-' | '_' | '{' | '}' | '.') {
            out.push(ch);
        } else {
            out.push(' ');
        }
    }
    out.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn contains_any(tokens: &[&str], terms: &[&str]) -> bool {
    terms.iter().any(|term| tokens.contains(term))
}

fn is_run_diagnosis(tokens: &[&str]) -> bool {
    contains_any(tokens, DIAGNOSIS_TERMS) && contains_any(tokens, DIAGNOSIS_CONTEXT)
}

fn is_small_talk(normalized: &str, tokens: &[&str]) -> bool {
    let padded = format!(" {normalized} ");
    GREETING_TOKENS
        .iter()
        .any(|greeting| tokens.contains(greeting))
        || SMALL_TALK_PHRASES
            .iter()
            .any(|phrase| padded.contains(&format!(" {phrase} ")))
}

fn is_question(normalized: &str) -> bool {
    let mut prefixed = String::with_capacity(normalized.len() + 1);
    prefixed.push_str(normalized);
    prefixed.push(' ');
    QUESTION_PREFIXES
        .iter()
        .any(|prefix| prefixed.starts_with(prefix))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn classify(message: &str) -> ChatMode {
        classify_message(message, None, false)
    }

    #[test]
    fn issue_example_reply_with_ok_is_chat() {
        // The exact turn from issue #1040 P2-4: it took 163 s and returned
        // a DNA-seq pipeline.
        assert_eq!(classify("Reply with the single word OK."), ChatMode::Chat);
    }

    #[test]
    fn greetings_and_small_talk_are_chat() {
        for message in [
            "hello",
            "Hi there!",
            "thanks, that pipeline was great",
            "who are you?",
            "what can you do",
            "good morning",
            "OK",
        ] {
            assert_eq!(classify(message), ChatMode::Chat, "message: {message}");
        }
    }

    #[test]
    fn explanation_questions_are_chat() {
        for message in [
            "What is FastQC?",
            "Explain what a pipeline is",
            "How does STAR alignment work?",
            "What is a DAG?",
            "Tell me about variant calling",
            "What is the difference between STAR and HISAT2?",
            "Can you explain RNA-seq quantification?",
        ] {
            assert_eq!(classify(message), ChatMode::Chat, "message: {message}");
        }
    }

    #[test]
    fn pipeline_requests_route_to_generation() {
        for message in [
            "Build an RNA-seq pipeline with STAR and featureCounts",
            "run fastqc on my samples",
            "Design a WGS germline variant calling workflow",
            "convert this nextflow workflow to oxo-flow",
            "make a FASTQ QC pipeline",
            "add a rule to my workflow for MultiQC",
            "how do I build a pipeline for scRNA-seq?",
            "Give me a minimal TOML workflow",
            "make it 8 threads",
            "analyze my data",
            "ChIP-seq peak calling with MACS2",
            "RNA-seq",
        ] {
            assert_eq!(classify(message), ChatMode::Pipeline, "message: {message}");
        }
    }

    #[test]
    fn run_diagnosis_routes_to_generation_even_as_a_question() {
        for message in [
            "why did my run fail?",
            "diagnose the failed rule",
            "debug this job error",
        ] {
            assert_eq!(classify(message), ChatMode::Pipeline, "message: {message}");
        }
        // Without a run/job/rule context, the same words are chat.
        assert_eq!(classify("what is an error?"), ChatMode::Chat);
    }

    #[test]
    fn run_scope_forces_generation() {
        assert_eq!(
            classify_message("what does this mean?", None, true),
            ChatMode::Pipeline
        );
    }

    #[test]
    fn attached_data_forces_generation() {
        let context = ChatContext {
            data_paths: Some(vec!["/data/sample1.fastq".into()]),
            samplesheet: None,
            intent: Some("dashboard".into()),
        };
        assert_eq!(
            classify_message("what can I do with this?", Some(&context), false),
            ChatMode::Pipeline
        );
    }

    #[test]
    fn page_context_hint_does_not_force_generation() {
        // The SPA always sends the page name as `context.intent`; it must
        // never turn every chat turn into a pipeline request.
        let context = ChatContext {
            data_paths: None,
            samplesheet: None,
            intent: Some("dashboard".into()),
        };
        assert_eq!(
            classify_message("Reply with the single word OK.", Some(&context), false),
            ChatMode::Chat
        );
    }

    #[test]
    fn empty_message_is_chat() {
        assert_eq!(classify(""), ChatMode::Chat);
        assert_eq!(classify("   "), ChatMode::Chat);
    }

    #[test]
    fn mode_labels_match_the_api_contract() {
        assert_eq!(ChatMode::Chat.as_str(), "chat");
        assert_eq!(ChatMode::Pipeline.as_str(), "pipeline");
    }
}
