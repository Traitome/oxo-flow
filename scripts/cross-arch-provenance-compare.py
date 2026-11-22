#!/usr/bin/env python3
"""Compare two cross-arch provenance arms and emit a machine-readable verdict.

Reads each arm's checkpoint.json (top-level `checksums` = recorded output
SHA-256 of every declared output) plus container.txt/identity.txt, then reports,
per artifact, whether the recorded checksum is byte-identical across the two
arms, and a per-artifact-CLASS verdict (architecture-stable = every artifact in
the class identical).

Usage: cross-arch-provenance-compare.py <arm-a-dir> <arm-b-dir>
"""
import json
import re
import sys

# Fixture outputs -> the artifact class the preregistration (§C4) partitions on.
ARTIFACT_CLASS = {
    "data/payload.txt": "pure-text",
    "summary.txt": "derived-text",
    "manifest.sha256": "checksum-manifest",
    "data/payload.txt.gz": "byte-exact-compressed",
    "data/bytes.bin.gz": "byte-exact-compressed",
    "data/bytes.bin": "raw-binary",
    "float.txt": "ieee754-float",
    "float_series.txt": "ieee754-float",
}


def recorded(d):
    """Map output path -> recorded sha256 from a checkpoint.json `checksums`."""
    ck = json.load(open(f"{d}/checkpoint.json"))
    out = {f.lstrip("./"): h for f, h in ck.get("checksums", {}).items()}
    if not out:  # fall back to verify.json expected (== recorded at run time)
        v = json.load(open(f"{d}/verify.json"))["verify"]
        out = {e["file"]: e["expected"] for e in v["entries"]}
    return out


def rule_fingerprints(d):
    ck = json.load(open(f"{d}/checkpoint.json"))
    return ck.get("rule_fingerprints", {})


def kv(path):
    d = {}
    for line in open(path):
        m = re.match(r"^([a-z0-9._-]+):\s*(.*)$", line.rstrip("\n"))
        if m:
            d[m.group(1)] = m.group(2)
    return d


def main():
    adir, bdir = sys.argv[1], sys.argv[2]
    a, b = recorded(adir), recorded(bdir)
    files = sorted(set(a) | set(b))
    arts = []
    for f in files:
        ha, hb = a.get(f), b.get(f)
        arts.append({
            "file": f,
            "class": ARTIFACT_CLASS.get(f, "unclassified"),
            "arm_a_sha256": ha,
            "arm_b_sha256": hb,
            "identical": ha == hb and ha is not None,
        })
    classes = {}
    for art in arts:
        c = classes.setdefault(art["class"], {"total": 0, "identical": 0, "verdict_arch_stable": None})
        c["total"] += 1
        c["identical"] += 1 if art["identical"] else 0
    for c in classes.values():
        c["verdict_arch_stable"] = c["identical"] == c["total"]

    idn = sum(1 for x in arts if x["identical"])
    res = {
        "issue": "1166",
        "method": "container-digest cross-architecture provenance comparison",
        "arms": {
            "a": {"label": "linux/amd64 (native)",
                  "identity": kv(f"{adir}/identity.txt"),
                  "container": kv(f"{adir}/container.txt"),
                  "rule_fingerprints": rule_fingerprints(adir)},
            "b": {"label": "linux/arm64 (qemu-emulated)",
                  "identity": kv(f"{bdir}/identity.txt"),
                  "container": kv(f"{bdir}/container.txt"),
                  "rule_fingerprints": rule_fingerprints(bdir)},
        },
        "artifacts_total": len(arts),
        "artifacts_identical": idn,
        "artifacts_differing": len(arts) - idn,
        "differing_set": [x["file"] for x in arts if not x["identical"]],
        "classes": classes,
        "artifacts": arts,
    }
    json.dump(res, sys.stdout, indent=2, sort_keys=True)
    sys.stdout.write("\n")


if __name__ == "__main__":
    main()
