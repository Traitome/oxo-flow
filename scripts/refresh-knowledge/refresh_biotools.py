#!/usr/bin/env python3
"""Refresh biotools_overlay.jsonl — bio.tools metadata for the tools that
Bioconda/nf-core already cover.

The bio.tools API has no bulk dump and its default ordering is not
meaningful, so the crawl is a bounded paginated walk:

  - 50 records per page, 1 request per second (their documented rate limit)
  - target names come from the out dir's bioconda_tools.jsonl and
    nfcore_modules.jsonl (lower-cased intersection join)
  - stops early once every target name has been seen; otherwise it stops at
    --max-pages (default 666 = the full corpus) or when the accumulated
    output exceeds --size-cap

Output (JSONL):

    {"n", "description", "license", "homepage", "topic": [uris],
     "operation": [uris], "doi": [dois]}

  - license = license identifier string when present ("" otherwise)
  - topic/operation = list of EDAM URI ids ("topic_XXXX"/"operation_XXXX")
  - doi = publication DOI list

Idempotent: re-running replaces the output file atomically.
"""

import argparse
import json
import os
import sys
import time

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from common import default_out_dir, http_get, http_get_json, log, update_meta, write_jsonl  # noqa: E402

API_URL = "https://bio.tools/api/tool/"
PAGE_SIZE = 50
MAX_PAGES = 666  # 33273 records / 50 = ~666 pages
SIZE_CAP = 4 * 1024 * 1024  # biotools_overlay.jsonl cap (~4 MiB)
DELAY = 1.0  # 1 request / second

TARGET_FILES = ("bioconda_tools.jsonl", "nfcore_modules.jsonl")


def load_targets(out_dir: str) -> set[str]:
    """Lower-cased names from the sibling knowledge files."""
    targets: set[str] = set()
    for name in TARGET_FILES:
        path = os.path.join(out_dir, name)
        if not os.path.exists(path):
            log(f"  WARNING: {path} not found — no targets from this file")
            continue
        with open(path, encoding="utf-8") as f:
            for line in f:
                line = line.strip()
                if not line or line.startswith("#"):
                    continue
                try:
                    rec = json.loads(line)
                except json.JSONDecodeError:
                    continue
                n = rec.get("n")
                if n:
                    targets.add(n.lower())
    return targets


def license_str(license_val) -> str:
    """bio.tools 'license' may be a string or a list of strings/objects."""
    if license_val is None:
        return ""
    if isinstance(license_val, str):
        return license_val
    if isinstance(license_val, list):
        parts = []
        for item in license_val:
            if isinstance(item, str):
                parts.append(item)
            elif isinstance(item, dict):
                url = item.get("url") or item.get("name") or ""
                if url:
                    parts.append(url)
        return ", ".join(parts)
    return ""


def uri_ids(items) -> list:
    """Extract EDAM uri ids from a topic[]/operation[] list."""
    out = []
    for item in items or []:
        uri = item.get("uri", "") if isinstance(item, dict) else ""
        if uri:
            out.append(uri.rsplit("/", 1)[-1])
    return out


def dois_of(pubs) -> list:
    """Extract DOI strings from a publication[] list."""
    out = []
    for pub in pubs or []:
        doi = pub.get("doi", "") if isinstance(pub, dict) else ""
        if doi:
            out.append(doi)
    return out


def build_record(tool: dict) -> dict:
    """Compact {n, description, license, homepage, topic, operation, doi} record."""
    return {
        "n": str(tool.get("name") or ""),
        "description": str(tool.get("description") or ""),
        "license": license_str(tool.get("license")),
        "homepage": str(tool.get("homepage") or ""),
        "topic": uri_ids(tool.get("topic")),
        "operation": uri_ids(tool.get("operation")),
        "doi": dois_of(tool.get("publication")),
    }


def main() -> int:
    ap = argparse.ArgumentParser(description="Refresh biotools_overlay.jsonl from bio.tools")
    ap.add_argument("--out", default=default_out_dir(), help="output dir (default: crates/oxo-flow-ai/src/knowledge/)")
    ap.add_argument("--max-pages", type=int, default=MAX_PAGES, help="hard bound on crawled pages")
    ap.add_argument("--size-cap", type=int, default=SIZE_CAP, help="byte cap for the output file")
    ap.add_argument("--no-delay", action="store_true", help="disable the 1 req/sec delay (debug only)")
    args = ap.parse_args()
    out_dir = args.out

    targets = load_targets(out_dir)
    log(f"Loaded {len(targets)} target names from {TARGET_FILES}.")

    rows: list[dict] = []
    seen_names: set[str] = set()
    found = 0
    bytes_accumulated = 0
    errors = 0
    stop_reason = "max-pages reached"

    for page in range(1, args.max_pages + 1):
        url = f"{API_URL}?page={page}&count={PAGE_SIZE}"
        try:
            # The API serves JSON only when asked for it; without the Accept
            # header it answers with the JS frontend HTML shell.
            data = http_get_json(url, timeout=60, retries=2, accept="application/json")
        except Exception as e:
            errors += 1
            log(f"  WARNING: page {page} failed ({e}); error count {errors}")
            if errors >= 5:
                stop_reason = f"too many consecutive errors ({errors})"
                break
            time.sleep(DELAY * 3)
            continue
        errors = 0
        records = data.get("list", [])
        total = data.get("count", 0)
        if page == 1:
            log(f"bio.tools reports {total} records ({PAGE_SIZE}/page, ~{total // PAGE_SIZE + 1} pages).")
        for tool in records:
            name = str(tool.get("name") or "")
            if not name:
                continue
            key = name.lower()
            if key in seen_names:
                continue
            seen_names.add(key)
            if key in targets:
                rec = build_record(tool)
                row = json.dumps(rec, ensure_ascii=False, separators=(",", ":"))
                bytes_accumulated += len(row) + 1
                if bytes_accumulated > args.size_cap:
                    stop_reason = "output size cap reached"
                    break
                rows.append(rec)
                found += 1
        if page % 50 == 0:
            log(f"  page {page}/{args.max_pages}: {found} matching tools found so far")
        if stop_reason == "output size cap reached":
            break
        # Early stop: every target name has been seen (matched or known-absent).
        if seen_names >= targets:
            stop_reason = "all target names seen"
            break
        if args.no_delay:
            time.sleep(0.1)
        else:
            time.sleep(DELAY)

    rows.sort(key=lambda r: r["n"])
    path, count, size = write_jsonl(out_dir, "biotools_overlay.jsonl", rows)
    log(f"Wrote {count} biotools overlay records to {path} ({size} bytes).")
    log(f"  Crawl ended: {stop_reason}; {len(seen_names)} names seen, {len(targets) - len(seen_names)} targets never seen.")
    update_meta(out_dir, "biotools_overlay",
                {"count": count, "url": API_URL, "targets": len(targets),
                 "seen": len(seen_names), "stop_reason": stop_reason})
    return 0


if __name__ == "__main__":
    sys.exit(main())
