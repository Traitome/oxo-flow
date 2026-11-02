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
import math
import os
import random
import re
import statistics
import sys
import tomllib

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import common  # noqa: E402
import ai_judge  # noqa: E402

NOT_FOUND_RE = re.compile(
    r"not found|no such|does not exist|doesn't exist|no tool|unable to find|"
    r"cannot find|could not find|couldn't find|could not locate|does not appear to exist|"
    r"unknown tool|i don'?t know of|no known|no record|no entry|not listed|"
    r"not included|not known|don'?t recognize|do not recognize|unaware of",
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


def bootstrap_ci(values, confidence=0.95, iterations=10000, seed=172):
    """Deterministic percentile bootstrap CI for a mean (stdlib only)."""
    values = [float(value) for value in values]
    if not values:
        return {"lower": 0.0, "upper": 0.0, "confidence": confidence, "iterations": 0}
    rng = random.Random(seed)
    n = len(values)
    means = []
    for _ in range(iterations):
        means.append(sum(values[rng.randrange(n)] for _ in range(n)) / n)
    means.sort()
    alpha = (1.0 - confidence) / 2.0
    percentile_n = len(means)
    # Nearest-rank empirical percentile. With 10,000 bootstrap means, the
    # 2.5th percentile is order statistic 250 (zero-based index 249).
    lower_index = min(percentile_n - 1, max(0, int(math.ceil(alpha * percentile_n)) - 1))
    upper_index = min(percentile_n - 1, max(0, int(math.ceil((1.0 - alpha) * percentile_n)) - 1))
    return {
        "lower": round(means[lower_index], 3),
        "upper": round(means[upper_index], 3),
        "confidence": confidence,
        "iterations": iterations,
    }


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
        "capture_source",
        "perfect",
        "ai_judge_json",
        "ai_judge_rationale",
        "ai_judge_error",
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
    out["overall_ci95"] = bootstrap_ci(overall_values)
    return out


def per_item_summary(results):
    by_id = {}
    for row in results:
        by_id.setdefault(row["id"], []).append(row)
    summaries = []
    for item_id, rows in sorted(by_id.items()):
        overalls = [float(r.get("overall", 0.0)) for r in rows]
        first_trials = [r for r in rows if int(r.get("trial", 0)) == 1]
        summaries.append(
            {
                "id": item_id,
                "trials": len(rows),
                "first_trial_available": 1.0 if first_trials else 0.0,
                "first_trial_overall": mean([float(r.get("overall", 0.0)) for r in first_trials]),
                "first_trial_perfect": 1.0 if any(float(r.get("overall", 0.0)) == 1.0 for r in first_trials) else 0.0,
                "overall_mean": mean(overalls),
                "overall_stdev": stdev(overalls),
                "perfect_trial_rate": round(sum(1 for v in overalls if v == 1.0) / len(overalls), 3),
                "pass_at_k": 1.0 if any(v == 1.0 for v in overalls) else 0.0,
            }
        )
    return summaries


def capture_availability(gold_rows, results, capture_manifest):
    """Report successful captures as a first-class infrastructure metric."""
    trials = int((capture_manifest or {}).get("trials") or 1)
    # A smoke run may limit `--limit 20` while the judge loads the full gold
    # file.  The manifest's selected item ids, not the full CSV, define the
    # intended denominator.
    manifest_ids = set((capture_manifest or {}).get("item_ids") or [])
    selected_rows = [row for row in gold_rows if not manifest_ids or row.get("id") in manifest_ids]
    expected_trials = len(selected_rows) * trials
    successful = len(results)
    if not expected_trials:
        return {
            "expected_trials": 0,
            "successful_trials": successful,
            "reported_failures": int((capture_manifest or {}).get("failures") or 0),
            "selected_items": len(selected_rows),
            "capture_success_rate": 1.0 if results else 0.0,
            "basis": "successful_results_no_manifest" if results else "no_successful_results",
        }
    rate = min(1.0, successful / expected_trials)
    return {
        "expected_trials": expected_trials,
        "successful_trials": successful,
        "reported_failures": int((capture_manifest or {}).get("failures") or 0),
        "selected_items": len(selected_rows),
        "capture_success_rate": round(rate, 3),
        "basis": "manifest_expected_trials" if rate == successful / expected_trials else "capped_invalid_manifest_denominator",
    }


def load_manifest(captures_path):
    path = common.manifest_path(captures_path)
    if os.path.isfile(path):
        with open(path, encoding="utf-8") as fh:
            return path, json.load(fh)
    return "", None


def ai_judge_detail_path(out_path):
    out_path = os.path.abspath(out_path)
    stem, ext = os.path.splitext(out_path)
    return stem + ".ai_judge.csv" if ext else os.path.join(out_path, "ai_judge.csv")


def ai_judge_summary_path(out_path):
    out_path = os.path.abspath(out_path)
    stem, ext = os.path.splitext(out_path)
    return stem + ".ai_judge.summary.json" if ext else os.path.join(out_path, "ai_judge.summary.json")


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


def rule_shell(rule, fallback_text):
    """Collect shell text from a rule, including transform map/combine."""
    shell = rule.get("shell", "") if rule else fallback_text
    if rule and isinstance(rule.get("transform"), dict):
        transform = rule["transform"]
        parts = [transform.get("map", "")]
        if isinstance(transform.get("combine"), dict):
            parts.append(transform["combine"].get("shell", ""))
        transform_shell = " ".join(part for part in parts if part)
        shell = (shell + " " + transform_shell).strip() if shell else transform_shell
    return shell


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
    """Read resources from either [rules.resources] or top-level keys."""
    resources = rule.get("resources", {})
    if not isinstance(resources, dict):
        resources = {}
    threads = parse_threads(resources.get("threads"))
    if threads is None:
        threads = parse_threads(rule.get("threads"))
    memory = resources.get("memory")
    if memory is None:
        memory = rule.get("memory")
    return threads, memory


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
        if any(
            common.path_matches(expected, declared) or common.path_role_matches(expected, declared)
            for declared in declared_paths
        ):
            hits += 1
    return round(hits / len(expected_paths), 3)


def param_present(param, shell):
    """Boundary-match a CLI option so --out cannot satisfy --output."""
    escaped = re.escape(param)
    return bool(re.search(rf"(?<![A-Za-z0-9_-]){escaped}(?![A-Za-z0-9_-])", shell, re.IGNORECASE))


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
            row["trial"] = common.parse_trial(row.get("trial"))
            by_id.setdefault(row["id"], []).append(row)

    known_names = common.known_tool_names()
    results = []
    for row in gold_rows:
        captures = sorted(by_id.get(row["id"], []), key=lambda r: r["trial"])
        if not captures:
            print(f"WARN: no capture for {row['id']} — skipped")
            continue
        for capture in captures:
            if (capture.get("error") or "").strip():
                # An API/transport failure is infrastructure, not a model
                # answer: scoring the empty string would book the rate limiter
                # as a hallucination. Skip it; the manifest carries the cause.
                print(
                    f"WARN: {row['id']} trial {capture['trial']} capture failed "
                    f"({capture['error'][:80]}) — skipped"
                )
                continue
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
                    "capture_source": captures_path,
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
            meta = load_meta(capture["meta"])
            if (meta.get("error") or "").strip():
                print(
                    f"WARN: {row['id']} trial {capture['trial']} capture failed "
                    f"({str(meta.get('error', ''))[:80]}) — skipped"
                )
                continue
            text, rules, parse_err = load_generated(capture["path"])
            rule = rules[0] if rules else {}
            shell = rule_shell(rule, text)
            scores = {}
            # Corrupted gold CSV cells must not read as "no expectations" —
            # that scores the row vacuously perfect. Record them like a
            # parse failure (same policy as check_gold.py's hard finding).
            gold_errs = []

            tool_names = (row["expected_tool"],) + PACKAGE_ALIASES.get(row["expected_tool"], ())
            scores["tool_present"] = 1.0 if any(common.name_present(name, shell + " " + text) for name in tool_names) else 0.0

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
            except json.JSONDecodeError as exc:
                params = []
                gold_errs.append(f"expected_key_params: {exc}")
            if params:
                scores["key_params"] = round(
                    sum(1 for p in params if param_present(p, shell)) / len(params),
                    3,
                )

            try:
                expected_in = json.loads(row["expected_inputs"])
                expected_out = json.loads(row["expected_outputs"])
            except json.JSONDecodeError as exc:
                expected_in, expected_out = [], []
                gold_errs.append(f"expected_inputs/outputs: {exc}")
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
            except json.JSONDecodeError as exc:
                resource_range = {}
                gold_errs.append(f"resource_range: {exc}")
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

            scores["overall"] = mean(list(scores.values())) if scores else 0.0
            results.append(
                {
                    "id": row["id"],
                    "trial": capture["trial"],
                    "layer": "rule",
                    **scores,
                    "perfect": 1.0 if scores and all(v == 1.0 for v in scores.values()) else 0.0,
                    "capture_path": capture["path"],
                    "capture_source": capture["path"],
                    "parse_err": parse_err or "",
                    "capture_error": "; ".join([meta.get("error", "")] + gold_errs).strip("; "),
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
            meta = load_meta(capture["meta"])
            if (meta.get("error") or "").strip():
                print(
                    f"WARN: {row['id']} trial {capture['trial']} capture failed "
                    f"({str(meta.get('error', ''))[:80]}) — skipped"
                )
                continue
            text, rules, parse_err = load_generated(capture["path"])
            names = [r.get("name", "") for r in rules]
            scores = {}
            # Same policy as the rule layer: a corrupted gold cell must not
            # read as "no expectations" and score the row vacuously perfect.
            gold_errs = []

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
            except json.JSONDecodeError as exc:
                expected_steps = expected_tools = expected_edges = expected_outputs = []
                gold_errs.append(f"expected_steps/tools/dag_edges/outputs: {exc}")

            if expected_steps:
                # One scientific step gets at most one generated rule. The
                # previous first-match search could reuse a helper rule for
                # several expected stages and overstate step coverage.
                name_map = common.map_expected_steps(expected_steps, names, expected_tools, rules)
                scores["step_coverage"] = round(len(name_map) / len(expected_steps), 3)
            if expected_tools:
                hits = sum(
                    1
                    for tool in expected_tools
                    if any(
                        common.name_present(candidate, text)
                        for candidate in (tool,) + PACKAGE_ALIASES.get(tool, ())
                    )
                )
                scores["tool_coverage"] = round(hits / len(expected_tools), 3)

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
                    "capture_source": capture["path"],
                    "parse_err": parse_err or "",
                    "capture_error": "; ".join([meta.get("error", "")] + gold_errs).strip("; "),
                }
            )
    return results


def build_validity_warnings(summary, gold_rows, capture_manifest):
    warnings = []
    if not capture_manifest:
        warnings.append(
            "No capture manifest is available; trial denominator and provider/knowledge provenance cannot be audited."
        )
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
    capture_rate = float(summary.get("capture_availability", {}).get("capture_success_rate", 1.0))
    if capture_rate < 1.0:
        warnings.append(
            f"Capture availability is {capture_rate:.3f}; failed/missing trials are excluded from model scores, "
            "so availability must be reported alongside every performance estimate."
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
    missing_first = [
        row["id"]
        for row in summary.get("per_item", [])
        if float(row.get("first_trial_available", 1.0)) == 0.0
    ]
    if missing_first:
        warnings.append(
            "Items without trial 1 are excluded from pass@1 (not silently failed): " + ", ".join(missing_first)
        )
    return warnings


# ── Reporting ───────────────────────────────────────────────────────────────

def report(
    results,
    gold_rows,
    out_path,
    summary_out=None,
    item_summary_out=None,
    capture_manifest=None,
    capture_manifest_path="",
    ai_judge_summary="",
):
    if not results:
        # A fully failed capture run is itself an important result. Write the
        # availability record instead of silently emitting no artifact.
        summary = {
            "benchmark": "oxo-flow-ai-eval",
            "layer": "",
            "generated_at": common.utc_now(),
            "n_results": 0,
            "n_items": 0,
            "capture_manifest_path": capture_manifest_path,
            "capture_manifest": capture_manifest,
            "capture_availability": capture_availability(gold_rows, [], capture_manifest),
            "validity_warnings": ["No successful captures were available to judge."],
        }
        common.write_json(summary_out or common.summary_json_path(out_path), summary)
        print("no rows judged — wrote capture-availability-only summary")
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
        "item_first_trial_coverage": mean([row["first_trial_available"] for row in item_rows]),
        "item_pass_at_1": mean([
            row["first_trial_perfect"]
            for row in item_rows
            if row["first_trial_available"] == 1.0
        ]),
        "by_difficulty": {},
        "by_query_type": {},
        "per_item": item_rows,
        "overall_item_ci95": bootstrap_ci([row["overall_mean"] for row in item_rows]),
        "capture_availability": capture_availability(gold_rows, results, capture_manifest),
        "ai_judge_summary": os.path.abspath(ai_judge_summary) if ai_judge_summary else "",
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
        f"mean overall {summary['overall']['overall_mean']:.3f} "
        f"(95% CI {summary['overall']['overall_ci95']['lower']:.3f}-"
        f"{summary['overall']['overall_ci95']['upper']:.3f}), "
        f"item pass@1 {summary['item_pass_at_1']:.3f}, "
        f"trial-1 coverage {summary['item_first_trial_coverage']:.3f}, "
        f"item pass@k {summary['item_pass_at_k']:.3f}, capture success "
        f"{summary['capture_availability']['capture_success_rate']:.3f} ==="
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
    ap.add_argument("--ai-judge", action="store_true", help="run advisory expert-model review")
    ap.add_argument("--ai-judge-provider", help="openai, claude, or ollama (default: AI_JUDGE_PROVIDER)")
    ap.add_argument("--ai-judge-base-url", help="judge endpoint (default: AI_JUDGE_BASE_URL)")
    ap.add_argument("--ai-judge-model", help="judge model (default: AI_JUDGE_MODEL)")
    ap.add_argument("--ai-judge-max-tokens", type=int, default=16384)
    # Thinking judges can legitimately reason for several minutes on a large
    # workflow; the default is deliberately longer than the capture timeout.
    ap.add_argument("--ai-judge-timeout", type=int, default=600)
    ap.add_argument("--ai-judge-out", help="advisory detail CSV path (default: derived from --out)")
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

    ai_judge_rows = []
    judge_provider = None
    if args.ai_judge:
        try:
            judge_provider = ai_judge.resolve_provider(
                args.ai_judge_provider,
                args.ai_judge_base_url,
                args.ai_judge_model,
            )
        except ValueError as exc:
            sys.exit(f"invalid AI judge configuration: {exc}")
        if judge_provider is None:
            sys.exit(
                "AI judge is not configured: set AI_JUDGE_PROVIDER, AI_JUDGE_BASE_URL, "
                "AI_JUDGE_MODEL and AI_JUDGE_API_KEY (Ollama needs no key)"
            )
        print(
            f"Running advisory AI judge with {judge_provider['kind']}:{judge_provider['model']} "
            "(not part of the deterministic score of record)"
        )
        ai_judge_rows = ai_judge.evaluate_results(
            results,
            args.layer,
            gold_rows,
            args.captures,
            judge_provider,
            max_tokens=args.ai_judge_max_tokens,
            timeout=args.ai_judge_timeout,
        )
        judge_detail = args.ai_judge_out or ai_judge_detail_path(args.out)
        write_csv(judge_detail, ai_judge_rows)
        judge_summary = ai_judge.summarize(ai_judge_rows, judge_provider)
        common.write_json(
            ai_judge_summary_path(args.out),
            judge_summary,
        )
        print(f"Wrote advisory AI judge detail to {judge_detail}")

    report(
        results,
        gold_rows,
        args.out,
        summary_out=args.summary_out or common.summary_json_path(args.out),
        item_summary_out=args.item_summary_out or common.item_summary_csv_path(args.out),
        capture_manifest=capture_manifest,
        capture_manifest_path=capture_manifest_path,
        ai_judge_summary=ai_judge_summary_path(args.out) if args.ai_judge else "",
    )
    # Advisory-judge failure is nonzero because the requested report is
    # incomplete, but it must never prevent deterministic artifacts above from
    # being written first.
    if args.ai_judge and not judge_summary["n_reviewed"]:
        sys.exit("advisory AI judge produced no valid reviews; deterministic results were still written")
if __name__ == "__main__":
    main()
