#!/usr/bin/env python3
"""Generate eval/gold/tool.csv from the embedded knowledge base.

Deterministic (fixed stride sampling, no randomness): re-running the
script on an unchanged knowledge base reproduces the same CSV, so the
gold set is reviewable and diffable across knowledge refreshes.

Rows are assembled from:
  - a curated table of purpose/alias/negative queries (hand-written,
    kept in this file so reviewers can audit them), and
  - programmatic exact-name / version-pin rows sampled from the
    bioconda / nf-core / bio.tools / commercial tables with provenance
    URLs pointing at the primary source.

The script only writes the CSV; it never calls an AI provider. Stdlib
only, same convention as scripts/refresh-knowledge/.
"""

import csv
import json
import os
import sys

REPO_ROOT = os.path.abspath(os.path.join(os.path.dirname(__file__), "..", ".."))
KNOWLEDGE_DIR = os.path.join(REPO_ROOT, "crates", "oxo-flow-ai", "src", "knowledge")
OUT_PATH = os.path.join(REPO_ROOT, "eval", "gold", "tool.csv")

BIOCONDA_RECIPE_URL = (
    "https://github.com/bioconda/bioconda-recipes/tree/master/recipes/{name}"
)
NFCORE_MODULE_URL = "https://github.com/nf-core/modules/tree/main/modules/nf-core/{name}"


def load_jsonl(name):
    """Load a knowledge JSONL, returning list[dict] in file order."""
    rows = []
    path = os.path.join(KNOWLEDGE_DIR, name)
    with open(path, encoding="utf-8") as fh:
        for line in fh:
            line = line.strip()
            if not line or line.startswith("#"):
                continue
            rows.append(json.loads(line))
    return rows


def load_meta_dates():
    """Map knowledge source file name -> generated_at date from meta."""
    meta = json.load(open(os.path.join(KNOWLEDGE_DIR, "knowledge_meta.json"), encoding="utf-8"))
    sources = meta.get("sources", meta)
    if isinstance(sources, dict):
        sources = list(sources.values())
    dates = {}
    for src in sources:
        data_file = src.get("data_file", "")
        generated = src.get("generated_at", "")[:10]
        if data_file and generated:
            dates[os.path.basename(data_file)] = generated
    return dates


def build_bioconda_index(rows):
    """name -> row for the bioconda table."""
    return {r["n"]: r for r in rows if r.get("n")}


def curated_purpose_queries():
    """(query, tool) pairs: a bioinformatics need -> the tool that solves it.

    Versions are filled from the bioconda table at generation time so the
    gold always matches the embedded knowledge base (the judge source).
    Commercial tools resolve against the commercial table.
    """
    return [
        ("adapter trimming and quality filtering for paired-end FASTQ reads", "fastp"),
        ("read alignment to a reference genome, BWA successor with SIMD", "bwa-mem2"),
        ("splice-aware RNA-seq alignment", "star"),
        ("gene-level read counting for RNA-seq BAM files", "subread"),
        ("germline variant calling following GATK best practices", "gatk4"),
        ("functional annotation of VCF variants (Ensembl)", "ensembl-vep"),
        ("bisulfite sequencing alignment", "bismark"),
        ("de novo assembly of bacterial genomes", "spades"),
        ("metagenomic read classification with k-mers", "kraken2"),
        ("per-base quality and adapter-content report for FASTQ", "fastqc"),
        ("SAM/BAM manipulation and indexing", "samtools"),
        ("BED/GFF/interval manipulation", "bedtools"),
        ("ChIP-seq peak calling", "macs2"),
        ("transcript quantification by pseudoalignment", "salmon"),
        ("structural variant calling from WGS", "manta"),
        ("copy number variation calling from WGS", "cnvkit"),
        ("QC metric aggregation into one interactive report", "multiqc"),
        ("marking PCR and optical duplicates in BAM files", "picard"),
        ("long-read consensus polishing", "racon"),
        ("pangenome graph construction", "minigraph"),
        ("viral genome consensus calling (amplicon sequencing)", "ivar"),
        ("coverage depth computation across a BAM", "mosdepth"),
        ("tandem repeat expansion genotyping", "expansionhunter"),
        ("transcript quantification from aligned RNA-seq BAM files", "stringtie"),
        ("transcriptome assembly from RNA-seq reads", "trinity"),
        ("predicting the protein effect of amino acid substitutions", "provean"),
        ("read-level error correction for long reads", "canu"),
        ("mitochondrial genome assembly and annotation", "mitoz"),
        ("phylogenetic tree inference from a multiple sequence alignment", "iqtree"),
        ("multi-omics pathway enrichment", "gsea"),
    ]


def curated_alias_queries():
    """(query, tool) pairs where the query uses a non-canonical name."""
    return [
        ("what is fastqC", "fastqc"),
        ("what is the bwa mem2 aligner", "bwa-mem2"),
        ("what is GATK4", "gatk4"),
        ("what does samtool do", "samtools"),
        ("what is VEP (variant annotation)", "ensembl-vep"),
        ("what is the star aligner", "star"),
        ("what is featurecounts", "subread"),
        ("what is bcl2fastq", "bcl2fastq"),
        ("what does the cell ranger pipeline do", "cellranger"),
        ("what is picard tools", "picard"),
        ("what is bedtool", "bedtools"),
        ("what is MACS2", "macs2"),
    ]


def curated_negative_queries():
    """Fake tool names the AI must reject (negative samples)."""
    return [
        "what is bwa_mem4",
        "what does rnaseq_ultra_aligner do",
        "what is fastq_super_cleaner",
        "what does gene_exploder do",
        "what is metagenome_magic",
        "what is vcf_annihilator",
        "what does aligninator do",
        "what is seq_polisher_pro",
        "what does crispr_finder_x do",
        "what is epigenome_wizard",
    ]


def sample_rows(rows, name_key, stride, offset=0):
    """Deterministic fixed-stride sample of a table, sorted by name."""
    ordered = sorted(rows, key=lambda r: r.get(name_key, "").lower())
    return ordered[offset::stride]


def emit():
    dates = load_meta_dates()
    bioconda = load_jsonl("bioconda_tools.jsonl")
    bioconda_index = build_bioconda_index(bioconda)
    nfcore = load_jsonl("nfcore_modules.jsonl")
    commercial = load_jsonl("commercial_tools.jsonl")
    commercial_index = {r["n"]: r for r in commercial}
    biotools = load_jsonl("biotools_overlay.jsonl")

    header = [
        "id", "layer", "query", "query_type", "expected_tool", "expected_version",
        "expected_source", "negative_sample", "provenance_url", "provenance_date",
        "difficulty", "gold_draft_by", "review_status", "reviewer",
        "review_comment", "review_date",
    ]
    rows = []
    next_id = [1]

    def add(query, query_type, tool, version, source, negative, url, date, difficulty):
        rows.append({
            "id": f"tool-{next_id[0]:03d}",
            "layer": "tool",
            "query": query,
            "query_type": query_type,
            "expected_tool": tool,
            "expected_version": version,
            "expected_source": source,
            "negative_sample": negative,
            "provenance_url": url,
            "provenance_date": date,
            "difficulty": difficulty,
            "gold_draft_by": "claude",
            "review_status": "draft",
            "reviewer": "",
            "review_comment": "",
            "review_date": "",
        })
        next_id[0] += 1

    def resolve_version(name, fallback=""):
        """Version from the bioconda table, else the commercial table."""
        row = bioconda_index.get(name) or commercial_index.get(name)
        return (row or {}).get("v", fallback)

    # ── Purpose queries (curated) ──────────────────────────────────────
    for query, tool in curated_purpose_queries():
        if tool in bioconda_index:
            source, date = "bioconda", dates.get("bioconda_tools.jsonl", "")
            url = BIOCONDA_RECIPE_URL.format(name=tool)
        elif tool in commercial_index:
            row = commercial_index[tool]
            source, date = "commercial", row.get("checked_at", "")
            url = row.get("url") or "https://www.google.com/search?q=" + tool
        else:
            print(f"WARN: curated purpose tool '{tool}' not in knowledge base — skipped")
            continue
        add(query, "purpose", tool, resolve_version(tool), source, 0, url, date, "easy")

    # ── Alias queries (curated) ────────────────────────────────────────
    for query, tool in curated_alias_queries():
        row = bioconda_index.get(tool) or commercial_index.get(tool)
        if row is None:
            print(f"WARN: curated alias tool '{tool}' not in knowledge base — skipped")
            continue
        if tool in bioconda_index:
            source, url, date = (
                "bioconda",
                BIOCONDA_RECIPE_URL.format(name=tool),
                dates.get("bioconda_tools.jsonl", ""),
            )
        else:
            source, url, date = "commercial", row.get("url") or "", row.get("checked_at", "")
        add(query, "alias", tool, resolve_version(tool), source, 0, url, date, "medium")

    # ── Negative samples (curated) ─────────────────────────────────────
    for query in curated_negative_queries():
        add(query, "negative", "", "", "none", 1, "", "", "medium")

    # ── Exact-name rows sampled from bioconda ──────────────────────────
    for row in sample_rows(bioconda, "n", stride=181, offset=17):
        name = row["n"]
        add(
            f"what is {name} and what is its latest bioconda version",
            "exact_name", name, row.get("v", ""), "bioconda", 0,
            BIOCONDA_RECIPE_URL.format(name=name),
            dates.get("bioconda_tools.jsonl", ""), "easy",
        )

    # ── Version-pin rows sampled from bioconda ─────────────────────────
    for row in sample_rows(bioconda, "n", stride=379, offset=251):
        name = row["n"]
        add(
            f"what is the latest version of {name} available in bioconda",
            "version_pin", name, row.get("v", ""), "bioconda", 0,
            BIOCONDA_RECIPE_URL.format(name=name),
            dates.get("bioconda_tools.jsonl", ""), "medium",
        )

    # ── nf-core module rows (no pinned versions in our data) ───────────
    for row in sample_rows(nfcore, "n", stride=89, offset=5):
        name = row["n"]
        add(
            f"what does the nf-core module {name} do",
            "exact_name", name, "", "nfcore", 0,
            NFCORE_MODULE_URL.format(name=name),
            dates.get("nfcore_modules.jsonl", ""), "medium",
        )

    # ── Commercial rows ────────────────────────────────────────────────
    for row in sample_rows(commercial, "n", stride=2, offset=0):
        name = row["n"]
        if not row.get("auto") or not row.get("v"):
            continue  # only auto-checked tools with a version are answerable
        add(
            f"what is the latest version of the commercial tool {name}",
            "commercial", name, row["v"], "commercial", 0,
            row.get("url") or "", row.get("checked_at", ""), "hard",
        )

    # ── bio.tools rows (description recall) ────────────────────────────
    for row in sample_rows([r for r in biotools if r.get("description")], "n", stride=157, offset=31):
        name = row["n"]
        add(
            f"what does the bio.tools entry for {name} say it does",
            "purpose", name, "", "biotools", 0,
            row.get("homepage") or f"https://bio.tools/{name}",
            dates.get("biotools_overlay.jsonl", ""), "hard",
        )

    os.makedirs(os.path.dirname(OUT_PATH), exist_ok=True)
    with open(OUT_PATH, "w", newline="", encoding="utf-8") as fh:
        writer = csv.DictWriter(fh, fieldnames=header, quoting=csv.QUOTE_MINIMAL)
        writer.writeheader()
        writer.writerows(rows)
    print(f"Wrote {len(rows)} rows to {os.path.relpath(OUT_PATH, REPO_ROOT)}")


if __name__ == "__main__":
    emit()
