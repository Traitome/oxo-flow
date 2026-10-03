#!/usr/bin/env python3
"""Judge captured AI outputs against the gold CSVs.

Usage:
  python3 eval/scripts/runner.py tool --captures outputs/tool_answers.csv
  python3 eval/scripts/runner.py rule --captures outputs/rules --oxo-flow <bin>
  python3 eval/scripts/runner.py workflow --captures outputs/workflows --oxo-flow <bin>

Scores are 0..1 per sub-metric; `overall` is the mean of the applicable
sub-metrics. The detailed CSV is one row per judged trial. Companion outputs:
`*.items.csv` (per-item aggregate) and `*.summary.json` (dataset summary).
Stdlib only.
"""

import argparse
import csv
import json
import os
import re
import statistics
import sys
import tomllib

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import common  # noqa: E402

NOT_FOUND_RE = re.compile(
    r"not found|no such|does not exist|doesn't exist|no tool|unable to find|"
    r"cannot find|could not find|couldn't find|could not locate|does not appear to exist|"
    r"unknown tool|i don'?t know of|no known",
    re.IGNORECASE,
)
TRIAL_RE = re.compile(r"trial-(\d+)")

# Executable -> bioconda package it ships in, for references that pin the
# package name while the shell invokes the executable (#172 audit).
PACKAGE_ALIASES = {
    "gatk": ("gatk4",),
    "featureCounts": ("subread",),
    "deduplicate_bismark": ("bismark",),
    "bismark_methylation_extractor": ("bismark",),
    "bismark_genome_preparation": ("bismark",),
    "coverage2cytosine": ("bismark",),
    "annotatePeaks.pl": ("homer",),
    "trim_galore": ("trim-galore",),
    "computeMatrix": ("deeptools",),
}


# ── Shared helpers ──────────────────────────────────────────────────────────

def mean(values):
    return round(sum(values) / len(values), 3) if values else 0.0


def stdev(values):
    return round(statistics.stdev(values), 3) if len(values) >= 2 else 0.0


def metric_fields(results):
    """Numeric metric keys across ALL rows (union, first-seen order).

    Reading only results[0] dropped metrics that appear later — the tool
    layer's `no_hallucination` never reached the summary because the first
    judged row was a positive sample (#172 audit).
    """
    excluded = {
        "id",
        "trial",
        "layer",
        "capture_path",
        "parse_err",
        "answer_len",
        "rule_count",
        "capture_error",
        "perfect",
    }
    fields = []
    for row in results:
        for key, value in row.items():
            if key in excluded or key in fields:
                continue
            if isinstance(value, (int, float)) and not isinstance(value, bool):
                fields.append(key)
    return fields


def summarize_rows(rows, metrics):
    out = {"n": len(rows)}
    for metric in metrics:
        values = [float(r[metric]) for r in rows if metric in r]
        out[metric] = {
            "mean": mean(values),
            "stdev": stdev(values),
            "perfect_rate": round(sum(1 for v in values if v == 1.0) / len(values), 3) if values else 0.0,
        }
    overall_values = [float(r.get("overall", 0.0)) for r in rows]
    out["perfect_trial_rate"] = round(
        sum(1 for r in rows if float(r.get("overall", 0.0)) == 1.0) / len(rows), 3
    ) if rows else 0.0
    out["overall_mean"] = mean(overall_values)
    out["overall_stdev"] = stdev(overall_values)
    return out


def per_item_summary(results):
    by_id = {}
    for row in results:
        by_id.setdefault(row["id"], []).append(row)
    summaries = []
    for item_id, rows in sorted(by_id.items()):
        overalls = [float(r.get("overall", 0.0)) for r in rows]
        summaries.append(
            {
                "id": item_id,
                "trials": len(rows),
                "overall_mean": mean(overalls),
                "overall_stdev": stdev(overalls),
                "perfect_trial_rate": round(sum(1 for v in overalls if v == 1.0) / len(overalls), 3),
                "pass_at_k": 1.0 if any(v == 1.0 for v in overalls) else 0.0,
            }
        )
    return summaries


def load_manifest(captures_path):
    path = common.manifest_path(captures_path)
    if os.path.isfile(path):
        with open(path, encoding="utf-8") as fh:
            return path, json.load(fh)
    return "", None


def write_csv(path, rows):
    if not rows:
        return
    fieldnames = []
    for row in rows:
        for key in row:
            if key not in fieldnames:
                fieldnames.append(key)
    os.makedirs(os.path.dirname(os.path.abspath(path)) or ".", exist_ok=True)
    with open(path, "w", newline="", encoding="utf-8") as fh:
        writer = csv.DictWriter(fh, fieldnames=fieldnames, extrasaction="ignore")
        writer.writeheader()
        writer.writerows(rows)


def parse_trial(value, default=1):
    try:
        return int(value or default)
    except (TypeError, ValueError):
        return default


def trial_from_path(path):
    for part in path.replace("\\", "/").split("/"):
        m = TRIAL_RE.search(part)
        if m:
            return int(m.group(1))
    return 1


def load_generated(path):
    """Parse a generated .oxoflow file; returns (text, rules, err).

    `[defaults]` values are merged into every rule (rule keys win), matching
    the engine: reference rules that declare threads/memory only via
    `[defaults]` were otherwise scored as having no resources at all (#172
    audit).
    """
    with open(path, encoding="utf-8") as fh:
        text = fh.read()
    try:
        data = tomllib.loads(text)
    except tomllib.TOMLDecodeError as exc:
        return text, [], str(exc)
    defaults = data.get("defaults") or {}
    rules = [{**defaults, **rule} for rule in data.get("rules", [])]
    return text, rules, None


def find_pinned_version(text, tool):
    """Extract a version pin from conda or image-style references."""
    patterns = [
        re.compile(
            r"(?:bioconda::)?" + re.escape(tool) + r"\s*=\s*(\d+(?:\.\d+)*[a-zA-Z0-9.-]*)",
            re.IGNORECASE,
        ),
        re.compile(
            re.escape(tool) + r"\s*[:@]\s*(\d+(?:\.\d+)*[a-zA-Z0-9.-]*)",
            re.IGNORECASE,
        ),
    ]
    for pattern in patterns:
        m = pattern.search(text)
        if m:
            # Docker tags carry build suffixes (`ucsc-bedgraphtobigwig:445--h954228d_0`);
            # the release identity is the part before `--` (#172 audit).
            return re.split(r"--", m.group(1), maxsplit=1)[0] or m.group(1)
    return None


def parse_threads(value):
    if isinstance(value, int):
        return value
    if isinstance(value, str) and value.strip().isdigit():
        return int(value.strip())
    return None


def memory_to_mb(value):
    if isinstance(value, (int, float)):
        return int(value)
    if not isinstance(value, str):
        return None
    m = re.fullmatch(r"(\d+(?:\.\d+)?)\s*([GMK])i?B?", value.strip(), re.IGNORECASE)
    if not m:
        return None
    number, unit = float(m.group(1)), m.group(2).upper()
    return int(number * {"G": 1024, "M": 1, "K": 1 / 1024}[unit])


def parse_resources(rule):
    res = rule.get("resources", {})
    return parse_threads(res.get("threads")), res.get("memory")


def inferred_edges(rules):
    edges = []
    for src in rules:
        src_name = src.get("name", "")
        src_out = src.get("output", [])
        for dst in rules:
            if dst is src:
                continue
            if any(common.path_matches(out_path, in_path) for out_path in src_out for in_path in dst.get("input", [])):
                edges.append((src_name, dst.get("name", "")))
    return sorted(set(edges))


DOT_NODE_RE = re.compile(r'^\s*(\d+)\s*\[\s*label\s*=\s*"([^"]*)"\s*\]', re.MULTILINE)
DOT_EDGE_RE = re.compile(r"^\s*(\d+)\s*->\s*(\d+)\s*\[", re.MULTILINE)


def parse_dot_graph(text):
    """(node labels, (from, to) edge pairs) from `oxo-flow graph -f dot`."""
    labels = {idx: name for idx, name in DOT_NODE_RE.findall(text)}
    nodes = set(labels.values())
    edges = set()
    for src, dst in DOT_EDGE_RE.findall(text):
        if src in labels and dst in labels:
            edges.add((labels[src], labels[dst]))
    return nodes, edges


def parse_dot_edges(text):
    """(from_name, to_name) rule pairs from `oxo-flow graph -f dot` output."""
    return parse_dot_graph(text)[1]


def engine_edges(oxo_flow_bin, path):
    """The engine's own rule-level DAG for a workflow file.

    The engine connects rules through more than literal io suffixes
    (`depends_on`, pattern-tolerant input/output matching), so its graph —
    not the python suffix inference — is the reference for edge_coverage.
    On engine failure this falls back to suffix inference with a printed
    warning (best-effort only; the fallback cannot see `depends_on`).
    """
    code, out, err = common.oxo_flow_cmd(
        oxo_flow_bin, ["graph", os.path.basename(path), "-f", "dot"], cwd=os.path.dirname(path)
    )
    if code != 0:
        print(f"WARN: `oxo-flow graph` failed for {path} ({err.strip() or f'exit {code}'}); using suffix inference")
        _, rules, _ = load_generated(path)
        return set(inferred_edges(rules))
    return parse_dot_edges(out)


def pick_capture_file(captures_dir, item_id):
    direct = os.path.join(captures_dir, f"{item_id}.oxoflow")
    if os.path.isfile(direct):
        return [{"trial": 1, "path": direct, "meta": direct + ".meta.json"}]

    item_dir = os.path.join(captures_dir, item_id)
    if not os.path.isdir(item_dir):
        return []

    candidates = []
    for dirpath, _, filenames in os.walk(item_dir):
        for name in sorted(filenames):
            if not name.endswith(".oxoflow"):
                continue
            path = os.path.join(dirpath, name)
            trial = trial_from_path(os.path.relpath(path, item_dir))
            meta = os.path.join(item_dir, f"trial-{trial:03d}.meta.json")
            if not os.path.isfile(meta):
                meta = path + ".meta.json"
            candidates.append({"trial": trial, "path": path, "meta": meta})

    by_trial = {}
    for item in candidates:
        by_trial.setdefault(item["trial"], []).append(item)
    chosen = []
    for trial, items in sorted(by_trial.items()):
        chosen.append(sorted(items, key=lambda x: x["path"])[0])
    return chosen


def load_meta(path):
    if path and os.path.isfile(path):
        with open(path, encoding="utf-8") as fh:
            return json.load(fh)
    return {}


def path_hits(expected_paths, declared_paths):
    if not expected_paths:
        return 0.0
    hits = 0
    for expected in expected_paths:
        if any(common.path_matches(expected, declared) for declared in declared_paths):
            hits += 1
    return round(hits / len(expected_paths), 3)


# ── Tool layer ──────────────────────────────────────────────────────────────

MIN_MENTION_NAME_LEN = 3
MAX_MENTION_WINDOW = 4
SUGGESTION_CUES = (
    "did you", "thinking of", "known as", "available as", "closest", "nearest",
    "alternative", "meant", "mean", "means", "maybe", "perhaps", "suggest",
    "instead", "try", "trying", "use", "used", "uses", "using", "refer",
    "refers", "referring", "similar", "correct", "actually", "probably",
    "equivalent", "replacement", "called", "calling", "exists",
)
_SUGGESTION_RE = re.compile(
    r"\b(?:" + "|".join(re.escape(cue) for cue in SUGGESTION_CUES) + r")\b",
    re.IGNORECASE,
)


def _mention_tokens(text):
    """Lowercase alphanumeric tokens; -, _ and / collapse into breaks."""
    text = re.sub(r"[-_/]", " ", text.lower())
    text = re.sub(r"[^a-z0-9 ]", " ", text)
    return text.split()


def _drop_token_span(tokens, span):
    """Remove every contiguous occurrence of `span` from `tokens`."""
    if not span:
        return tokens
    out, i = [], 0
    while i < len(tokens):
        if tokens[i : i + len(span)] == span:
            i += len(span)
        else:
            out.append(tokens[i])
            i += 1
    return out


def _mention_forms(tokens):
    """Spellings the text can produce: tokens plus adjacent concatenations.

    The window concat catches hyphen/space variants of real tools
    (`fast-qc`, `multi qc` for `fastqc`, `multiqc`) that pure token matching
    let through (#172 audit).
    """
    forms = set(tokens)
    for size in range(2, MAX_MENTION_WINDOW + 1):
        for i in range(len(tokens) - size + 1):
            forms.add("".join(tokens[i : i + size]))
    return forms


def answer_mentions_known_tool(answer, known_names, exclude_name=""):
    """True when the answer *suggests* a known tool as the intended answer.

    Only sentences carrying a suggestion/existence cue ("did you mean",
    "maybe", "use ... instead", "closest", "known as", ...) are scanned, so
    a rejection that merely mentions a format word or an unrelated tool
    ("it is not a SAM/BAM tool") is not counted as a fallback suggestion
    (#172 audit). The queried fabricated name is dropped before the scan —
    a correct rejection has to echo it — and matching accepts token, fused
    and adjacent-window spellings. Names shorter than
    MIN_MENTION_NAME_LEN characters are skipped as too ambiguous to scan.
    """
    for sentence in re.split(r"(?<=[.!?;])\s+|\n+", answer):
        if not sentence or not _SUGGESTION_RE.search(sentence):
            continue
        tokens = _mention_tokens(sentence)
        if exclude_name:
            excluded = _mention_tokens(exclude_name)
            tokens = _drop_token_span(tokens, excluded)
            if excluded:
                fused_excluded = "".join(excluded)
                tokens = [token for token in tokens if token != fused_excluded]
        if not tokens:
            continue
        forms = _mention_forms(tokens)
        joined = " " + " ".join(tokens) + " "
        for name in known_names:
            name_tokens = _mention_tokens(name)
            if not name_tokens:
                continue
            fused = "".join(name_tokens)
            if len(fused) < MIN_MENTION_NAME_LEN:
                continue
            if fused in forms or f" {' '.join(name_tokens)} " in joined:
                return True
    return False


def judge_tool(gold_rows, captures_path):
    by_id = {}
    with open(captures_path, newline="", encoding="utf-8") as fh:
        for row in csv.DictReader(fh):
            row["trial"] = parse_trial(row.get("trial"))
            by_id.setdefault(row["id"], []).append(row)

    known_names = common.known_tool_names()
    results = []
    for row in gold_rows:
        captures = sorted(by_id.get(row["id"], []), key=lambda r: r["trial"])
        if not captures:
            print(f"WARN: no capture for {row['id']} — skipped")
            continue
        for capture in captures:
            answer = capture.get("answer", "")
            scores = {}
            if row["expected_tool"]:
                scores["name_match"] = 1.0 if common.name_present(row["expected_tool"], answer) else 0.0
            # schema.md: expected_version is empty when the query does not ask
            # for a version, and version_match is scored only when it is asked
            # (#172: 45 rows carried a version on non-version queries and
            # penalised correct answers). Rows without a query keep the old
            # behaviour for synthetic inputs.
            query = row.get("query", "")
            asks_version = "version" in query.lower() if query else True
            if row["expected_version"] and asks_version:
                # Boundary-checked match: "2.0.4" must not be satisfied by
                # "2.0.40" or "12.0.4" (#172 audit), while "v2.0.4" still is.
                version = re.sub(r"\s", "", row["expected_version"])
                scores["version_match"] = (
                    1.0
                    if re.search(r"(?<![0-9.])" + re.escape(version) + r"(?![0-9A-Za-z])", re.sub(r"\s", "", answer))
                    else 0.0
                )
            if row["negative_sample"] == "1":
                rejected = bool(NOT_FOUND_RE.search(answer))
                hallucinated = answer_mentions_known_tool(
                    answer,
                    known_names,
                    exclude_name=common.fake_tool_name(row.get("query", "")),
                )
                scores["no_hallucination"] = 1.0 if rejected and not hallucinated else 0.0
            scores["overall"] = mean(list(scores.values())) if scores else 0.0
            results.append(
                {
                    "id": row["id"],
                    "trial": capture["trial"],
                    "layer": "tool",
                    **scores,
                    "perfect": 1.0 if scores and all(v == 1.0 for v in scores.values()) else 0.0,
                    "answer_len": len(answer),
                    "capture_error": capture.get("error", ""),
                }
            )
    return results


# ── Rule layer ──────────────────────────────────────────────────────────────

def judge_rule(gold_rows, captures_dir, oxo_flow_bin):
    versions = common.bioconda_versions()
    known_pins = common.known_version_pins(versions)
    results = []
    for row in gold_rows:
        captures = pick_capture_file(captures_dir, row["id"])
        if not captures:
            print(f"WARN: no capture for {row['id']} — skipped")
            continue
        for capture in captures:
            text, rules, parse_err = load_generated(capture["path"])
            rule = rules[0] if rules else {}
            shell = rule.get("shell", "") if rule else text
            scores = {}

            scores["tool_present"] = 1.0 if common.name_present(row["expected_tool"], shell + " " + text) else 0.0

            if row["expected_version"]:
                # References pin the *package* name, which often differs from
                # the executable the shell invokes (gatk -> gatk4, featureCounts
                # -> subread); without the alias the pin is invisible and a
                # reference-faithful answer scores 0 (#172 audit).
                pinned, known = None, set()
                for name in (row["expected_tool"],) + PACKAGE_ALIASES.get(row["expected_tool"], ()):
                    if pinned is None:
                        pinned = find_pinned_version(text, name)
                    known |= known_pins.get(name, set())
                scores["version_pinned"] = 1.0 if pinned and (pinned in known or pinned == row["expected_version"]) else 0.0

            try:
                params = json.loads(row["expected_key_params"])
            except json.JSONDecodeError:
                params = []
            if params:
                scores["key_params"] = round(
                    sum(1 for p in params if re.search(re.escape(p), shell, re.IGNORECASE)) / len(params),
                    3,
                )

            try:
                expected_in = json.loads(row["expected_inputs"])
                expected_out = json.loads(row["expected_outputs"])
            except json.JSONDecodeError:
                expected_in, expected_out = [], []
            declared_in = list(rule.get("input", [])) if rule else []
            if rule:
                # `expand_inputs` patterns are real declared inputs (the
                # reference's gather rules take their io from there).
                declared_in += [
                    entry.get("pattern", "")
                    for entry in rule.get("expand_inputs", [])
                    if isinstance(entry, dict) and entry.get("pattern")
                ]
            declared_out = rule.get("output", []) if rule else []
            io_scores = []
            if expected_in:
                io_scores.append(path_hits(expected_in, declared_in))
            if expected_out:
                io_scores.append(path_hits(expected_out, declared_out))
            if io_scores:
                scores["io_declared"] = mean(io_scores)

            code, _, _ = common.oxo_flow_cmd(
                oxo_flow_bin, ["validate", os.path.basename(capture["path"])], cwd=os.path.dirname(capture["path"])
            )
            scores["validate_pass"] = 1.0 if code == 0 else 0.0

            try:
                resource_range = json.loads(row["resource_range"])
            except json.JSONDecodeError:
                resource_range = {}
            threads, memory = parse_resources(rule) if rule else (None, None)
            scores["resources_declared"] = 1.0 if (threads is not None or memory is not None) else 0.0
            if threads is None and memory is None:
                scores["resources_in_range"] = 0.0
            else:
                ok = True
                if threads is not None:
                    ok = ok and resource_range.get("threads_min", 1) <= threads <= resource_range.get("threads_max", 128)
                if memory is not None:
                    mem_mb = memory_to_mb(memory)
                    ok = ok and (mem_mb is not None) and mem_mb <= resource_range.get("memory_max_mb", 262144)
                scores["resources_in_range"] = 1.0 if ok else 0.0

            meta = load_meta(capture["meta"])
            scores["overall"] = mean(list(scores.values())) if scores else 0.0
            results.append(
                {
                    "id": row["id"],
                    "trial": capture["trial"],
                    "layer": "rule",
                    **scores,
                    "perfect": 1.0 if scores and all(v == 1.0 for v in scores.values()) else 0.0,
                    "capture_path": capture["path"],
                    "parse_err": parse_err or "",
                    "capture_error": meta.get("error", ""),
                }
            )
    return results


# ── Workflow layer ──────────────────────────────────────────────────────────

def judge_workflow(gold_rows, captures_dir, oxo_flow_bin):
    results = []
    for row in gold_rows:
        captures = pick_capture_file(captures_dir, row["id"])
        if not captures:
            print(f"WARN: no capture for {row['id']} — skipped")
            continue
        for capture in captures:
            text, rules, parse_err = load_generated(capture["path"])
            names = [r.get("name", "") for r in rules]
            scores = {}

            code, _, _ = common.oxo_flow_cmd(
                oxo_flow_bin, ["validate", os.path.basename(capture["path"])], cwd=os.path.dirname(capture["path"])
            )
            scores["validate_pass"] = 1.0 if code == 0 else 0.0
            code, _, _ = common.oxo_flow_cmd(
                oxo_flow_bin, ["lint", os.path.basename(capture["path"])], cwd=os.path.dirname(capture["path"])
            )
            scores["lint_pass"] = 1.0 if code == 0 else 0.0

            try:
                expected_steps = json.loads(row["expected_steps"])
                expected_tools = json.loads(row["expected_tools"])
                expected_edges = json.loads(row["expected_dag_edges"])
                expected_outputs = json.loads(row["expected_outputs"])
            except json.JSONDecodeError:
                expected_steps = expected_tools = expected_edges = expected_outputs = []

            if expected_steps:
                hits = sum(1 for step in expected_steps if any(common.loose_step_match(step, name) for name in names))
                scores["step_coverage"] = round(hits / len(expected_steps), 3)
            if expected_tools:
                hits = sum(1 for tool in expected_tools if common.name_present(tool, text))
                scores["tool_coverage"] = round(hits / len(expected_tools), 3)

            name_map = common.map_expected_steps(expected_steps, names)
            edges = engine_edges(oxo_flow_bin, capture["path"])
            if expected_edges:
                hits = sum(1 for src, dst in expected_edges if (name_map.get(src), name_map.get(dst)) in edges)
                scores["edge_coverage"] = round(hits / len(expected_edges), 3)

            if expected_outputs:
                declared = [p for r in rules for p in r.get("output", [])]
                scores["output_coverage"] = path_hits(expected_outputs, declared)

            meta = load_meta(capture["meta"])
            scores["overall"] = mean(list(scores.values())) if scores else 0.0
            results.append(
                {
                    "id": row["id"],
                    "trial": capture["trial"],
                    "layer": "workflow",
                    **scores,
                    "perfect": 1.0 if scores and all(v == 1.0 for v in scores.values()) else 0.0,
                    "rule_count": len(rules),
                    "capture_path": capture["path"],
                    "parse_err": parse_err or "",
                    "capture_error": meta.get("error", ""),
                }
            )
    return results


def build_validity_warnings(summary, gold_rows, capture_manifest):
    warnings = []
    gold_drafters = sorted({row.get("gold_draft_by", "") for row in gold_rows if row.get("gold_draft_by")})
    provider_kind = ((capture_manifest or {}).get("provider") or {}).get("kind", "")
    if provider_kind and provider_kind in gold_drafters:
        warnings.append(
            "Capture provider matches the gold drafter family (`gold_draft_by`), so reported scores should be treated as same-family evaluation unless an independently reviewed holdout is used."
        )
    if (capture_manifest or {}).get("include_unreviewed"):
        warnings.append(
            "Capture included unreviewed gold rows (`--include-unreviewed`); treat results as preview-only, not publication-grade benchmark output."
        )
    if summary["n_items"] < 10:
        warnings.append("Fewer than 10 items were judged; dataset-level estimates are low-confidence.")
    low_n = []
    for name, stats in summary["by_difficulty"].items():
        if stats["n"] < 10:
            low_n.append(f"difficulty:{name}={stats['n']}")
    for name, stats in summary["by_query_type"].items():
        if stats["n"] < 10:
            low_n.append(f"query_type:{name}={stats['n']}")
    if low_n:
        warnings.append("Low-n breakdowns (<10 judged trials) should be treated cautiously: " + ", ".join(low_n))
    return warnings


# ── Reporting ───────────────────────────────────────────────────────────────

def report(results, gold_rows, out_path, summary_out=None, item_summary_out=None, capture_manifest=None, capture_manifest_path=""):
    if not results:
        print("no rows judged — nothing to report")
        return

    write_csv(out_path, results)
    item_rows = per_item_summary(results)
    write_csv(item_summary_out, item_rows)

    metrics = metric_fields(results)
    gold_by_id = {r["id"]: r for r in gold_rows}
    summary = {
        "benchmark": "oxo-flow-ai-eval",
        "layer": results[0]["layer"],
        "generated_at": common.utc_now(),
        "detail_csv": os.path.abspath(out_path),
        "item_summary_csv": os.path.abspath(item_summary_out),
        "capture_manifest_path": capture_manifest_path,
        "capture_manifest": capture_manifest,
        "n_results": len(results),
        "n_items": len({r["id"] for r in results}),
        "n_trials_max": max(r["trial"] for r in results),
        "overall": summarize_rows(results, metrics),
        "item_pass_at_k": mean([row["pass_at_k"] for row in item_rows]),
        "by_difficulty": {},
        "by_query_type": {},
        "per_item": item_rows,
        "comparability_note": (
            "Overall scores average only the sub-metrics applicable to that row; mixed query types "
            "may therefore have different metric counts."
        ),
    }

    difficulties = sorted({gold.get("difficulty", "") for gold in gold_rows if gold.get("difficulty")})
    for difficulty in difficulties:
        rows = [r for r in results if gold_by_id.get(r["id"], {}).get("difficulty") == difficulty]
        if rows:  # skip empty groups — n=0 with mean=0.000 reads as a real score
            summary["by_difficulty"][difficulty] = summarize_rows(rows, metrics)

    query_types = sorted({gold.get("query_type", "") for gold in gold_rows if gold.get("query_type")})
    for query_type in query_types:
        rows = [r for r in results if gold_by_id.get(r["id"], {}).get("query_type") == query_type]
        if rows:
            summary["by_query_type"][query_type] = summarize_rows(rows, metrics)

    summary["validity_warnings"] = build_validity_warnings(summary, gold_rows, capture_manifest)
    common.write_json(summary_out, summary)

    print(
        f"\n=== {results[0]['layer']} layer: {summary['n_items']} items, {summary['n_results']} judged trial(s), "
        f"mean overall {summary['overall']['overall_mean']:.3f}, "
        f"item pass@k {summary['item_pass_at_k']:.3f} ==="
    )
    for difficulty, stats in summary["by_difficulty"].items():
        print(f"  difficulty={difficulty:6s} n={stats['n']:3d} mean={stats['overall_mean']:.3f} stdev={stats['overall_stdev']:.3f}")
    for query_type, stats in summary["by_query_type"].items():
        print(f"  query_type={query_type:12s} n={stats['n']:3d} mean={stats['overall_mean']:.3f}")
    print(f"\nWrote detail CSV to {out_path}")
    print(f"Wrote item summary CSV to {item_summary_out}")
    print(f"Wrote summary JSON to {summary_out}")


def main():
    ap = argparse.ArgumentParser(description=__doc__)
    ap.add_argument("layer", choices=["tool", "rule", "workflow"])
    ap.add_argument("--captures", required=True, help="answers CSV or captures dir")
    ap.add_argument("--oxo-flow", help="oxo-flow binary (rule/workflow layers)")
    ap.add_argument("--out", default="eval/results.csv", help="detailed results CSV path")
    ap.add_argument("--summary-out", help="summary JSON path (default: derived from --out)")
    ap.add_argument("--item-summary-out", help="per-item summary CSV path (default: derived from --out)")
    ap.add_argument("--gold", help="override gold CSV path (default: eval/gold/<layer>.csv)")
    ap.add_argument("--include-unreviewed", action="store_true")
    args = ap.parse_args()

    gold_rows = common.load_gold(args.layer, args.include_unreviewed, args.gold)
    capture_manifest_path, capture_manifest = load_manifest(args.captures)

    if args.layer == "tool":
        results = judge_tool(gold_rows, args.captures)
    else:
        if not args.oxo_flow:
            sys.exit("--oxo-flow <binary> is required for the rule/workflow layers")
        # Resolve once against the caller's cwd: judge commands run with the
        # capture dir as cwd, where a relative binary path silently fails and
        # zeroes validate/lint/edge scores for every row (#172 audit).
        oxo_flow_bin = os.path.abspath(args.oxo_flow)
        if not os.path.isfile(oxo_flow_bin):
            sys.exit(f"--oxo-flow binary not found: {oxo_flow_bin}")
        if args.layer == "rule":
            results = judge_rule(gold_rows, args.captures, oxo_flow_bin)
        else:
            results = judge_workflow(gold_rows, args.captures, oxo_flow_bin)
    report(
        results,
        gold_rows,
        args.out,
        summary_out=args.summary_out or common.summary_json_path(args.out),
        item_summary_out=args.item_summary_out or common.item_summary_csv_path(args.out),
        capture_manifest=capture_manifest,
        capture_manifest_path=capture_manifest_path,
    )


if __name__ == "__main__":
    main()
