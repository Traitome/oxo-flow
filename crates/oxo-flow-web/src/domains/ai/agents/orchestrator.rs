//! Intent inference — maps a free-text user message to a coarse analysis intent.
//!
//! Pipeline generation runs through the shared `PipelineGenAgent` in
//! `oxo-flow-ai` (issue #342); monitoring and reporting live in their own
//! agent modules.

/// Infer analysis intent from a user message.
pub fn infer_intent(message: &str) -> String {
    let lower = message.to_lowercase();
    if lower.contains("rna-seq")
        || lower.contains("rnaseq")
        || lower.contains("transcriptome")
        || lower.contains("differential expression")
    {
        "RNA-seq analysis".into()
    } else if lower.contains("variant")
        || lower.contains("wgs")
        || lower.contains("germline")
        || lower.contains("somatic")
    {
        "Variant calling".into()
    } else if lower.contains("chip-seq") || lower.contains("chipseq") {
        "ChIP-seq analysis".into()
    } else if lower.contains("single-cell") || lower.contains("scrna") || lower.contains("10x") {
        "Single-cell RNA-seq".into()
    } else if lower.contains("qc") || lower.contains("quality") || lower.contains("fastqc") {
        "Quality control".into()
    } else if lower.contains("alignment") || lower.contains("align") || lower.contains("star") {
        "Read alignment".into()
    } else {
        "Bioinformatics analysis".into()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_infer_intent() {
        assert_eq!(infer_intent("RNA-seq"), "RNA-seq analysis");
        assert_eq!(infer_intent("variant calling"), "Variant calling");
        assert_eq!(infer_intent("QC check"), "Quality control");
        assert_eq!(infer_intent("unknown stuff"), "Bioinformatics analysis");
    }
}
