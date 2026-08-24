#!/usr/bin/env python3
"""Generate an HTML report for gallery 11 (conditional execution).

Reads a VEP-annotated VCF (plain or gzip-compressed) and a qc/ directory
of FastQC/mosdepth outputs, and writes a small self-contained HTML report.
Stdlib only — runs inside gallery envs/report.yaml.
"""

import argparse
import gzip
import html
import os


def open_maybe_gzip(path):
    """Open a file that may be gzip-compressed (VEP output is .vcf.gz)."""
    with open(path, "rb") as fh:
        magic = fh.read(2)
    if magic == b"\x1f\x8b":
        return gzip.open(path, "rt", encoding="utf-8", errors="replace")
    return open(path, "rt", encoding="utf-8", errors="replace")


def vcf_stats(path):
    """Count variants and PASS calls in a VCF."""
    variants = 0
    passing = 0
    with open_maybe_gzip(path) as fh:
        for line in fh:
            if line.startswith("#"):
                continue
            variants += 1
            fields = line.rstrip("\n").split("\t")
            if len(fields) > 6 and fields[6] == "PASS":
                passing += 1
    return variants, passing


def qc_summary(qc_dir):
    """List the QC artifacts (name + size) for the report."""
    rows = []
    if os.path.isdir(qc_dir):
        for name in sorted(os.listdir(qc_dir)):
            path = os.path.join(qc_dir, name)
            if os.path.isfile(path):
                rows.append((name, os.path.getsize(path)))
    return rows


def main():
    ap = argparse.ArgumentParser(description=__doc__)
    ap.add_argument("--vcf", required=True, help="VEP-annotated VCF (may be .vcf.gz)")
    ap.add_argument("--qc", required=True, help="directory with QC artifacts")
    ap.add_argument("--out", required=True, help="output HTML path")
    args = ap.parse_args()

    variants, passing = vcf_stats(args.vcf)
    qc_rows = qc_summary(args.qc)

    body = [
        "<h1>Conditional Workflow Report</h1>",
        "<h2>Variants</h2>",
        f"<p>Total variants: {variants}</p>",
        f"<p>PASS filter: {passing}</p>",
        "<h2>QC artifacts</h2>",
        "<ul>",
    ]
    for name, size in qc_rows:
        body.append(f"<li>{html.escape(name)} ({size} bytes)</li>")
    body.append("</ul>")

    doc = (
        "<!DOCTYPE html>\n<html>\n<head>\n<meta charset='utf-8'>\n"
        "<title>Conditional Workflow Report</title>\n"
        "</head>\n<body>\n"
        + "\n".join(body)
        + "\n</body>\n</html>\n"
    )
    os.makedirs(os.path.dirname(args.out) or ".", exist_ok=True)
    with open(args.out, "w", encoding="utf-8") as fh:
        fh.write(doc)
    print(f"Wrote {args.out} ({variants} variants, {passing} PASS)")


if __name__ == "__main__":
    main()
