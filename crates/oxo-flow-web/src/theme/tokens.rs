//! Metropolis Design System Tokens for cross-platform visual consistency.
//!
//! Provides tokenized colors, typography, and line specifications shared across
//! Web SPA, Desktop (Wry/Tao), Terminal TUI, and VS Code.

use serde::{Deserialize, Serialize};

/// Biological modality for semantic color assignment.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum BioModality {
    Genomics,
    Transcriptomics,
    Epigenomics,
    Metagenomics,
    Infrastructure,
}

impl BioModality {
    /// Primary brand color in hex.
    pub const fn primary_color(&self) -> &'static str {
        match self {
            Self::Genomics => "#3B82F6",        // Deep Indigo / Electric Cyan
            Self::Transcriptomics => "#10B981", // Emerald / Teal
            Self::Epigenomics => "#F59E0B",     // Amber / Coral
            Self::Metagenomics => "#8B5CF6",    // Violet / Magenta
            Self::Infrastructure => "#64748B",  // Slate / Zinc
        }
    }

    /// Secondary / highlight color in hex.
    pub const fn accent_color(&self) -> &'static str {
        match self {
            Self::Genomics => "#06B6D4",
            Self::Transcriptomics => "#14B8A6",
            Self::Epigenomics => "#F97316",
            Self::Metagenomics => "#D946EF",
            Self::Infrastructure => "#94A3B8",
        }
    }

    /// Background glow / halo tint in rgba format.
    pub const fn glow_color(&self) -> &'static str {
        match self {
            Self::Genomics => "rgba(59, 130, 246, 0.15)",
            Self::Transcriptomics => "rgba(16, 185, 129, 0.15)",
            Self::Epigenomics => "rgba(245, 158, 11, 0.15)",
            Self::Metagenomics => "rgba(139, 92, 246, 0.15)",
            Self::Infrastructure => "rgba(100, 116, 139, 0.15)",
        }
    }

    /// Infer modality from tool or stage name.
    pub fn infer(name: &str) -> Self {
        let lower = name.to_lowercase();
        if lower.contains("rna")
            || lower.contains("star")
            || lower.contains("salmon")
            || lower.contains("kallisto")
            || lower.contains("featurecounts")
            || lower.contains("deseq2")
            || lower.contains("cellranger")
            || lower.contains("seurat")
        {
            Self::Transcriptomics
        } else if lower.contains("chip")
            || lower.contains("atac")
            || lower.contains("macs2")
            || lower.contains("bismark")
            || lower.contains("methylation")
            || lower.contains("histone")
        {
            Self::Epigenomics
        } else if lower.contains("kraken")
            || lower.contains("bracken")
            || lower.contains("qiime")
            || lower.contains("dada2")
            || lower.contains("humann")
            || lower.contains("metaphlan")
            || lower.contains("checkm")
            || lower.contains("metagenom")
        {
            Self::Metagenomics
        } else if lower.contains("qc")
            || lower.contains("multiqc")
            || lower.contains("fastqc")
            || lower.contains("clean")
            || lower.contains("compress")
            || lower.contains("index")
        {
            Self::Infrastructure
        } else {
            Self::Genomics
        }
    }
}

/// Metropolis Design System Tokens.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MetropolisTokens {
    pub font_sans: &'static str,
    pub font_mono: &'static str,
    pub metro_line_width: u32,
    pub station_radius: u32,
    pub inter_corridor_spacing: u32,
}

pub const DEFAULT_METROPOLIS_TOKENS: MetropolisTokens = MetropolisTokens {
    font_sans: "-apple-system, BlinkMacSystemFont, 'Inter', 'Segoe UI', Roboto, sans-serif",
    font_mono: "'JetBrains Mono', 'Cascadia Code', 'SF Mono', Menlo, Consolas, monospace",
    metro_line_width: 3,
    station_radius: 6,
    inter_corridor_spacing: 32,
};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_modality_inference() {
        assert_eq!(
            BioModality::infer("STAR_align"),
            BioModality::Transcriptomics
        );
        assert_eq!(
            BioModality::infer("macs2_callpeak"),
            BioModality::Epigenomics
        );
        assert_eq!(
            BioModality::infer("kraken2_classify"),
            BioModality::Metagenomics
        );
        assert_eq!(BioModality::infer("bwa_mem"), BioModality::Genomics);
        assert_eq!(
            BioModality::infer("multiqc_report"),
            BioModality::Infrastructure
        );
    }
}
