#!/usr/bin/env python3
"""Refresh bioconda_tools.jsonl — the embedded Bioconda CLI-tool table.

Port of Traitome/oxo-call-extends scripts/fetch_bioconda_tools.py: downloads
the Bioconda channel metadata and applies the SAME heuristic filters to
identify command-line bioinformatics tools:

  - library prefixes  : bioconductor-*, r-*, perl-*, python-*
  - framework prefixes: snakemake-{executor,storage,report}-plugin-*,
                        snakemake-interface-*, galaxy-*, flask-*
  - known non-CLI     : snakemake-wrapper-utils
  - summary keywords  : "python library", "r package", "api client", ...

Output (JSONL, compact {n, v, t, p} — identical schema to the shipped
bioconda_tools.jsonl):

    {"n": "<name>", "v": "<latest version>", "t": "<summary>", "p": "<platform chars>"}

where p is 'L' for linux, 'M' for macOS, 'W' for windows (concatenated in
that order), or '?' when only noarch builds exist.

Idempotent: re-running replaces the output file atomically.
"""

import argparse
import json
import os
import sys

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from common import default_out_dir, http_get, log, update_meta, write_jsonl  # noqa: E402

CHANNEL_DATA_URL = "https://conda.anaconda.org/bioconda/channeldata.json"

# Same filter rules as oxo-call-extends scripts/fetch_bioconda_tools.py.
LIBRARY_PREFIXES = (
    "bioconductor-",
    "r-",
    "perl-",
    "python-",
)
FRAMEWORK_PREFIXES = (
    "snakemake-executor-plugin-",
    "snakemake-storage-plugin-",
    "snakemake-report-plugin-",
    "snakemake-interface-",
    "galaxy-",
    "flask-",
)
LIBRARY_SUMMARY_KEYWORDS = [
    "python library",
    "python package",
    "python module",
    "python bindings",
    "python wrapper",
    "python interface",
    "r package",
    "r library",
    "perl module",
    "perl library",
    "java library",
    "javascript library",
    "ruby gem",
    "api client",
    "api wrapper",
    "sdk for",
]
KNOWN_NON_CLI = {"snakemake-wrapper-utils"}


def classify_package(name: str, meta: dict) -> tuple[bool, str]:
    """Mirror of the oxo-call-extends classify_package()."""
    for prefix in LIBRARY_PREFIXES:
        if name.startswith(prefix):
            return False, f"library prefix: {prefix}"
    for prefix in FRAMEWORK_PREFIXES:
        if name.startswith(prefix):
            return False, f"framework plugin prefix: {prefix}"
    if name in KNOWN_NON_CLI:
        return False, "known non-CLI package"
    summary = (meta.get("summary") or "").lower()
    for kw in LIBRARY_SUMMARY_KEYWORDS:
        if kw in summary:
            return False, f"summary contains library keyword: '{kw}'"
    return True, "passed all filters"


def platform_chars(subdirs: list) -> str:
    """Map conda subdirs to compact platform letters (L/M/W) or '?' for noarch-only."""
    chars = ""
    if any(s.startswith("linux") for s in subdirs):
        chars += "L"
    if any(s.startswith("osx") for s in subdirs):
        chars += "M"
    if any(s.startswith("win") for s in subdirs):
        chars += "W"
    return chars if chars else "?"


def build_record(name: str, meta: dict) -> dict:
    """Compact {n, v, t, p} record — schema of the shipped knowledge file."""
    return {
        "n": name,
        "v": meta.get("version") or "",
        "t": meta.get("summary") or "",
        "p": platform_chars(meta.get("subdirs") or []),
    }


def main() -> int:
    ap = argparse.ArgumentParser(description="Refresh bioconda_tools.jsonl")
    ap.add_argument("--out", default=default_out_dir(), help="output dir (default: crates/oxo-flow-ai/src/knowledge/)")
    args = ap.parse_args()
    out_dir = args.out

    log(f"Downloading Bioconda channel metadata from {CHANNEL_DATA_URL} ...")
    raw = http_get(CHANNEL_DATA_URL, timeout=300, retries=3)
    data = json.loads(raw.decode("utf-8"))
    packages = data.get("packages", {})
    log(f"  Retrieved {len(packages)} packages.")

    cli_tools = []
    excluded = 0
    for name in sorted(packages.keys()):
        is_cli, _reason = classify_package(name, packages[name])
        if is_cli:
            cli_tools.append(build_record(name, packages[name]))
        else:
            excluded += 1

    path, count, size = write_jsonl(out_dir, "bioconda_tools.jsonl", cli_tools)
    log(f"Wrote {count} CLI tools to {path} ({size} bytes); excluded {excluded}.")
    update_meta(
        out_dir,
        "bioconda_tools",
        {"count": count, "url": CHANNEL_DATA_URL, "excluded": excluded},
    )
    return 0


if __name__ == "__main__":
    sys.exit(main())
