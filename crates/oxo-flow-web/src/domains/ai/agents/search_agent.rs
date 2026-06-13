//! Search Agent — web search integration with quality scoring.
//!
//! Searches for bioinformatics best practices, tool recommendations,
//! and parameter guidance. Results are scored by relevance and source reliability.
//!
//! Context engineering: uses progressive disclosure — starts with a lightweight
//! knowledge base query and tries web search as a just-in-time enhancement.
//! Security: only tool names/params are sent, never user data paths.

use super::types::*;

/// Search for bioinformatics best practices related to an intent.
/// Uses knowledge base first (deterministic, 0 cost), then enriches with web search.
#[allow(clippy::collapsible_if)]
pub async fn search_practices(intent: &str, tool_names: &[String]) -> SearchResult {
    let query = build_search_query(intent, tool_names);
    let mut results = Vec::new();

    // Level 1: Knowledge base (deterministic, zero cost, always available)
    results.push(knowledge_base_lookup(&query));

    // Level 2: Web search (just-in-time, may be unavailable)
    let url = format!(
        "https://api.duckduckgo.com/?q={}&format=json&no_html=1&skip_disambig=1",
        urlencoding::encode(&query)
    );
    if let Ok(resp) = reqwest::get(&url).await {
        if let Ok(body) = resp.text().await {
            if let Ok(parsed) = serde_json::from_str::<serde_json::Value>(&body) {
                if let Some(abstract_text) = parsed.get("AbstractText").and_then(|v| v.as_str())
                    && !abstract_text.is_empty()
                {
                    results.push(SearchItem {
                        title: parsed
                            .get("AbstractSource")
                            .and_then(|v| v.as_str())
                            .unwrap_or("Web")
                            .to_string(),
                        url: parsed
                            .get("AbstractURL")
                            .and_then(|v| v.as_str())
                            .unwrap_or("")
                            .to_string(),
                        snippet: abstract_text.to_string(),
                        relevance_score: 0.7,
                        source: "web_search".into(),
                    });
                }
                if let Some(topics) = parsed.get("RelatedTopics").and_then(|v| v.as_array()) {
                    for topic in topics.iter().take(3) {
                        if let Some(text) = topic.get("Text").and_then(|v| v.as_str()) {
                            results.push(SearchItem {
                                title: topic
                                    .get("FirstURL")
                                    .and_then(|v| v.as_str())
                                    .unwrap_or("Related")
                                    .to_string(),
                                url: topic
                                    .get("FirstURL")
                                    .and_then(|v| v.as_str())
                                    .unwrap_or("")
                                    .to_string(),
                                snippet: text.to_string(),
                                relevance_score: 0.5,
                                source: "web_search".into(),
                            });
                        }
                    }
                }
            }
        }
    }

    // Sort by relevance score
    results.sort_by(|a, b| {
        b.relevance_score
            .partial_cmp(&a.relevance_score)
            .unwrap_or(std::cmp::Ordering::Equal)
    });

    let total = results.len();
    let score = results.first().map(|r| r.relevance_score).unwrap_or(0.0);

    SearchResult {
        query,
        results,
        total,
        score,
    }
}

/// Build a search query from intent and tool names.
fn build_search_query(intent: &str, tool_names: &[String]) -> String {
    let mut parts: Vec<&str> = Vec::new();
    for t in tool_names {
        parts.push(t);
    }
    parts.push(intent);
    parts.push("bioinformatics best practices 2026");
    parts.join(" ")
}

/// Built-in knowledge base (deterministic, 0 cost, no external API).
fn knowledge_base_lookup(query: &str) -> SearchItem {
    let lower = query.to_lowercase();
    if lower.contains("rna-seq") || lower.contains("rnaseq") {
        SearchItem {
            title: "RNA-seq Best Practices (ENCODE)".into(),
            url: "https://www.encodeproject.org/data-standards/rna-seq/".into(),
            snippet: "ENCODE RNA-seq best practices recommend: stranded PE 100bp+ reads, \
                      STAR for alignment (2-pass for novel junctions), featureCounts or RSEM for \
                      quantification, and DESeq2/edgeR for differential expression. \
                      Minimum 3 biological replicates per condition."
                .into(),
            relevance_score: 0.95,
            source: "knowledge_base".into(),
        }
    } else if lower.contains("variant") || lower.contains("wgs") {
        SearchItem {
            title: "GATK Best Practices for Germline SNP & Indel Discovery".into(),
            url: "https://gatk.broadinstitute.org/".into(),
            snippet: "GATK Best Practices: BWA-MEM alignment, MarkDuplicates, \
                      BaseRecalibrator, HaplotypeCaller in GVCF mode."
                .into(),
            relevance_score: 0.95,
            source: "knowledge_base".into(),
        }
    } else if lower.contains("chip-seq") || lower.contains("chipseq") {
        SearchItem {
            title: "ENCODE ChIP-seq Best Practices".into(),
            url: "https://www.encodeproject.org/chip-seq/histone/".into(),
            snippet: "ENCODE ChIP-seq: 20M+ reads for TF, 45M+ for histone, \
                      Bowtie2 alignment, MACS2 peak calling, IDR analysis."
                .into(),
            relevance_score: 0.9,
            source: "knowledge_base".into(),
        }
    } else {
        SearchItem {
            title: "Reproducible Bioinformatics Workflows".into(),
            url: "https://doi.org/10.1038/s41587-024-02354-1".into(),
            snippet: "Use containerized environments (Docker/Singularity), \
                      parameter files, version-controlled pipelines, \
                      comprehensive logging, and automated QC checks."
                .into(),
            relevance_score: 0.8,
            source: "knowledge_base".into(),
        }
    }
}

/// Score a search result for relevance and source reliability.
pub fn score_result(item: &SearchItem, query: &str) -> f64 {
    let mut score = item.relevance_score;
    if item.source == "knowledge_base" {
        score *= 1.2;
    }
    let lower_query = query.to_lowercase();
    for word in lower_query.split_whitespace() {
        if item.title.to_lowercase().contains(word) {
            score += 0.05;
        }
        if item.snippet.to_lowercase().contains(word) {
            score += 0.03;
        }
    }
    score.min(1.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_search_rnaseq_returns_kb() {
        let result = search_practices("RNA-seq", &["STAR".into(), "featureCounts".into()]).await;
        assert!(!result.results.is_empty(), "should have KB results");
        assert!(result.results.iter().any(|r| r.source == "knowledge_base"));
    }

    #[tokio::test]
    async fn test_search_variant_returns_kb() {
        let result = search_practices("WGS variant calling", &["GATK".into()]).await;
        assert!(!result.results.is_empty());
        assert!(result.results[0].snippet.contains("GATK"));
    }

    #[test]
    fn test_score_result_kb() {
        let item = SearchItem {
            title: "RNA-seq Best Practices".into(),
            url: "".into(),
            snippet: "STAR alignment for RNA-seq".into(),
            relevance_score: 0.9,
            source: "knowledge_base".into(),
        };
        let score = score_result(&item, "RNA-seq STAR");
        assert!(score > 0.9 && score <= 1.0);
    }

    #[test]
    fn test_build_search_query() {
        let q = build_search_query("RNA-seq", &["STAR".into(), "fastp".into()]);
        assert!(q.contains("STAR") && q.contains("RNA-seq") && q.contains("best practices"));
    }

    #[test]
    fn test_knowledge_base_rnaseq() {
        let item = knowledge_base_lookup("RNA-seq analysis");
        assert!(item.snippet.contains("STAR") && item.snippet.contains("DESeq2"));
        assert_eq!(item.source, "knowledge_base");
    }

    #[test]
    fn test_knowledge_base_default() {
        let item = knowledge_base_lookup("unknown analysis type");
        assert!(item.snippet.contains("container") || item.snippet.contains("reproducible"));
    }
}
