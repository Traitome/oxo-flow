#!/usr/bin/env python3
"""Provenance manifest for publication-facing artifact regeneration (#1305).

Ledger §7 wave 5 and §8 Change Discipline require every benchmark artifact to
record command, commit, host, software versions, and inputs. This module emits
a key-ordered (order-independent) JSON manifest for the publication-facing
status reports under ``docs/superpowers/plans/`` and provides a fail-closed
checker.

Honesty note: the committed status reports are hand-authored documents that
embed measured rows; the generators in ``eval/benchmarks`` and
``eval/frontier`` emit the raw measurement *tables*. Neither regenerates the
committed reports byte-for-byte, and this tool does not claim that. It records
the true provenance of the measured numbers that appear in the reports, and —
since the #1305 closure amendment — the generator-run provenance of the three
non-measured reports too (their generator outputs are snapshot-committed under
``eval/benchmarks/snapshots/`` and drift-gated by ``make reports-regen``).

Usage:
  python3 eval/benchmarks/provenance_manifest.py --emit eval/benchmarks/provenance_manifest.json
  python3 eval/benchmarks/provenance_manifest.py --check
"""

from __future__ import annotations

import argparse
import json
import os
import re
import sys

REPO_ROOT = os.path.abspath(os.path.join(os.path.dirname(__file__), "..", ".."))
DEFAULT_MANIFEST = os.path.join(REPO_ROOT, "eval", "benchmarks", "provenance_manifest.json")

# Discovery set: EVERY markdown document under the plans directory is scanned,
# not just those named "*-report.md" (review P2 residual, PR #1307). Gating on
# the filename suffix was a real hole: a plan doc
# ("industrial-scientific-implementation-plan.md") carried measured campaign
# numbers ("7/10 proposals, 3/10 applied, ...") yet was never scanned, and the
# comment that "plan docs carry no publication measurements" was falsified by
# that very row. The rule is now: a measured number anywhere under
# docs/superpowers/plans/*.md must have a provenance entry; the manifest is the
# record, not the gate.
#
# DESIGN_DOC_OPTOUT is the narrow, explicit exception list. Each entry is a
# design/protocol/runbook document reviewed to carry NO publication measurement;
# it is excluded so the fail-closed bare-integer table rule (which fires on
# success criteria like "`make ci` exits 0") does not force a bogus provenance
# entry onto a doc that has nothing to attribute. This is an allow-list of
# exceptions by decision, never a broad "design docs are skipped" category --
# any other plans doc that gains a measured number is gated automatically.
PLANS_DIR = os.path.join(REPO_ROOT, "docs", "superpowers", "plans")
MARKDOWN_SUFFIX = ".md"
DESIGN_DOC_OPTOUT = frozenset(
    {
        # Validation runbook: its only "measured" hits are the fail-closed
        # bare-integer table rule firing on target counts / success criteria
        # (e.g. "`make ci` exits 0"), which are acceptance criteria, not
        # measurements.
        "docs/superpowers/plans/linux-server-validation-protocol.md",
    }
)

REQUIRED_ENTRY_KEYS = (
    "source_commit",
    "generator_command",
    "host",
    "tool_versions",
    "inputs",
)

COMMIT_RE = re.compile(r"^[0-9a-f]{7,40}$")
SHA256_RE = re.compile(r"^[0-9a-f]{64}$")
_SEPARATOR_CELL_RE = re.compile(r"^:?-{3,}:?$")

# Measured-number detection rule (review P2-2; documented so it is auditable).
#
# A number counts as a *measurement* when it is one of:
#   * a percentage            e.g. "99.9%", "12 %"
#   * a fraction              e.g. "7/10", "3 / 10", "8/9 applied"
#   * a word percent          e.g. "49 percent", "12 pct"
#   * scientific notation     e.g. "1.2e-3", "2E6", "5e-2"
#   * a decimal with a        e.g. "118.69 ms", "9.50", "3159.5 nodes/sec"
#     fractional part
# These are detected anywhere in a document (prose or table cell), so prose
# statements like "latency was 118.69 ms", "coverage reached 99.9%", or "7/10
# cases" are caught.
#
# A *plain integer* (e.g. "30") is treated as a measurement only inside a
# markdown table data row, never in prose -- prose integers are far more often
# issue numbers, years, or section numbers ("issue #1305", "Edition 2024",
# "section 2") than measurements.
#
# Dotted version / section strings are deliberately NOT measurements: a
# two-or-more-segment dotted identifier ("26.10.6", "1.98.0", "3.12.4",
# "127.0.0.1", "v1.2", "section 1.4", "WCAG 2.1", "### Task 2.1") must not be
# read as a decimal. Two guards enforce this:
#   1. the decimal / fraction patterns are anchored to reject a leading or
#      trailing dot and slash, so the inner "10.6" / "12.4" fragments of a
#      multi-segment string never match; and
#   2. a real two-segment decimal still needs to be told from "v1.2" / "1.4"
#      context, so a candidate is ignored when a version / section / figure
#      keyword (including abbreviated forms with or without a trailing period,
#      e.g. "Fig.", "Sec.", "pp.", "v.") immediately precedes it and the token
#      itself is short on both sides (see _DOTTED_CONTEXT_KEYWORDS and
#      _preceded_by_dotted_keyword).
# Numbers inside fenced code blocks and inline `code` spans are stripped first:
# design docs quote snippets ($k \in \{1, 3, 5\}$, `wall_time_secs 1.5`) that
# are not measurements.
_PERCENT_RE = re.compile(r"-?\d+(?:\.\d+)?\s?%")
_FRACTION_RE = re.compile(r"(?<![\d./:])-?\d+\s?/\s?\d+(?![\d/])")
_WORD_PERCENT_RE = re.compile(r"\b\d+(?:\.\d+)?\s?(?:percent|pct)\b", re.IGNORECASE)
_SCI_RE = re.compile(r"-?\d+(?:\.\d+)?[eE][+-]?\d+")
_DECIMAL_RE = re.compile(r"(?<![\d./])-?\d+\.\d+(?![\d./])")
_INTEGER_RE = re.compile(r"^-?\d+$")

# Keyword immediately preceding a dotted "x.y" that marks it as an
# identifier/version/section/standard token rather than a measurement.
_DOTTED_CONTEXT_KEYWORDS = frozenset(
    {
        "v", "ver", "version", "sec", "secs", "section", "ch", "chapter",
        "fig", "figs", "figure", "tab", "table", "step", "task", "phase",
        "milestone", "part", "appendix", "annex", "edition", "clause", "rfc",
        "iso", "iec", "no", "article", "p", "pp", "eq", "ref", "wcag",
        "snakemake", "nextflow", "§",
    }
)
# Statistical / measurement keywords (round-6 P3, PR #1307): these name a
# quantity, never a location or version, so they must NEVER trigger the
# zero-major ("0.x") reference suppression -- "p 0.5" and "p 0.9" are genuine
# readings, while "version 0.2" / "section 0.1" stay suppressed.
_MEASUREMENT_KEYWORDS = frozenset(
    {
        "p", "prob", "alpha", "beta", "power", "q", "fdr", "padj", "auc",
        "r", "r2", "accuracy", "precision", "recall", "f1", "score", "rate",
        "ratio",
    }
)
# Abbreviated reference keywords (round-6 P2, PR #1307): a trailing period is
# accepted as reference context ONLY for these curated abbreviations ("Fig.",
# "Sec.", "pp.", "v."). A full lowercase word followed by "." is a SENTENCE
# boundary, not a reference ("... the version. 1.5 participants" is a
# measurement), so it must not be suppressed.
#
# Known lexical limitation (unfixable without parsing prose): a few entries
# are ALSO ordinary English words -- "no" and "ref" -- so the sentence "The
# answer was no. 3.5 samples" stays suppressed (read as the abbreviation
# "No. 3.5"). Kept because "No. 3.1"/"Ref. 2.4" are the far more common
# spelling in these design docs; the ambiguity is rare and fails closed in the
# suppression direction only for those two tokens.
_ABBREVIATION_KEYWORDS = frozenset(
    {
        "v", "ver", "sec", "secs", "fig", "figs", "tab", "ch", "no", "pp",
        "eq", "ref",
    }
)
# Units that, sitting immediately after a reference-like number, cancel the
# "this is a section/figure reference" suppression: "1.4 ms" is a measurement,
# not "section 1.4". Used only to refine the dotted-keyword guard, never to
# widen the real-decimal patterns.
_DECIMAL_UNITS = frozenset(
    {
        "s", "ms", "us", "ns", "sec", "secs", "second", "seconds",
        "min", "mins", "minute", "minutes", "h", "hr", "hrs", "hour",
        "hours", "day", "days", "kb", "mb", "gb", "tb", "kib", "mib",
        "gib", "byte", "bytes", "b", "px", "x", "rps", "qps", "tps",
        "fps", "node", "nodes", "core", "cores", "cpu", "cpus",
        "percent", "pct",
    }
)
# Fenced code (```...```) and inline code (`...`) are stripped before scanning.
_FENCE_RE = re.compile(r"```.*?```", re.DOTALL)
_INLINE_CODE_RE = re.compile(r"`[^`]*`")


def _strip_code(text: str) -> str:
    """Remove fenced code blocks and inline code spans (not prose)."""
    return _INLINE_CODE_RE.sub(" ", _FENCE_RE.sub(" ", text))


def _preceded_by_dotted_keyword(line: str, start: int, end: int) -> bool:
    """True when ``line[start:end]`` IS a version/section/standard reference.

    A *reference* is a two-segment dotted id ("X.Y") that sits immediately after
    a section / figure / version keyword -- the full word or an abbreviated form
    with a trailing period ("Fig. 4.2", "Sec. 2.1", "pp. 12.3", "Ch. 5.1",
    "v. 2.0", "No. 3.1"). Such tokens name a location or a version and are never
    measurements, so the caller ignores them *even when a comma or a following
    clause is present* ("see section 1.4, which describes it"). The round-4
    guard only suppressed whitespace-separated, non-zero-major, no-comma forms,
    so "Fig. 4.2", "v0.8", "version 0.2" and "see section 1.4, ..." leaked into
    the measured set (round-5 P3 regression, PR #1307).

    The suppression stays narrow (review P3-2, PR #1307) so it cannot re-hide a
    real measurement:

      * the token must be a plain two-segment "X.Y" id for the decimal case --
        a 3+ segment dotted string ("3.12.4") is already rejected by
        ``_DECIMAL_RE`` before the guard runs; a fraction candidate ("4/5") is
        a reference only for the traditional "Step 4/5" spelling, while a real
        fraction measurement ("7/10 proposals") stays detected;
      * its fraction must be a short 1-2 digit suffix -- "118.69" (2 digits with
        a unit) and "3.125" (3 digits) are measurements, and a trailing unit
        token ("3.1 ms", "3.12 us") always cancels the reference reading;
      * a zero-major id ("0.x") counts as a reference only when its fraction is
        a single digit -- the version/section forms ("v 0.8", "version 0.2",
        "section 0.1") are refs, but a two-digit leading-zero fraction is the
        conventional probability/measurement spelling ("p 0.05"); and
      * only whitespace may separate the number from the keyword, so
        "table 3, 118.69" and "phase 3, 3.12" do NOT count.

    Allows "#", "##", "###" markdown heading markers as section context so that
    headings like "### Task 1.1" / "### 1.1 Title" are not read as decimals.
    """
    matched = line[start:end]
    after = line[end : end + 8]
    if _DECIMAL_RE.fullmatch(matched):
        # A dotted decimal candidate: it is a reference only when it is a plain
        # two-segment "X.Y" id. A long fraction ("3.125", "118.69") is not a
        # reference sub-part, and a zero-major id ("0.x") is a reference only
        # for a single-digit fraction (a two-digit leading-zero fraction such
        # as "0.05" is the conventional measurement spelling).
        major, frac = matched.split(".", 1)
        if len(frac) > 2:
            return False
        if major.lstrip("-") == "0" and len(frac) != 1:
            return False
    else:
        # A fraction ("4/5", "7/10") or any other match shape. Keep the round-4
        # rule so a step / part reference ("Step 4/5") stays suppressed while a
        # real fraction measurement ("7/10 proposals") is detected.
        integer_match = re.match(r"-?\d+", matched)
        if integer_match and integer_match.group(0).lstrip("-") == "0":
            return False
        if after.startswith(","):
            return False
    unit_match = re.match(r"\s*([A-Za-z]+)", after)
    if unit_match and unit_match.group(1).lower() in _DECIMAL_UNITS:
        return False
    prefix = line[max(0, start - 24) : start]
    # A section RANGE ("3.1-3.7" / "3.1–3.7") continues the same reference,
    # so walk back over a dash-joined preceding number before looking for the
    # keyword. A comma/space-separated separate number ("table 3, 118.69") is
    # NOT a range and is deliberately not crossed here.
    while True:
        head = prefix.rstrip(" \t")
        stripped = head.rstrip("-–—~")
        if stripped == head:
            break
        range_match = re.search(r"\d+(?:\.\d+)*$", stripped)
        if not range_match:
            break
        prefix = stripped[: range_match.start()]
    head = prefix.rstrip(" \t")
    # The keyword may carry a trailing period ("Fig.", "Sec.", "pp.", "v."). That
    # period is only reference context for a CURATED ABBREVIATION (round-6 P2):
    # a full lowercase word ending a sentence ("... the version. 1.5", "...
    # the table. 4.2") has a SENTENCE-ending period, not a reference, and must
    # not suppress the following measurement.
    match = re.search(r"([A-Za-z§#]+)(\.?)$", head)
    if not match:
        return False
    token = match.group(1).lower()
    if set(token) <= {"#"}:
        return True
    if match.group(2) == "." and token not in _ABBREVIATION_KEYWORDS:
        return False
    # Measurement / statistical keywords ("p", "r", "q", ...) name a quantity,
    # never a location or version, so they never suppress a reading (round-6
    # P3): "p 0.5" / "p 0.9" are genuine readings, while the reference-noun
    # keywords ("version 0.2", "section 0.1", "v0.8") still suppress.
    if token in _MEASUREMENT_KEYWORDS:
        return False
    return token in _DOTTED_CONTEXT_KEYWORDS


def _has_measured_number(text: str) -> bool:
    """True when ``text`` carries a percent / fraction / word-percent / scientific / decimal measurement."""
    if (
        _PERCENT_RE.search(text)
        or _WORD_PERCENT_RE.search(text)
        or _SCI_RE.search(text)
    ):
        return True
    for match in _FRACTION_RE.finditer(text):
        if not _preceded_by_dotted_keyword(text, match.start(), match.end()):
            return True
    for match in _DECIMAL_RE.finditer(text):
        if not _preceded_by_dotted_keyword(text, match.start(), match.end()):
            return True
    return False


def _is_measured_table_row(line: str) -> bool:
    """True when a markdown table row carries at least one numeric data cell.

    Header rows and ``| :---: |`` separator rows are excluded so that a table
    with no measured values does not require a provenance entry. A data row is
    "measured" when any cell holds a percent / scientific / decimal number or a
    bare integer (the historical, fail-closed rule).
    """
    stripped = line.strip()
    if not stripped.startswith("|"):
        return False
    cells = [c.strip() for c in stripped.strip("|").split("|")]
    if not cells:
        return False
    if all(_SEPARATOR_CELL_RE.match(c) for c in cells):
        return False
    for cell in cells:
        if _has_measured_number(cell):
            return True
        for piece in cell.replace(",", "").replace("+/-", " ").split():
            if _INTEGER_RE.match(piece):
                return True
    return False


def report_has_measured_number(text: str) -> bool:
    """Return True when a report body contains a measured number.

    Detects a measured markdown table data row, or a percent / fraction /
    word-percent / scientific / decimal measurement in prose (review P2-2).
    Fenced code blocks and inline code spans are stripped first so quoted
    snippets are not mistaken for measurements.
    """
    for line in _strip_code(text).splitlines():
        if _is_measured_table_row(line):
            return True
        if line.strip().startswith("|"):
            continue
        if _has_measured_number(line):
            return True
    return False


def _validate_entry(rel_path: str, entry: object, errors: list) -> None:
    if not isinstance(entry, dict):
        errors.append(f"{rel_path}: entry is not a JSON object")
        return
    for key in REQUIRED_ENTRY_KEYS:
        if key not in entry:
            errors.append(f"{rel_path}: missing required key '{key}'")
    # 'note' is optional but, when present, must be a non-empty string (review
    # P3): it carries the free-text honesty caveat (e.g. which figure is
    # UNPINNED), so an empty or non-string value is a malformed entry, not a
    # silent no-op.
    if "note" in entry:
        note = entry.get("note")
        if not isinstance(note, str) or not note.strip():
            errors.append(f"{rel_path}: optional 'note' must be a non-empty string")
    commit = entry.get("source_commit")
    if not isinstance(commit, str) or not COMMIT_RE.match(commit):
        errors.append(f"{rel_path}: 'source_commit' is not a 7-40 hex commit: {commit!r}")
    inputs = entry.get("inputs")
    if not isinstance(inputs, list):
        errors.append(f"{rel_path}: 'inputs' must be a list")
    else:
        for item in inputs:
            if not isinstance(item, dict):
                errors.append(f"{rel_path}: input entry is not an object")
                continue
            digest = item.get("sha256")
            if not isinstance(digest, str) or not SHA256_RE.match(digest):
                errors.append(
                    f"{rel_path}: input sha256 is not a 64 hex digest: {digest!r}"
                )


def check_provenance(manifest: object, report_paths: list) -> list:
    """Return a list of error strings; empty means the gate passes.

    ``report_paths`` is a list of ``(repo_relative_path, absolute_path)`` pairs.
    Fails closed when a report carries a measured number but has no provenance
    entry (or its entry is malformed).
    """
    errors: list = []
    if not isinstance(manifest, dict):
        return ["manifest root is not a JSON object"]
    artifacts = manifest.get("artifacts")
    if not isinstance(artifacts, dict):
        return ["manifest 'artifacts' is missing or not an object"]

    for rel_path, abs_path in report_paths:
        if not os.path.isfile(abs_path):
            errors.append(f"{rel_path}: publication artifact not found")
            continue
        with open(abs_path, "r", encoding="utf-8") as fh:
            body = fh.read()
        entry = artifacts.get(rel_path)
        if not report_has_measured_number(body):
            if entry is not None:
                _validate_entry(rel_path, entry, errors)
            continue
        if entry is None:
            errors.append(
                f"{rel_path}: carries a measured number but has no provenance entry"
            )
            continue
        _validate_entry(rel_path, entry, errors)
    return errors


def discover_report_paths(plans_dir: str = PLANS_DIR) -> list:
    """Discover scannable markdown documents under ``plans_dir``.

    Deterministic (``sorted``) so the gate set is order-independent. EVERY
    ``*.md`` document is a candidate (review P2 residual, PR #1307): gating on a
    filename suffix let a plan doc with measured numbers escape, so the scope is
    now "the whole plans directory" minus the explicit ``DESIGN_DOC_OPTOUT``
    list. Returns ``(repo_relative_path, absolute_path)`` pairs.
    """
    if not os.path.isdir(plans_dir):
        return []
    names = sorted(
        n
        for n in os.listdir(plans_dir)
        if n.endswith(MARKDOWN_SUFFIX) and os.path.isfile(os.path.join(plans_dir, n))
    )
    pairs = []
    for name in names:
        abs_path = os.path.join(plans_dir, name)
        rel_path = os.path.relpath(abs_path, REPO_ROOT).replace(os.sep, "/")
        if rel_path in DESIGN_DOC_OPTOUT:
            continue
        pairs.append((rel_path, abs_path))
    return pairs


def default_report_paths() -> list:
    return discover_report_paths()


def _generator_relationship_entry(
    generator_path: str, generator_sha256: str,
    generator_command: str, seeds: dict, snapshot_path: str,
) -> dict:
    """Provenance for a NON-MEASURED report's generator relationship (#1305).

    These reports state an evidence status (simulation only / MODEL PROJECTION
    / tooling validation only) and carry no measured numbers, so there is no
    measurement to attribute. What CAN be pinned — and what the #1305 closure
    amendment pins — is the generator run behind each report's committed
    snapshot: the exact command, the commit whose tree it ran on, the host it
    ran on, the tool versions, the generator script's content hash, and the
    generator's seeds. ``make reports-regen`` byte-gates each snapshot against
    the generator's current output (host-dependent fields excluded by name,
    never silently). The report body itself stays hand-authored.
    """
    return {
        "source_commit": "cc211ae883a11e0c4c7ea31b939e6aca62248b26",
        "generator_command": generator_command,
        "host": {
            "os": "macOS 27.0.1 / Darwin 27.0.0 arm64",
            "cpu": "Mac17,2",
            "logical_cores": 10,
            "memory_bytes": 25769803776,
        },
        "tool_versions": {
            "python3": "3.13.9",
        },
        "seeds": seeds,
        "inputs": [
            {
                "path": generator_path,
                "sha256": generator_sha256,
            },
            {
                "path": snapshot_path,
                "sha256": _sha256_of(os.path.join(REPO_ROOT, snapshot_path)),
            },
        ],
        "note": (
            "Hand-authored status document with no measured numbers; this "
            "entry pins the generator RUN behind the committed snapshot (%s), "
            "not a measurement. The snapshot is regenerated and byte-diffed by "
            "`make reports-regen` (host-dependent fields excluded by name in "
            "eval/benchmarks/reports_regen.py)."
        ) % snapshot_path,
    }


def _sha256_of(path: str) -> str:
    import hashlib

    with open(path, "rb") as fh:
        return hashlib.sha256(fh.read()).hexdigest()


def _implementation_plan_entry() -> dict:
    """Provenance for the measured AI-repair row in the implementation plan.

    The plan's status table and section 5 quote the Round-2 fault-injection
    campaign (7/10 proposals, 3/10 applied/verified/reruns). The measured table
    of record is ``eval/frontier/PILOT_FINDINGS.md``; this entry names the
    commit that added that campaign's numbers and the harness that produced
    them.
    """
    return {
        "source_commit": "c72cb7e467ce8e106bbab4f029efb373bdb46e20",
        "generator_command": (
            "OXO_FLOW_AI_PROVIDER=openai OPENAI_BASE_URL=<provider-endpoint> "
            "OPENAI_API_KEY=<provider-key> OPENAI_MODEL=<model> "
            "python3 eval/frontier/fault_injection.py "
            "--binary target/release/oxo-flow "
            "--out-json eval/frontier/results/fault_repair_logs/live_summary.json"
        ),
        "host": {
            "os": "Linux",
            "cpu": "school-proxy GLM endpoint (model round-trips remote)",
            "note": "GLM via school proxy, campaign date 2026-11-18",
        },
        "tool_versions": {
            "python3": "3.12.4",
            "oxo-flow": "0.32.0 (v0.32.0 + verified-repair loop, #1185/#1186/#1187)",
        },
        "inputs": [
            {
                "path": "eval/frontier/intents.json",
                "sha256": "76136532e308464bed29827e001b6d894840c5d2e7bdd8925f24361bde75bd51",
            }
        ],
        "note": (
            "Pins the Round-2 seeded fault-injection row (7/10 proposals, 3/10 "
            "applied, 3/10 verified, 3/10 reruns). Attribution provenance only: "
            "the campaign numbers and their raw logs live in "
            "eval/frontier/PILOT_FINDINGS.md and are not re-derived here."
        ),
    }


def _cost_budget_plan_entry() -> dict:
    """Provenance for the measured cluster row in the cost-budget plan (#1400).

    The plan's "Cluster verification" section quotes the fr-cluster control /
    treatment runs. The measured table of record is
    ``docs/superpowers/audits/cost-budget-enforcement-2026-11-26.md``; this
    entry names the branch base that added those numbers and SHA-256-pins the
    committed evidence artifact they live in.
    """
    return {
        "source_commit": "8e6abda9",
        "generator_command": (
            "oxo-flow run cb1400-minimal.oxoflow --profile fr-test "
            "(SLURM partition TEST; control without max_cost, treatment with "
            "amount = 1e-7 USD + gb_second_rate = 1.0)"
        ),
        "host": {
            "os": "Linux (SLURM cluster)",
            "cpu": "fr cluster, partition TEST",
            "note": "fr @ 202.197.87.243:8808, user wangsx, 2026-11-26",
        },
        "tool_versions": {
            "python3": "3.12.4",
            "oxo-flow": "0.32.0",
        },
        "inputs": [
            {
                "path": "docs/superpowers/audits/cost-budget-enforcement-2026-11-26.md",
                "sha256": "ec73bf0ee628697209ed47514406ca643bd0b893b879deef87b11065bc38eeed",
            }
        ],
        "note": (
            "Pins the cluster-verification row (control completed 3/3; treatment "
            "cost_budget_exceeded with only step1.txt written). Attribution "
            "provenance only: the raw run artifacts and their numbers live in "
            "the audit file above and are not re-derived here."
        ),
    }


def build_manifest() -> dict:
    """Build the manifest from the recorded (pinned) provenance of this wave."""
    cohort_entry = {
        "source_commit": "e855fa6a71631237d5973041caf95664af23590d",
        "generator_command": (
            "OXO_FLOW_BIN=target/release/oxo-flow "
            "python3 eval/benchmarks/cohort_driver.py --samples 10"
        ),
        "host": {
            "os": "macOS 26.6.2 (25G83) / Darwin 25.6.0 arm64",
            "cpu": "Apple M3",
            "logical_cores": 8,
            "memory_bytes": 25769803776,
        },
        "tool_versions": {
            "rustc": "1.98.0 (88d9e12ae 2026-08-18)",
            "cargo": "1.98.0",
            "python3": "3.12.4",
        },
        "inputs": [
            {
                "path": "target/release/oxo-flow",
                "sha256": "81bf9bab8fb253b35f1f04575650c7ea573db878a9fd04957378c7788748809f",
            }
        ],
        "seeds": {
            "cohort_generation": 42,
            "note": (
                "cohort_driver.generate_cohort default seed (unchanged across "
                "the recorded commit); the wall-time values are unseeded host "
                "timings"
            ),
        },
        "note": (
            "Pins the section 2 reproduction block. The section 1 row (118.69 ms) "
            "is UNPINNED and documented as such; it is not attributable to this commit."
        ),
    }
    artifacts = {
        "docs/superpowers/plans/cohort-scaling-report.md": cohort_entry,
        "docs/superpowers/plans/hardware-scaling-report.md": (
            _generator_relationship_entry(
                "eval/benchmarks/hardware_scaling.py",
                "e3e4bfc9e3b5d6cb5aa75ff7fa1e79aa899b47b27567aef60c9629f4b71d4758",
                "python3 eval/benchmarks/hardware_scaling.py --allow-simulation "
                "--out-json eval/benchmarks/snapshots/hardware_scaling_projection.json",
                {"note": "none — deterministic fixed-factor model, no RNG"},
                "eval/benchmarks/snapshots/hardware_scaling_projection.json",
            )
        ),
        "docs/superpowers/plans/industrial-scientific-implementation-plan.md": (
            _implementation_plan_entry()
        ),
        "docs/superpowers/plans/statistical-benchmark-report.md": (
            _generator_relationship_entry(
                "eval/frontier/statistical_report.py",
                "f1e90246b20e893c25963e83b03029ea380a6f960871addface65b78c9529c44",
                "python3 eval/frontier/statistical_report.py --synthetic-fixture "
                "--out-md eval/benchmarks/snapshots/statistical_synthetic_fixture.md",
                {"synthetic_dataset_rng": 12345, "bootstrap_rng": 42},
                "eval/benchmarks/snapshots/statistical_synthetic_fixture.md",
            )
        ),
        "docs/superpowers/plans/sus-survey-report.md": (
            _generator_relationship_entry(
                "eval/benchmarks/sus_survey_analysis.py",
                "b74453543b0d6351725d051c85b3baa975951df52035bfb06e06a425cdc22291",
                "python3 eval/benchmarks/sus_survey_analysis.py --out-md "
                "eval/benchmarks/snapshots/sus_survey_harness_check.md",
                {"synthetic_fixture_rng": 999},
                "eval/benchmarks/snapshots/sus_survey_harness_check.md",
            )
        ),
        "docs/superpowers/plans/2026-11-26-cost-budget-enforcement.md": (
            _cost_budget_plan_entry()
        ),
    }
    return {
        "version": 1,
        "description": (
            "Provenance manifest for publication-facing artifacts under "
            "docs/superpowers/plans (#1305). Key-ordered; every scanned *.md "
            "document (minus the explicit DESIGN_DOC_OPTOUT list) that carries "
            "a measured number must have an entry here."
        ),
        "artifacts": dict(sorted(artifacts.items())),
    }


def main() -> int:
    parser = argparse.ArgumentParser(description="Publication-artifact provenance manifest")
    parser.add_argument("--emit", default=None, help="Write the manifest JSON here")
    parser.add_argument("--check", action="store_true", help="Validate the manifest (fail closed)")
    parser.add_argument("--manifest", default=DEFAULT_MANIFEST, help="Manifest path for --check")
    args = parser.parse_args()

    if args.emit:
        manifest = build_manifest()
        os.makedirs(os.path.dirname(os.path.abspath(args.emit)), exist_ok=True)
        with open(args.emit, "w", encoding="utf-8") as fh:
            json.dump(manifest, fh, indent=2, sort_keys=True)
            fh.write("\n")
        print(f"Provenance manifest written to: {args.emit}")
        return 0

    if args.check:
        if not os.path.isfile(args.manifest):
            print(f"provenance-check: manifest not found: {args.manifest}", file=sys.stderr)
            return 1
        try:
            with open(args.manifest, "r", encoding="utf-8") as fh:
                manifest = json.load(fh)
        except json.JSONDecodeError as exc:
            print(f"provenance-check: manifest is not valid JSON: {exc}", file=sys.stderr)
            return 1
        errors = check_provenance(manifest, default_report_paths())
        if errors:
            print("provenance-check: FAILED", file=sys.stderr)
            for err in errors:
                print(f"  - {err}", file=sys.stderr)
            return 1
        print("provenance-check: OK (manifest well-formed; every measured report has provenance)")
        return 0

    parser.print_help()
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
