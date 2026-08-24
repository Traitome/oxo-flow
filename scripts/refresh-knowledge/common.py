#!/usr/bin/env python3
"""Shared helpers for the oxo-flow knowledge-refresh scripts (issue #153).

Every script in this directory uses ONLY the Python 3 standard library
(urllib for networking — no pip dependencies), writes output atomically
via temp-file + rename, and reports progress on stdout.
"""

import json
import os
import sys
import time
import urllib.error
import urllib.request

USER_AGENT = "oxo-flow-knowledge-refresh/1.0 (issue #153)"
META_FILENAME = "knowledge_meta.json"


def log(msg: str) -> None:
    """Print a timestamped progress line."""
    print(f"[{time.strftime('%H:%M:%S')}] {msg}", flush=True)


def human_size(n: int) -> str:
    """Format a byte count for logs."""
    for unit in ("B", "KiB", "MiB", "GiB"):
        if n < 1024 or unit == "GiB":
            return f"{n:.1f} {unit}" if unit != "B" else f"{n} B"
        n /= 1024
    return f"{n} B"


def http_get(
    url: str,
    timeout: int = 120,
    retries: int = 3,
    backoff: float = 2.0,
    headers: dict | None = None,
    accept: str | None = None,
) -> bytes:
    """GET a URL with retries. Returns raw bytes or raises RuntimeError."""
    hdrs = {"User-Agent": USER_AGENT}
    if headers:
        hdrs.update(headers)
    if accept:
        hdrs["Accept"] = accept
    last = None
    for attempt in range(1, retries + 1):
        try:
            req = urllib.request.Request(url, headers=hdrs)
            with urllib.request.urlopen(req, timeout=timeout) as resp:
                return resp.read()
        except (urllib.error.URLError, urllib.error.HTTPError, OSError, TimeoutError) as e:
            last = e
            if attempt < retries:
                time.sleep(backoff * attempt)
    raise RuntimeError(f"GET {url} failed after {retries} attempts: {last}")


def http_get_json(url: str, **kw) -> dict:
    """GET a URL and parse the response as JSON."""
    raw = http_get(url, **kw)
    return json.loads(raw.decode("utf-8"))


def http_post_json(url: str, payload: dict, timeout: int = 180, retries: int = 4) -> dict:
    """POST a JSON-RPC-style payload; parse the JSON response."""
    body = json.dumps(payload).encode("utf-8")
    last = None
    for attempt in range(1, retries + 1):
        try:
            req = urllib.request.Request(
                url,
                data=body,
                headers={
                    "User-Agent": USER_AGENT,
                    "Content-Type": "application/json",
                    # The SkillGraph MCP server answers plain JSON; the SSE
                    # accept header keeps streaming-capable servers happy.
                    "Accept": "application/json, text/event-stream",
                },
            )
            with urllib.request.urlopen(req, timeout=timeout) as resp:
                return json.loads(resp.read().decode("utf-8"))
        except (urllib.error.URLError, urllib.error.HTTPError, OSError, TimeoutError) as e:
            last = e
            if attempt < retries:
                time.sleep(2.0 * attempt)
    raise RuntimeError(f"POST {url} failed after {retries} attempts: {last}")


def ensure_out_dir(out_dir: str) -> None:
    """Create the output directory if it does not exist."""
    os.makedirs(out_dir, exist_ok=True)


def write_text_atomic(path: str, text: str) -> None:
    """Write text to `path` atomically (temp file + rename)."""
    tmp = f"{path}.tmp.{os.getpid()}"
    with open(tmp, "w", encoding="utf-8") as f:
        f.write(text)
    os.replace(tmp, path)


def write_jsonl(out_dir: str, name: str, rows: list) -> tuple[str, int, int]:
    """Write `rows` as compact JSONL to `out_dir/name`. Returns (path, count, size)."""
    ensure_out_dir(out_dir)
    path = os.path.join(out_dir, name)
    lines = [json.dumps(r, ensure_ascii=False, separators=(",", ":")) for r in rows]
    write_text_atomic(path, "\n".join(lines) + "\n")
    return path, len(rows), os.path.getsize(path)


def load_meta(out_dir: str) -> dict:
    """Load the (optional) knowledge_meta.json accumulator."""
    path = os.path.join(out_dir, META_FILENAME)
    if os.path.exists(path):
        try:
            with open(path, encoding="utf-8") as f:
                return json.load(f)
        except (json.JSONDecodeError, OSError):
            return {}
    return {}


# Data file for each source key; defaults to "<key>.jsonl".
# The key doubles as the source NAME — consumers (lookup_tool freshness
# lines, `ai status`, the release gate) address sources by these names.
DATA_FILES = {
    "bioconda_tools": "bioconda_tools.jsonl",
    "skills_index": "skills_index.jsonl",
    "pipeline_graph": "pipeline_graph.jsonl",
    "nfcore_modules": "nfcore_modules.jsonl",
    "commercial_tools": "commercial_tools.jsonl",
    "biotools_overlay": "biotools_overlay.jsonl",
    "edam_terms": "edam_terms.jsonl",
}


def update_meta(out_dir: str, key: str, value: dict) -> None:
    """Merge `value` into the sources.<key> section of knowledge_meta.json.

    Every source record carries the fields the runtime meta loader
    (crates/oxo-flow-ai/src/knowledge/meta.rs) consumes: data_file, count,
    generated_at (RFC 3339 UTC) and auto. Extra fields (url, stats, ...)
    are preserved for humans and the pipeline itself.
    """
    meta = load_meta(out_dir)
    now = time.strftime("%Y-%m-%dT%H:%M:%SZ", time.gmtime())
    record = dict(value)
    record.setdefault("description", "")
    record.setdefault("data_file", DATA_FILES.get(key, f"{key}.jsonl"))
    record.setdefault("generated_at", now)
    record.setdefault("auto", True)
    meta.setdefault("sources", {})[key] = record
    meta["generated_at"] = now
    path = os.path.join(out_dir, META_FILENAME)
    write_text_atomic(path, json.dumps(meta, indent=2, ensure_ascii=False) + "\n")


def default_out_dir() -> str:
    """Default output dir: this repo's crates/oxo-flow-ai/src/knowledge/."""
    return os.path.abspath(
        os.path.join(
            os.path.dirname(os.path.abspath(__file__)),
            "..",
            "..",
            "crates",
            "oxo-flow-ai",
            "src",
            "knowledge",
        )
    )
