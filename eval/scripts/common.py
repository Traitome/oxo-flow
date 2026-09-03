"""Shared helpers for the oxo-flow AI eval harness (stdlib only).

Conventions mirror scripts/refresh-knowledge/: Python stdlib only, no
third-party packages, deterministic behavior.
"""

import csv
import json
import os
import re
import subprocess
import urllib.error
import urllib.request

REPO_ROOT = os.path.abspath(os.path.join(os.path.dirname(__file__), "..", ".."))
GOLD_DIR = os.path.join(REPO_ROOT, "eval", "gold")
KNOWLEDGE_DIR = os.path.join(REPO_ROOT, "crates", "oxo-flow-ai", "src", "knowledge")

DEFAULT_CLAUDE_URL = "https://api.anthropic.com"
DEFAULT_DEEPSEEK_URL = "https://api.deepseek.com/v1"
DEFAULT_OPENAI_URL = "https://api.openai.com/v1"
DEFAULT_OLLAMA_URL = "http://localhost:11434"
DEFAULT_CLAUDE_MODEL = "claude-sonnet-4-20250514"
DEFAULT_DEEPSEEK_MODEL = "deepseek-chat"
DEFAULT_OPENAI_MODEL = "gpt-4o"
DEFAULT_OLLAMA_MODEL = "llama3"


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
    """Return provider config dict or None when unconfigured/disabled."""
    kind = os.environ.get("OXO_FLOW_AI_PROVIDER", "").strip().lower()
    api_key = os.environ.get("OXO_FLOW_AI_API_KEY", "")
    api_url = os.environ.get("OXO_FLOW_AI_API_URL", "")
    model = os.environ.get("OXO_FLOW_AI_MODEL", "")

    cfg = {}
    path = os.path.expanduser("~/.oxo-flow/ai_config.json")
    if os.path.exists(path):
        with open(path, encoding="utf-8") as fh:
            cfg = json.load(fh)

    if not kind:
        kind = str(cfg.get("provider", "")).strip().lower()
    if kind in ("", "disabled"):
        return None

    if kind == "claude":
        api_key = api_key or os.environ.get("ANTHROPIC_AUTH_TOKEN", "") or cfg.get("api_key", "")
        api_url = api_url or os.environ.get("ANTHROPIC_BASE_URL", "") or cfg.get("api_url", "")
        model = model or os.environ.get("ANTHROPIC_MODEL", "") or cfg.get("model", "")
        api_url = api_url or DEFAULT_CLAUDE_URL
        model = model or DEFAULT_CLAUDE_MODEL
    elif kind == "openai":
        api_key = api_key or os.environ.get("OPENAI_API_KEY", "") or cfg.get("api_key", "")
        api_url = api_url or os.environ.get("OPENAI_BASE_URL", "") or cfg.get("api_url", "")
        model = model or os.environ.get("OPENAI_MODEL", "") or cfg.get("model", "")
        api_url = api_url or DEFAULT_OPENAI_URL
        model = model or DEFAULT_OPENAI_MODEL
    elif kind == "deepseek":
        api_key = api_key or os.environ.get("DEEPSEEK_API_KEY", "") or cfg.get("api_key", "")
        api_url = api_url or os.environ.get("DEEPSEEK_BASE_URL", "") or cfg.get("api_url", "")
        model = model or os.environ.get("DEEPSEEK_MODEL", "") or cfg.get("model", "")
        api_url = api_url or DEFAULT_DEEPSEEK_URL
        model = model or DEFAULT_DEEPSEEK_MODEL
    elif kind == "ollama":
        api_url = api_url or os.environ.get("OLLAMA_HOST", "") or cfg.get("api_url", "")
        model = model or os.environ.get("OLLAMA_MODEL", "") or cfg.get("model", "")
        api_url = api_url or DEFAULT_OLLAMA_URL
        model = model or DEFAULT_OLLAMA_MODEL
        api_key = ""
    else:
        return None

    if kind != "ollama" and not api_key:
        return None
    return {"kind": kind, "api_url": api_url, "api_key": api_key, "model": model}


def _json_request(url, body, headers, timeout=120):
    req = urllib.request.Request(
        url,
        data=json.dumps(body).encode(),
        headers=headers,
    )
    with urllib.request.urlopen(req, timeout=timeout) as resp:
        return json.load(resp)


def _ensure_openai_chat_url(api_url):
    api_url = api_url.rstrip("/")
    if api_url.endswith("/chat/completions"):
        return api_url
    if api_url.endswith("/v1"):
        return api_url + "/chat/completions"
    return api_url + "/v1/chat/completions"


def _ensure_claude_messages_url(api_url):
    api_url = api_url.rstrip("/")
    if api_url.endswith("/v1/messages"):
        return api_url
    if api_url.endswith("/v1"):
        return api_url + "/messages"
    return api_url + "/v1/messages"


def _openai_messages(messages):
    return [{"role": m["role"], "content": m["content"]} for m in messages]


def _claude_payload(messages, model, max_tokens, temperature):
    system_parts = [m["content"] for m in messages if m.get("role") == "system"]
    convo = [
        {"role": m["role"], "content": m["content"]}
        for m in messages
        if m.get("role") in ("user", "assistant")
    ]
    return {
        "model": model,
        "system": "\n\n".join(system_parts),
        "messages": convo,
        "max_tokens": max_tokens,
        "temperature": temperature,
    }


def chat(messages, provider, max_tokens=2048, temperature=0.2):
    """One non-streaming chat call against the configured provider."""
    kind = provider["kind"]
    api_url = provider["api_url"]
    api_key = provider["api_key"]
    model = provider["model"]
    try:
        if kind == "claude":
            data = _json_request(
                _ensure_claude_messages_url(api_url),
                _claude_payload(messages, model, max_tokens, temperature),
                {
                    "Content-Type": "application/json",
                    "x-api-key": api_key,
                    "anthropic-version": "2023-06-01",
                },
            )
            blocks = data.get("content") or []
            text = "".join(block.get("text", "") for block in blocks if block.get("type") == "text")
            if not text:
                raise KeyError("missing Claude text content")
            return text.strip()

        if kind == "ollama":
            data = _json_request(
                api_url.rstrip("/") + "/api/chat",
                {
                    "model": model,
                    "messages": _openai_messages(messages),
                    "stream": False,
                    "options": {"temperature": temperature},
                },
                {"Content-Type": "application/json"},
            )
            return data["message"]["content"].strip()

        data = _json_request(
            _ensure_openai_chat_url(api_url),
            {
                "model": model,
                "messages": _openai_messages(messages),
                "max_tokens": max_tokens,
                "temperature": temperature,
            },
            {
                "Content-Type": "application/json",
                "Authorization": "Bearer " + api_key,
            },
        )
        return data["choices"][0]["message"]["content"].strip()
    except (urllib.error.URLError, KeyError, IndexError, json.JSONDecodeError) as exc:
        raise RuntimeError(f"{kind} chat completion failed: {exc}") from exc


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
