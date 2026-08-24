#!/usr/bin/env python3
"""Refresh commercial_tools.jsonl — versions of commercial / closed-source
bioinformatics CLIs that Bioconda cannot carry.

Version sources, in decreasing order of reliability:

  1. GitHub releases API (auto): repos that publish version tags.
  2. Vendor documentation scrape (auto): clara-parabricks "Release Notes
     X.Y.Z-N" pattern on docs.nvidia.com.
  3. Pinned known-good versions (pin): set once, kept stable on purpose
     (guppy 6.5.7, bcl2fastq 2.20.0, cellranger-atac).
  4. Manual entries (manual): tools whose vendor pages are JS-rendered or
     otherwise not scriptable (Illumina support pages expose no versions in
     the raw HTML); version left "" with a note.

Output (JSONL):

    {"n", "v", "t", "source", "checked_at", "auto", "url", "note"}

  - checked_at = UTC date of the run (ISO)
  - auto       = True when the version was detected automatically
  - note       = optional provenance remark ("" when absent)

Idempotent: re-running replaces the output file atomically. A failing
source logs a warning and is dropped from the output — the file is still
written with the entries that succeeded.
"""

import argparse
import datetime


def today_iso() -> str:
    return datetime.date.today().isoformat()
import os
import re
import sys

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from common import default_out_dir, http_get, http_get_json, log, update_meta, write_jsonl  # noqa: E402

RELEASES_API = "https://api.github.com/repos/{repo}/releases/latest"

# (name, repo, description) — versions fetched via the GitHub API.
GITHUB_SOURCES = [
    ("cellranger", "10XGenomics/cellranger", "10x Genomics Chromium analysis pipeline (single-cell RNA-seq)"),
    ("dorado", "nanoporetech/dorado", "Oxford Nanopore basecaller (Guppy successor, CUDA)"),
    ("sentieon-cli", "Sentieon/sentieon-cli", "Sentieon command-line wrapper for the private Sentieon engine"),
    ("cellpose", "MouseLand/cellpose", "Cell instance segmentation for microscopy images (CPU/GPU)"),
    ("stardist", "stardist/stardist", "Star-convex cell nuclei detection (deep learning)"),
    ("pbmm2", "PacificBiosciences/pbmm2", "PacBio BAM mapper, minimap2-based"),
    ("deepvariant", "google/deepvariant", "Deep-learning variant caller for humans and other species"),
    ("ccs", "PacificBiosciences/ccs", "PacBio circular consensus sequencing (CCS) tool"),
    ("tw", "seqeralabs/tower-cli", "Seqera Tower command-line interface"),
]

# (name, description) — scraped from vendor documentation where possible,
# otherwise carried as a manual entry with v="".
SCRAPES = [
    ("bcl-convert", "Illumina BCL to FASTQ conversion tool"),
    ("dragen", "Illumina DRAGEN Bio-IT Platform (germline/somatic pipelines)"),
    ("clara-parabricks", "NVIDIA Clara Parabricks GPU-accelerated genomics suite"),
]

PARABRICK_URL = "https://docs.nvidia.com/clara/parabricks/latest/index.html"
# The docs sidebar lists releases newest-first as "Release NotesX.Y.Z-N
# Release Notes..."; the version immediately follows the anchor text.
PARABRICK_RE = re.compile(r"Release Notes(\d+\.\d+\.\d+)(?:-\d+)?")

# (name, version, description, note) — versions fixed deliberately.
PINNED = [
    ("guppy", "6.5.7", "Oxford Nanopore basecaller (legacy, pre-Dorado)", "pinned: 6.5.7 was the last widely deployed release"),
    ("bcl2fastq", "2.20.0", "Illumina BCL to FASTQ conversion tool (legacy)", "pinned: 2.20.0 is the final Illumina release"),
    ("cellranger-atac", "2.1.0", "10x Genomics single-cell ATAC-seq analysis pipeline", "pinned: vendored by 10x Genomics website"),
]

# (name, version, description, note) — vendor pages not scriptable.
MANUAL = [
    ("spaceranger", "", "10x Genomics Visium spatial-transcriptomics analysis pipeline",
     "10x Genomics website is JS-rendered; no scriptable version endpoint"),
    ("cellranger-arc", "", "10x Genomics multiome (RNA + ATAC) analysis pipeline",
     "10x Genomics website is JS-rendered; no scriptable version endpoint"),
    ("smrt-link", "", "PacBio SMRT Link analysis suite (GUI + CLI)",
     "PacBio download pages block scraping (HTTP 403)"),
    ("minknow", "", "Oxford Nanopore MinKNOW sequencing control software",
     "ONT community site requires login for downloads"),
    ("sentieon", "", "Sentieon genomics engine (closed-source license)",
     "no public version feed; the wrapper sentieon-cli is tracked via GitHub"),
    ("bases2fastq", "", "Element Biosciences AVITI bases-to-FASTQ converter",
     "Element support portal requires login"),
    ("bionano-solve", "", "Bionano Genomics Solve structural-variant analysis suite",
     "Bionano download pages block scraping (HTTP 403)"),
    ("clc", "", "Qiagen CLC Genomics Workbench / Server (closed-source)",
     "Qiagen portal requires login"),
    ("torrent-suite", "", "Ion Torrent (Thermo Fisher) analysis suite",
     "Thermo Fisher downloads require login"),
    ("parabricks-artifacts", "", "NVIDIA Clara Parabricks released artifacts",
     "binary downloads require NVIDIA NGC login"),
]


def fetch_github(name: str, repo: str, description: str) -> dict:
    """Fetch the latest release tag; `gh api` (authenticated) first,
    urllib fallback. Failures degrade to a manual entry instead of the
    tool silently disappearing from the overlay."""
    import subprocess

    def gh_fetch():
        out = subprocess.run(
            ["gh", "api", f"repos/{repo}/releases/latest", "--jq", ".tag_name"],
            capture_output=True, text=True, timeout=60,
        )
        if out.returncode == 0 and out.stdout.strip():
            return out.stdout.strip()
        return None

    def urllib_fetch():
        import urllib.request
        req = urllib.request.Request(
            f"https://api.github.com/repos/{repo}/releases/latest",
            headers={"Accept": "application/vnd.github+json", "User-Agent": "oxo-flow-refresh"},
        )
        with urllib.request.urlopen(req, timeout=60) as r:
            import json as _json
            return _json.load(r)["tag_name"]

    tag = None
    try:
        tag = gh_fetch()
    except Exception:
        pass
    if tag is None:
        try:
            tag = urllib_fetch()
        except Exception as e:
            log(f"  WARNING: GitHub releases failed for {name} ({repo}): {e}")
            return {
                "n": name, "v": "", "t": description, "source": "manual",
                "checked_at": today_iso(), "auto": False, "url": f"https://github.com/{repo}",
                "note": "release fetch failed on this run; set manually",
            }
    # Normalize: strip a leading "v" and tool-name prefixes like "cellranger-".
    v = tag
    if v.lower().startswith("v"):
        v = v[1:]
    for prefix in (f"{name}-",):
        if v.startswith(prefix):
            v = v[len(prefix):]
    return {
        "n": name, "v": v, "t": description, "source": "github-releases",
        "checked_at": today_iso(), "auto": True,
        "url": f"https://github.com/{repo}/releases",
    }

def fetch_parabricks() -> dict:
    """Scrape the Parabricks version from the NVIDIA docs page."""
    try:
        html = http_get(PARABRICK_URL, timeout=60, retries=2).decode("utf-8", errors="replace")
        # The sidebar nav interleaves markup between anchor text and version —
        # strip tags with no replacement so the sidebar reads
        # "Release Notes4.7.1-1 Release Notes4.7.0-1 ..." (newest first).
        html = re.sub(r"<[^>]+>", "", html)
        m = PARABRICK_RE.search(html)
        if m:
            return {
                "n": "clara-parabricks",
                "v": m.group(1),
                "t": "NVIDIA Clara Parabricks GPU-accelerated genomics suite",
                "source": "scrape",
                "checked_at": "",
                "auto": True,
                "url": PARABRICK_URL,
                "note": f"detected via 'Release Notes' pattern ({m.group(0)!r})",
            }
        log("  WARNING: clara-parabricks page fetched but no 'Release Notes X.Y.Z' found")
    except Exception as e:
        log(f"  WARNING: clara-parabricks scrape failed: {e}")
    return {
        "n": "clara-parabricks",
        "v": "",
        "t": "NVIDIA Clara Parabricks GPU-accelerated genomics suite",
        "source": "manual",
        "checked_at": "",
        "auto": False,
        "url": PARABRICK_URL,
        "note": "docs.nvidia.com scrape failed or pattern missing; version unknown",
    }


def fetch_illumina(name: str, description: str, support_url: str) -> dict:
    """Illumina support pages are JS-rendered SPAs — record as manual."""
    return {
        "n": name,
        "v": "",
        "t": description,
        "source": "manual",
        "checked_at": "",
        "auto": False,
        "url": support_url,
        "note": "support.illumina.com is JS-rendered; no version in raw HTML",
    }


def main() -> int:
    ap = argparse.ArgumentParser(description="Refresh commercial_tools.jsonl")
    ap.add_argument("--out", default=default_out_dir(), help="output dir (default: crates/oxo-flow-ai/src/knowledge/)")
    args = ap.parse_args()
    out_dir = args.out
    checked_at = datetime.datetime.now(datetime.timezone.utc).strftime("%Y-%m-%d")

    rows: list[dict] = []
    failed = 0

    log(f"Fetching GitHub latest releases for {len(GITHUB_SOURCES)} repos ...")
    for name, repo, desc in GITHUB_SOURCES:
        rec = fetch_github(name, repo, desc)
        if rec is None:
            failed += 1
            continue
        rec["checked_at"] = checked_at
        rows.append(rec)
        log(f"  {name}: {rec['v']} (auto={rec['auto']})")

    log("Fetching vendor pages ...")
    for name, desc, url in (
        ("bcl-convert", "Illumina BCL to FASTQ conversion tool",
         "https://support.illumina.com/sequencing/sequencing_software/bcl-convert.html"),
        ("dragen", "Illumina DRAGEN Bio-IT Platform (germline/somatic pipelines)",
         "https://support.illumina.com/sequencing/sequencing_software/dragen-bio-it-platform.html"),
    ):
        rec = fetch_illumina(name, desc, url)
        rec["checked_at"] = checked_at
        rows.append(rec)
        log(f"  {name}: manual fallback (JS-rendered page)")

    rec = fetch_parabricks()
    rec["checked_at"] = checked_at
    rows.append(rec)
    log(f"  clara-parabricks: {rec['v'] or '(manual fallback)'} (auto={rec['auto']})")

    for name, version, desc, note in PINNED:
        rows.append({"n": name, "v": version, "t": desc, "source": "pin",
                     "checked_at": checked_at, "auto": False, "url": "", "note": note})
    for name, version, desc, note in MANUAL:
        rows.append({"n": name, "v": version, "t": desc, "source": "manual",
                     "checked_at": checked_at, "auto": False, "url": "", "note": note})

    rows.sort(key=lambda r: r["n"])
    path, count, size = write_jsonl(out_dir, "commercial_tools.jsonl", rows)
    log(f"Wrote {count} commercial tools to {path} ({size} bytes).")
    update_meta(out_dir, "commercial_tools",
                {"count": count, "auto_count": sum(1 for r in rows if r["auto"]),
                 "kinds": sorted({r["source"] for r in rows})})
    return 0 if failed == 0 else 1


if __name__ == "__main__":
    sys.exit(main())
