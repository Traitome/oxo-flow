"""Shared helpers for the gallery report scripts (stdlib only)."""

import gzip


def open_maybe_gzip(path):
    """Open a file that may be gzip-compressed (VEP output is .vcf.gz)."""
    with open(path, "rb") as fh:
        magic = fh.read(2)
    if magic == b"\x1f\x8b":
        return gzip.open(path, "rt", encoding="utf-8", errors="replace")
    return open(path, "rt", encoding="utf-8", errors="replace")
