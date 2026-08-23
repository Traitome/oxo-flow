#!/usr/bin/env python3
"""Refresh edam_terms.jsonl — EDAM ontology terms used by bio.tools records.

Downloads EDAM_stable.owl from the edamontology GitHub release (via the
releases/latest API, one request, fallback to the stable latest/download
redirect) and extracts the topic_* and operation_* classes:

    {"uri": "topic_XXXX", "label": "...", "definition": "..."}

The OWL is RDF/XML; owl:Class elements with an rdf:about of the form
http://edamontology.org/{topic|operation}_XXXX are kept, deprecated classes
(rdfs:label containing "deprecated" or an owl:deprecated literal) are
skipped.

EDAM is published under CC BY-SA 4.0 — the output file carries an
attribution header (lines starting with '#', which the Rust loader's
JSONL filter drops), and the license is recorded in knowledge_meta.json.

Idempotent: re-running replaces the output file atomically.
"""

import argparse
import json
import os
import sys
import xml.etree.ElementTree as ET

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from common import default_out_dir, http_get, http_get_json, log, update_meta, write_text_atomic  # noqa: E402

RELEASES_API = "https://api.github.com/repos/edamontology/edamontology/releases/latest"
STABLE_URL = "https://github.com/edamontology/edamontology/releases/latest/download/EDAM.owl"
SOURCE_URL = "https://github.com/edamontology/edamontology"
LICENSE_NOTE = "CC BY-SA 4.0 (https://creativecommons.org/licenses/by-sa/4.0/)"

HEADER = [
    "# EDAM ontology terms (topic_* and operation_* classes)",
    f"# Source: {SOURCE_URL}",
    f"# License: {LICENSE_NOTE}",
    "# Data extracted from EDAM_stable.owl",
]

# Local-name matching: RDF/XML namespaces vary between EDAM releases.
RDF_ABOUT = "{http://www.w3.org/1999/02/22-rdf-syntax-ns#}about"


def local(tag: str) -> str:
    return tag.rsplit("}", 1)[-1]


def fetch_owl() -> bytes:
    """Download EDAM_stable.owl via the GitHub release API, then the
    stable redirect as a fallback."""
    try:
        rel = http_get_json(RELEASES_API, timeout=60, retries=2,
                            headers={"Accept": "application/vnd.github+json"})
        for asset in rel.get("assets", []):
            name = asset.get("name", "")
            if name.endswith(".owl") and ("EDAM" in name or name == "EDAM.owl"):
                url = asset.get("browser_download_url")
                if url:
                    log(f"Downloading {name} from release asset ...")
                    return http_get(url, timeout=300, retries=3)
        tag = rel.get("tag_name", "")
        log(f"Release {tag!r} has no matching OWL asset — trying stable redirect.")
    except Exception as e:
        log(f"WARNING: releases API failed ({e}) — trying stable redirect.")
    log(f"Downloading EDAM_stable.owl from the stable release redirect ...")
    return http_get(STABLE_URL, timeout=300, retries=3)


def parse_owl(raw: bytes) -> list[dict]:
    """Extract {uri, label, definition} for topic_/operation_ classes."""
    root = ET.fromstring(raw)
    terms = []
    for child in root:
        if local(child.tag) != "Class":
            continue
        about = child.get(RDF_ABOUT, "")
        id_part = about.rsplit("/", 1)[-1]
        if not (id_part.startswith("topic_") or id_part.startswith("operation_")):
            continue
        label = ""
        definition = ""
        deprecated = False
        for sub in child:
            name = local(sub.tag)
            text = (sub.text or "").strip()
            if name == "label" and text:
                label = text
            elif name == "hasDefinition" and text:
                definition = text
            elif name == "deprecated":
                if text.lower() in ("true", "1"):
                    deprecated = True
                # rdf:resource="true" form
                if sub.get(RDF_ABOUT, "").lower().endswith("true"):
                    deprecated = True
        if not label or deprecated:
            continue
        terms.append({"uri": id_part, "label": label, "definition": definition})
    return terms


def main() -> int:
    ap = argparse.ArgumentParser(description="Refresh edam_terms.jsonl from the EDAM ontology")
    ap.add_argument("--out", default=default_out_dir(), help="output dir (default: crates/oxo-flow-ai/src/knowledge/)")
    args = ap.parse_args()
    out_dir = args.out

    raw = fetch_owl()
    log(f"Downloaded {len(raw)} bytes of OWL.")
    terms = parse_owl(raw)
    terms.sort(key=lambda t: t["uri"])
    log(f"Extracted {len(terms)} topic_/operation_ classes.")

    path = os.path.join(out_dir, "edam_terms.jsonl")
    lines = list(HEADER) + [
        json.dumps(t, ensure_ascii=False, separators=(",", ":")) for t in terms
    ]
    write_text_atomic(path, "\n".join(lines) + "\n")
    size = os.path.getsize(path)
    # The meta contract counts non-empty lines (the drift guard checks it),
    # which includes the '#' attribution header lines.
    non_empty = sum(1 for line in lines if line.strip())
    log(f"Wrote {len(terms)} EDAM terms to {path} ({size} bytes).")
    update_meta(out_dir, "edam_terms",
                {"count": non_empty, "records": len(terms),
                 "url": SOURCE_URL, "license": LICENSE_NOTE})
    return 0


if __name__ == "__main__":
    sys.exit(main())
