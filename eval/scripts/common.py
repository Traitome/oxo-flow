"""Shared helpers for the oxo-flow AI eval harness (stdlib only).

Conventions mirror scripts/refresh-knowledge/: Python stdlib only, no
third-party packages, deterministic behavior.
"""

import csv
import gzip
import json
import os
import re
import subprocess
import sys
import urllib.error
import urllib.request

REPO_ROOT = os.path.abspath(os.path.join(os.path.dirname(__file__), "..", ".."))
GOLD_DIR = os.path.join(REPO_ROOT, "eval", "gold")
KNOWLEDGE_DIR = os.path.join(REPO_ROOT, "crates", "oxo-flow-ai", "src", "knowledge")

DEFAULT_DEEPSEEK_URL = "https://api.deepseek.com/v1"
DEFAULT_OPENAI_URL = "https://api.openai.com/v1"


# ── Gold-set loading ────────────────────────────────────────────────────────

def load_gold(layer, include_unreviewed=False):
    """Load a gold CSV, optionally restricted to approved rows."""
    path = os.path.join(GOLD_DIR, f"{layer}.csv")
    with open(path, newline="", encoding="utf-8") as fh:
        rows = list(csv.DictReader(fh))
    if include_unreviewed:
        return rows
    approved = [r for r in rows if r.get("review_status") == "approved"]
    skipped = len(rows) - len(approved)
    if skipped:
        print(
            f"note: skipping {skipped} unreviewed {layer} row(s); "
            f"pass --include-unreviewed to judge them anyway"
        )
    return approved


# ── Knowledge helpers (the judge's source of truth) ─────────────────────────

def load_jsonl(name):
    rows = []
    with open(os.path.join(KNOWLEDGE_DIR, name), encoding="utf-8") as fh:
        for line in fh:
            line = line.strip()
            if not line or line.startswith("#"):
                continue
            rows.append(json.loads(line))
    return rows


def bioconda_versions():
    """Tool name -> version from the embedded bioconda table."""
    versions = {}
    for row in load_jsonl("bioconda_tools.jsonl"):
        if row.get("n") and row.get("v"):
            versions[row["n"]] = row["v"]
    return versions


def gallery_version_pins():
    """Tool -> set of versions pinned in examples/gallery/envs/*.yaml."""
    pins = {}
    envs_dir = os.path.join(REPO_ROOT, "examples", "gallery", "envs")
    if not os.path.isdir(envs_dir):
        return pins
    for name in os.listdir(envs_dir):
        if not name.endswith(".yaml"):
            continue
        with open(os.path.join(envs_dir, name), encoding="utf-8") as fh:
            for line in fh:
                entry = line.strip().lstrip("- ").strip()
                if "=" in entry:
                    tool, _, ver = entry.partition("=")
                    tool = tool.strip()
                    if tool and ver.strip():
                        pins.setdefault(tool, set()).add(ver.strip())
    return pins


def known_version_pins(kb_versions):
    """Union of gallery env pins and the KB latest version, per tool.

    The rule-layer version judge credits a pin that matches the gold
    reference pin or any version this union attests — so correct older
    pins (gallery fastp 0.23.4) are not penalized against the KB latest
    (1.3.6), while fabricated pins still score zero.
    """
    pins = gallery_version_pins()
    for tool, ver in kb_versions.items():
        pins.setdefault(tool, set()).add(ver)
    return pins


# ── AI provider resolution (mirrors the CLI: env > ~/.oxo-flow/ai_config.json) ──

def resolve_provider():
    """Return (api_url, api_key, model) or None when unconfigured."""
    kind = os.environ.get("OXO_FLOW_AI_PROVIDER", "")
    api_key = os.environ.get(f"{kind.upper()}_API_KEY", "") if kind else ""
    api_url = os.environ.get("OXO_FLOW_AI_API_URL", "")
    model = os.environ.get("OXO_FLOW_AI_MODEL", "")

    if not api_key:
        path = os.path.expanduser("~/.oxo-flow/ai_config.json")
        if not os.path.exists(path):
            return None
        with open(path, encoding="utf-8") as fh:
            cfg = json.load(fh)
        kind = kind or cfg.get("provider", "")
        api_key = cfg.get("api_key", "")
        api_url = api_url or cfg.get("api_url", "")
        model = model or cfg.get("model", "")

    if not kind or not api_key:
        return None
    if not api_url:
        api_url = DEFAULT_DEEPSEEK_URL if kind == "deepseek" else DEFAULT_OPENAI_URL
    return api_url, api_key, model or "default"


def chat(messages, api_url, api_key, model, max_tokens=2048, temperature=0.2):
    """One chat-completion call against an OpenAI-compatible endpoint."""
    body = json.dumps(
        {
            "model": model,
            "messages": messages,
            "max_tokens": max_tokens,
            "temperature": temperature,
        }
    ).encode()
    req = urllib.request.Request(
        f"{api_url.rstrip('/')}/chat/completions",
        data=body,
        headers={
            "Content-Type": "application/json",
            "Authorization": f"Bearer {api_key}",
        },
    )
    try:
        with urllib.request.urlopen(req, timeout=120) as resp:
            data = json.load(resp)
        return data["choices"][0]["message"]["content"].strip()
    except (urllib.error.URLError, KeyError, IndexError, json.JSONDecodeError) as exc:
        raise RuntimeError(f"chat completion failed: {exc}") from exc


# ── Text-matching helpers ───────────────────────────────────────────────────

def norm(text):
    """Lowercase, keep only alphanumerics (dash/underscore/slash collapse)."""
    text = re.sub(r"[-_/]", "", text.lower())
    return re.sub(r"[^a-z0-9]", "", text)


def name_present(name, text):
    """The normalized tool name occurs in the text."""
    return norm(name) in norm(text)


def loose_step_match(gold_step, candidate_name):
    """Loose match between an expected step name and a generated rule name.

    True when one normalized name contains the other (e.g. gold
    "star_align" vs generated "star_alignment").
    """
    g, c = norm(gold_step), norm(candidate_name)
    return bool(g) and bool(c) and (g in c or c in g)


def wildcard_norm(path):
    """Normalize a path pattern: {tokens} -> X, strip directories."""
    path = re.sub(r"\{[^}]*\}", "X", path)
    base = os.path.basename(path)
    return norm(base) if base else norm(path)


# ── Process helpers ─────────────────────────────────────────────────────────

def run(cmd, cwd=None, timeout=300):
    """Run a command; return (returncode, stdout, stderr)."""
    try:
        proc = subprocess.run(
            cmd, cwd=cwd, capture_output=True, text=True, timeout=timeout
        )
        return proc.returncode, proc.stdout, proc.stderr
    except (OSError, subprocess.TimeoutExpired) as exc:
        return -1, "", str(exc)


def oxo_flow_cmd(bin_path, args, cwd=None):
    return run([bin_path] + args, cwd=cwd)
