#!/usr/bin/env python3
"""Independent, advisory AI judging for the three eval layers.

The deterministic judge remains the benchmark score of record.  This module
adds a separate expert-model review so scientific validity that structural
metrics cannot see (for example, an invalid shell flag inside valid TOML) is
still surfaced.  Model and endpoint are supplied per run; no API key is ever
accepted on the command line or written to an artifact.

Environment:
  AI_JUDGE_PROVIDER   openai, claude, or ollama
  AI_JUDGE_BASE_URL   provider endpoint (OpenAI-compatible proxy, etc.)
  AI_JUDGE_API_KEY    API key (not required for Ollama)
  AI_JUDGE_MODEL      judge model name
"""

import csv
import json
import os
import re
import sys
from urllib.parse import urlsplit

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import common  # noqa: E402

DIMENSIONS = (
    "scientific_correctness",
    "completeness",
    "engine_conformance",
    "usability",
)

SYSTEM_PROMPT = (
    "You are an independent senior bioinformatics and workflow-engine reviewer. "
    "Evaluate the candidate solely against the user request; do not assume or "
    "request a reference answer. Do not run commands. Identify only defects you "
    "can justify from the artifact. Score each dimension from 0 (unusable) to "
    "10 (expert-quality). Return ONLY one JSON object with this exact shape: "
    '{"scientific_correctness":{"score":0,"rationale":""},'
    '"completeness":{"score":0,"rationale":""},'
    '"engine_conformance":{"score":0,"rationale":""},'
    '"usability":{"score":0,"rationale":""},'
    '"confidence":"low|medium|high","major_risks":[]}. '
    "scientific_correctness means biology, command semantics, dependencies and "
    "data flow. completeness means every requested step/output is present. "
    "engine_conformance means answer/TOML structure, explicit inputs and "
    "outputs, environments and resource declarations. usability/reproducibility "
    "means a competent user could run or apply the artifact without guessing."
)


def resolve_provider(provider=None, base_url=None, model=None):
    """Resolve an explicit judge provider; credentials come only from env."""
    kind = (provider or os.environ.get("AI_JUDGE_PROVIDER", "")).strip().lower()
    if not kind:
        return None
    endpoint = base_url or os.environ.get("AI_JUDGE_BASE_URL", "")
    api_key = os.environ.get("AI_JUDGE_API_KEY", "")
    if kind == "claude" and not endpoint:
        endpoint = os.environ.get("AI_JUDGE_ANTHROPIC_BASE_URL", "")
    if kind == "openai" and not endpoint:
        endpoint = os.environ.get("AI_JUDGE_OPENAI_BASE_URL", "")
    selected_model = model or os.environ.get("AI_JUDGE_MODEL", "")
    if not endpoint or not selected_model:
        return None
    parsed = urlsplit(endpoint)
    if parsed.username or parsed.password or parsed.query or parsed.fragment:
        raise ValueError(
            "AI judge endpoint must not contain userinfo, query parameters, or "
            "fragments because endpoint metadata is persisted for provenance"
        )
    if kind != "ollama":
        api_key = api_key.strip()
        if not api_key:
            return None
    else:
        api_key = ""
    return {"kind": kind, "api_url": endpoint, "api_key": api_key, "model": selected_model}


def public_provider(provider):
    return {
        "kind": provider["kind"],
        "api_url": provider["api_url"],
        "model": provider["model"],
    }


def task_for(layer, gold_row):
    if layer == "tool":
        return gold_row.get("query", "")
    if layer == "rule":
        task = gold_row.get("task_description", "")
        context = gold_row.get("context_note", "")
        return f"{task}\nConstraints: {context}" if context else task
    return gold_row.get("requirement_text", "")


def artifact_for(layer, result, captures_path):
    if layer == "tool":
        with open(captures_path, newline="", encoding="utf-8") as fh:
            for row in csv.DictReader(fh):
                if row.get("id") == result["id"] and common.parse_trial(row.get("trial")) == int(result["trial"]):
                    return row.get("answer", ""), row.get("error", "")
        return "", "capture row not found"
    path = result.get("capture_path", "")
    if not path or not os.path.isfile(path):
        return "", "capture file not found"
    with open(path, encoding="utf-8") as fh:
        return fh.read(), ""


def _json_object(text):
    """Decode a strict JSON object, tolerating one ordinary markdown fence."""
    stripped = text.strip()
    if stripped.startswith("```"):
        stripped = re.sub(r"^```(?:json)?\s*", "", stripped)
        stripped = re.sub(r"\s*```$", "", stripped)
    try:
        value = json.loads(stripped)
    except json.JSONDecodeError:
        start, end = stripped.find("{"), stripped.rfind("}")
        if start < 0 or end <= start:
            raise ValueError("no JSON object found")
        value = json.loads(stripped[start : end + 1])
    if not isinstance(value, dict):
        raise ValueError("judge response is not a JSON object")
    return value


def normalize_review(raw):
    normalized = {}
    rationales = []
    for dimension in DIMENSIONS:
        entry = raw.get(dimension)
        if not isinstance(entry, dict):
            raise ValueError(f"missing or invalid {dimension}")
        score = entry.get("score")
        rationale = str(entry.get("rationale", "")).strip()
        if not isinstance(score, (int, float)) or isinstance(score, bool) or not 0 <= float(score) <= 10:
            raise ValueError(f"invalid {dimension} score")
        if not rationale:
            raise ValueError(f"missing {dimension} rationale")
        normalized[dimension] = round(float(score), 3)
        rationales.append(f"{dimension}: {rationale}")

    confidence = str(raw.get("confidence", "")).strip().lower()
    if confidence not in {"low", "medium", "high"}:
        raise ValueError("invalid confidence")
    risks = raw.get("major_risks", [])
    if not isinstance(risks, list) or not all(isinstance(item, str) for item in risks):
        raise ValueError("major_risks must be a list of strings")
    normalized["ai_judge_overall"] = round(sum(normalized[d] for d in DIMENSIONS) / len(DIMENSIONS), 3)
    normalized["confidence"] = confidence
    normalized["major_risks_json"] = json.dumps(risks, sort_keys=True, ensure_ascii=False)
    normalized["rationale"] = "\n".join(rationales)
    return normalized


def evaluate_results(results, layer, gold_rows, captures_path, provider, max_tokens=8192, timeout=300):
    """Return one advisory row per result; API failures do not alter scores."""
    gold_by_id = {row["id"]: row for row in gold_rows}
    judged = []
    for result in results:
        gold = gold_by_id[result["id"]]
        artifact, capture_error = artifact_for(layer, result, captures_path)
        row = {
            "id": result["id"],
            "trial": result["trial"],
            "layer": layer,
            "scientific_correctness": "",
            "completeness": "",
            "engine_conformance": "",
            "usability": "",
            "ai_judge_overall": "",
            "confidence": "",
            "major_risks_json": "",
            "rationale": "",
            "response_id": "",
            "response_model": "",
            "judge_error": capture_error,
        }
        if not row["judge_error"]:
            messages = [
                {"role": "system", "content": SYSTEM_PROMPT},
                {
                    "role": "user",
                    "content": (
                        f"Layer: {layer}\n\nUser request:\n{task_for(layer, gold)}\n\n"
                        f"Candidate under review:\n{artifact}\n\n"
                        + (
                            "Layer-specific interpretation: for tool answers, "
                            "engine_conformance means exactness, clearly stated scope/source "
                            "and appropriate uncertainty; do NOT require TOML. Completeness "
                            "means the answer resolves every explicit part of the question. "
                            "Usability means the user gets an unambiguous actionable answer, "
                            "not an unprompted workflow."
                            if layer == "tool"
                            else "Layer-specific interpretation: engine_conformance requires "
                            "valid .oxoflow TOML with explicit rule inputs/outputs, "
                            "environments and resources appropriate to the request."
                        )
                    ),
                },
            ]
            try:
                response = common.chat(
                    messages,
                    provider,
                    max_tokens=max_tokens,
                    temperature=0.0,
                    seed=172,
                    timeout=timeout,
                )
                row["response_id"] = response.get("meta", {}).get("response_id", "")
                row["response_model"] = response.get("meta", {}).get("response_model", "")
                normalized = normalize_review(_json_object(response["content"]))
                row.update(normalized)
            except Exception as exc:
                row["judge_error"] = str(exc)
        judged.append(row)
    return judged


def summarize(rows, provider, advisory=True):
    valid = [row for row in rows if not row.get("judge_error")]
    dimensions = {}
    for dimension in DIMENSIONS:
        values = [float(row[dimension]) for row in valid if row.get(dimension) != ""]
        dimensions[dimension] = {
            "mean": round(sum(values) / len(values), 3) if values else 0.0,
            "n": len(values),
        }
    overall = [float(row["ai_judge_overall"]) for row in valid if row.get("ai_judge_overall") != ""]
    return {
        "advisory": advisory,
        "score_of_record": "deterministic_runner",
        "provider": public_provider(provider) if provider else None,
        "n_results": len(rows),
        "n_reviewed": len(valid),
        "review_success_rate": round(len(valid) / len(rows), 3) if rows else 0.0,
        "errors": [row["judge_error"] for row in rows if row.get("judge_error")],
        "overall_mean": round(sum(overall) / len(overall), 3) if overall else 0.0,
        "dimensions": dimensions,
    }
