export type MetroGranularity = 'rule' | 'process' | 'module';

export type BioModality =
  | 'genomics'
  | 'transcriptomics'
  | 'epigenomics'
  | 'metagenomics'
  | 'infrastructure';

export function inferModality(name: string): BioModality {
  const lower = name.toLowerCase();
  if (
    lower.includes('rna') ||
    lower.includes('star') ||
    lower.includes('salmon') ||
    lower.includes('kallisto') ||
    lower.includes('featurecounts') ||
    lower.includes('deseq2') ||
    lower.includes('cellranger') ||
    lower.includes('seurat')
  ) {
    return 'transcriptomics';
  }
  if (
    lower.includes('chip') ||
    lower.includes('atac') ||
    lower.includes('macs2') ||
    lower.includes('bismark') ||
    lower.includes('methylation') ||
    lower.includes('histone')
  ) {
    return 'epigenomics';
  }
  if (
    lower.includes('kraken') ||
    lower.includes('bracken') ||
    lower.includes('qiime') ||
    lower.includes('dada2') ||
    lower.includes('humann') ||
    lower.includes('metaphlan') ||
    lower.includes('checkm') ||
    lower.includes('metagenom')
  ) {
    return 'metagenomics';
  }
  if (
    lower.includes('qc') ||
    lower.includes('multiqc') ||
    lower.includes('fastqc') ||
    lower.includes('clean') ||
    lower.includes('compress') ||
    lower.includes('index')
  ) {
    return 'infrastructure';
  }
  return 'genomics';
}

export const MODALITY_COLORS: Record<BioModality, { primary: string; accent: string; glow: string }> = {
  genomics: {
    primary: 'var(--metro-genomics-primary, #3B82F6)',
    accent: 'var(--metro-genomics-accent, #06B6D4)',
    glow: 'var(--metro-genomics-glow, rgba(59, 130, 246, 0.15))',
  },
  transcriptomics: {
    primary: 'var(--metro-transcriptomics-primary, #10B981)',
    accent: 'var(--metro-transcriptomics-accent, #14B8A6)',
    glow: 'var(--metro-transcriptomics-glow, rgba(16, 185, 129, 0.15))',
  },
  epigenomics: {
    primary: 'var(--metro-epigenomics-primary, #F59E0B)',
    accent: 'var(--metro-epigenomics-accent, #F97316)',
    glow: 'var(--metro-epigenomics-glow, rgba(245, 158, 11, 0.15))',
  },
  metagenomics: {
    primary: 'var(--metro-metagenomics-primary, #8B5CF6)',
    accent: 'var(--metro-metagenomics-accent, #D946EF)',
    glow: 'var(--metro-metagenomics-glow, rgba(139, 92, 246, 0.15))',
  },
  infrastructure: {
    primary: 'var(--metro-infra-primary, #64748B)',
    accent: 'var(--metro-infra-accent, #94A3B8)',
    glow: 'var(--metro-infra-glow, rgba(100, 116, 139, 0.15))',
  },
};
