#!/usr/bin/env python3
"""Generate a clinical-style variant report for galleries 14 and 15.

Reads a VEP-annotated VCF (plain or gzip-compressed) and writes an HTML
report summarizing variant counts, PASS rate, and consequence classes,
with the top high/moderate-impact variants listed. Stdlib only — runs
inside gallery envs/report.yaml.
"""

import argparse
import gzip
import html
import os
from collections import Counter

# IMPACT ordering used to keep the most severe transcript per variant.
IMPACT_RANK = {"HIGH": 0, "MODERATE": 1, "LOW": 2, "MODIFIER": 3}


def open_maybe_gzip(path):
    """Open a file that may be gzip-compressed (VEP output is .vcf.gz)."""
    with open(path, "rb") as fh:
        magic = fh.read(2)
    if magic == b"\x1f\x8b":
        return gzip.open(path, "rt", encoding="utf-8", errors="replace")
    return open(path, "rt", encoding="utf-8", errors="replace")


def csq_columns(header_lines):
    """Find the Consequence/IMPACT/SYMBOL positions in the CSQ format.

    VEP defines the CSQ layout inside the ##INFO Description string
    ("... Format: Allele|Consequence|IMPACT|SYMBOL|..."); parsing it
    (instead of assuming Consequence is first) keeps the report correct
    across VEP versions and `--fields` overrides.
    """
    consequence = impact = symbol = None
    for line in header_lines:
        if line.startswith("##INFO=<ID=CSQ,"):
            marker = "Format:"
            idx = line.find(marker)
            if idx == -1:
                continue
            rest = line[idx + len(marker):].split('"', 1)[0].strip()
            fields = rest.split("|")
            consequence = fields.index("Consequence") if "Consequence" in fields else 0
            impact = fields.index("IMPACT") if "IMPACT" in fields else None
            symbol = fields.index("SYMBOL") if "SYMBOL" in fields else None
    return consequence, impact, symbol


def worst_csq(csq_value, consequence_col, impact_col, symbol_col):
    """Pick the most severe transcript's annotations from a CSQ= value."""
    consequence_col = consequence_col or 0
    impact_col = impact_col or 2
    symbol_col = symbol_col or 1
    best = None
    for record in csq_value.split(","):
        fields = record.split("|")
        impact = fields[impact_col] if impact_col is not None else "MODIFIER"
        if best is None or IMPACT_RANK.get(impact, 3) < IMPACT_RANK[best[1]]:
            best = (
                fields[consequence_col] if consequence_col < len(fields) else "unknown",
                impact,
                fields[symbol_col] if symbol_col is not None and symbol_col < len(fields) else "",
            )
    return best or ("unknown", "MODIFIER", "")


def parse_vcf(path):
    """Return (variants, passing, impact counter, top variants)."""
    variants = 0
    passing = 0
    impacts = Counter()
    top = []

    header_lines = []
    with open_maybe_gzip(path) as fh:
        lines = fh
        for line in lines:
            if line.startswith("##"):
                header_lines.append(line.rstrip("\n"))
                continue
            if line.startswith("#CHROM"):
                break
        consequence_col, impact_col, symbol_col = csq_columns(header_lines)

        for line in lines:
            variants += 1
            fields = line.rstrip("\n").split("\t")
            if len(fields) < 8:
                continue
            chrom, pos, _, ref, alt, _, filt, info = fields[:8]
            if filt == "PASS":
                passing += 1

            consequence = impact = "MODIFIER"
            symbol = ""
            for entry in info.split(";"):
                if entry.startswith("CSQ="):
                    consequence, impact, symbol = worst_csq(
                        entry[len("CSQ="):], consequence_col, impact_col, symbol_col
                    )
                    break
            impacts[impact] += 1
            if impact in ("HIGH", "MODERATE") and len(top) < 20:
                top.append(
                    {
                        "chrom": chrom,
                        "pos": pos,
                        "ref": ref,
                        "alt": alt,
                        "gene": symbol,
                        "consequence": consequence,
                        "impact": impact,
                    }
                )
    return variants, passing, impacts, top


def render_html(sample_label, stats, output_path):
    variants, passing, impacts, top = stats
    rows = []
    for v in top:
        rows.append(
            "<tr>"
            f"<td>{html.escape(v['chrom'])}:{html.escape(v['pos'])}</td>"
            f"<td>{html.escape(v['ref'])} &rarr; {html.escape(v['alt'])}</td>"
            f"<td>{html.escape(v['gene'] or '-')}</td>"
            f"<td>{html.escape(v['consequence'])}</td>"
            f"<td>{html.escape(v['impact'])}</td>"
            "</tr>"
        )

    impact_rows = "".join(
        f"<tr><td>{impact}</td><td>{count}</td></tr>"
        for impact, count in sorted(impacts.items(), key=lambda kv: IMPACT_RANK.get(kv[0], 9))
    )

    doc = f"""<!DOCTYPE html>
<html>
<head>
<meta charset='utf-8'>
<title>{html.escape(sample_label)} — Variant Report</title>
<style>
body {{ font-family: sans-serif; margin: 2em; }}
table {{ border-collapse: collapse; }}
td, th {{ border: 1px solid #ccc; padding: 0.4em 0.8em; text-align: left; }}
</style>
</head>
<body>
<h1>{html.escape(sample_label)} — Variant Report</h1>
<h2>Summary</h2>
<p>Total variants: {variants}</p>
<p>PASS filter: {passing}</p>
<h2>Impact classes</h2>
<table>
<tr><th>IMPACT</th><th>Variants</th></tr>
{impact_rows}
</table>
<h2>Top high/moderate-impact variants</h2>
<table>
<tr><th>Locus</th><th>Change</th><th>Gene</th><th>Consequence</th><th>IMPACT</th></tr>
{''.join(rows) or '<tr><td colspan="5">none</td></tr>'}
</table>
</body>
</html>
"""
    os.makedirs(os.path.dirname(output_path) or ".", exist_ok=True)
    with open(output_path, "w", encoding="utf-8") as fh:
        fh.write(doc)
    print(f"Wrote {output_path} ({variants} variants, {passing} PASS)")


def main():
    ap = argparse.ArgumentParser(description=__doc__)
    ap.add_argument("--input", required=True, help="VEP-annotated VCF (may be .vcf.gz)")
    ap.add_argument("--output", required=True, help="output HTML path")
    label = ap.add_mutually_exclusive_group()
    label.add_argument("--sample", help="sample name (gallery 14)")
    label.add_argument("--pair", help="experiment-control pair id (gallery 15)")
    args = ap.parse_args()

    sample_label = args.sample or args.pair or os.path.basename(args.input)
    render_html(sample_label, parse_vcf(args.input), args.output)


if __name__ == "__main__":
    main()
