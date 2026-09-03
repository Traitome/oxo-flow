"""Shared helpers for the oxo-flow AI eval harness (stdlib only).

Conventions mirror scripts/refresh-knowledge/: Python stdlib only, no
third-party packages, deterministic behavior.
"""

import csv
import hashlib
import json
import os
import re
import subprocess
import urllib.error
import urllib.request
from datetime import datetime, timezone

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


# ── Generic helpers ──────────────────────────────────────────────────────────

def utc_now():
    return datetime.now(timezone.utc).replace(microsecond=0).isoformat().replace("+00:00", "Z")


def sha256_text(text):
    return hashlib.sha256(text.encode("utf-8")).hexdigest()


def sha256_file(path):
    h = hashlib.sha256()
    with open(path, "rb") as fh:
        for chunk in iter(lambda: fh.read(65536), b""):
            h.update(chunk)
    return h.hexdigest()


def gold_csv_path(layer, override=None):
    return os.path.abspath(override or os.path.join(GOLD_DIR, f"{layer}.csv"))


def manifest_path(out_path):
    out_path = os.path.abspath(out_path)
    if out_path.endswith(os.sep) or (not os.path.splitext(out_path)[1]):
        return os.path.join(out_path, "manifest.json")
    stem, _ = os.path.splitext(out_path)
    return stem + ".manifest.json"


def summary_json_path(out_path):
    out_path = os.path.abspath(out_path)
    stem, ext = os.path.splitext(out_path)
    return stem + ".summary.json" if ext else os.path.join(out_path, "summary.json")


def item_summary_csv_path(out_path):
    out_path = os.path.abspath(out_path)
    stem, ext = os.path.splitext(out_path)
    return stem + ".items.csv" if ext else os.path.join(out_path, "items.csv")


def write_json(path, data):
    os.makedirs(os.path.dirname(os.path.abspath(path)), exist_ok=True)
    with open(path, "w", encoding="utf-8") as fh:
        json.dump(data, fh, indent=2, sort_keys=True)
        fh.write("\n")


def repo_commit():
    code, out, _ = run(["git", "rev-parse", "HEAD"], cwd=REPO_ROOT, timeout=30)
    return out.strip() if code == 0 else "unknown"


def repo_dirty():
    code, out, _ = run(["git", "status", "--porcelain"], cwd=REPO_ROOT, timeout=30)
    return bool(out.strip()) if code == 0 else None


def knowledge_digest():
    h = hashlib.sha256()
    for root, _, files in os.walk(KNOWLEDGE_DIR):
        for name in sorted(files):
            path = os.path.join(root, name)
            rel = os.path.relpath(path, KNOWLEDGE_DIR).replace(os.sep, "/")
            h.update(rel.encode("utf-8"))
            with open(path, "rb") as fh:
                for chunk in iter(lambda: fh.read(65536), b""):
                    h.update(chunk)
    return h.hexdigest()


def provider_public_config(provider):
    return {
        "kind": provider["kind"],
        "api_url": provider["api_url"],
        "model": provider["model"],
    }


# ── Gold-set loading ────────────────────────────────────────────────────────

def load_gold(layer, include_unreviewed=False, gold_path=None):
    """Load a gold CSV, optionally restricted to approved rows."""
    path = gold_csv_path(layer, gold_path)
    with open(path, newline="", encoding="utf-8") as fh:
        rows = list(csv.DictReader(fh))
    if include_unreviewed:
        if not rows:
            raise SystemExit(f"no {layer} rows found in {path}")
        return rows
    approved = [r for r in rows if r.get("review_status") == "approved"]
    skipped = len(rows) - len(approved)
    if skipped:
        print(
            f"note: skipping {skipped} unreviewed {layer} row(s); "
            f"pass --include-unreviewed to judge them anyway"
        )
    if not approved:
        raise SystemExit(
            f"no approved {layer} gold rows found in {path}; complete human review "
            f"or pass --include-unreviewed for preview-only runs"
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


def known_tool_names():
    names = set()
    for file_name in (
        "bioconda_tools.jsonl",
        "commercial_tools.jsonl",
        "biotools_overlay.jsonl",
        "nfcore_modules.jsonl",
    ):
        for row in load_jsonl(file_name):
            if row.get("n"):
                names.add(row["n"])
    return names


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
    """Union of gallery env pins and the KB latest version, per tool."""
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


def chat(messages, provider, max_tokens=2048, temperature=0.2, seed=None):
    """One non-streaming chat call against the configured provider."""
    kind = provider["kind"]
    api_url = provider["api_url"]
    api_key = provider["api_key"]
    model = provider["model"]
    try:
        if kind == "claude":
            payload = _claude_payload(messages, model, max_tokens, temperature)
            data = _json_request(
                _ensure_claude_messages_url(api_url),
                payload,
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
            return {
                "content": text.strip(),
                "meta": {
                    "response_id": data.get("id", ""),
                    "response_model": data.get("model", model),
                    "stop_reason": data.get("stop_reason", ""),
                    "usage": data.get("usage", {}),
                    "seed": seed,
                    "seed_supported": False,
                },
            }

        if kind == "ollama":
            options = {"temperature": temperature}
            if seed is not None:
                options["seed"] = seed
            data = _json_request(
                api_url.rstrip("/") + "/api/chat",
                {
                    "model": model,
                    "messages": _openai_messages(messages),
                    "stream": False,
                    "options": options,
                },
                {"Content-Type": "application/json"},
            )
            return {
                "content": data["message"]["content"].strip(),
                "meta": {
                    "response_model": data.get("model", model),
                    "stop_reason": data.get("done_reason", ""),
                    "usage": {
                        "prompt_eval_count": data.get("prompt_eval_count"),
                        "eval_count": data.get("eval_count"),
                        "total_duration": data.get("total_duration"),
                        "load_duration": data.get("load_duration"),
                        "prompt_eval_duration": data.get("prompt_eval_duration"),
                        "eval_duration": data.get("eval_duration"),
                    },
                    "seed": seed,
                    "seed_supported": True,
                },
            }

        body = {
            "model": model,
            "messages": _openai_messages(messages),
            "max_tokens": max_tokens,
            "temperature": temperature,
        }
        if seed is not None:
            body["seed"] = seed
        data = _json_request(
            _ensure_openai_chat_url(api_url),
            body,
            {
                "Content-Type": "application/json",
                "Authorization": "Bearer " + api_key,
            },
        )
        choice = data["choices"][0]
        return {
            "content": choice["message"]["content"].strip(),
            "meta": {
                "response_id": data.get("id", ""),
                "response_model": data.get("model", model),
                "system_fingerprint": data.get("system_fingerprint", ""),
                "stop_reason": choice.get("finish_reason", ""),
                "usage": data.get("usage", {}),
                "seed": seed,
                "seed_supported": True,
            },
        }
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


def step_tokens(text):
    return [norm(part) for part in re.split(r"[:/_.-]+", text) if norm(part)]


def loose_step_match(gold_step, candidate_name):
    """Loose but structure-aware match between expected and generated rule names."""
    g_full = norm(gold_step)
    c_full = norm(candidate_name)
    if not g_full or not c_full:
        return False
    if g_full == c_full:
        return True
    g_last = norm(gold_step.split("::")[-1])
    c_last = norm(candidate_name.split("::")[-1])
    if g_last and c_last and g_last == c_last:
        return True
    g_tokens = step_tokens(gold_step)
    c_tokens = step_tokens(candidate_name)
    if not g_tokens or not c_tokens:
        return False
    short, long = (g_tokens, c_tokens) if len(g_tokens) <= len(c_tokens) else (c_tokens, g_tokens)
    return len(short) >= 2 and all(token in long for token in short)


def path_parts(path):
    """Normalize a path pattern but keep directory structure."""
    normalized = re.sub(r"\{[^}]*\}", "X", path.replace("\\", "/"))
    parts = [norm(part) for part in normalized.split("/") if part not in ("", ".")]
    return [part for part in parts if part]


def path_matches(expected, declared):
    """True when normalized paths match exactly or by full suffix."""
    exp_parts = path_parts(expected)
    dec_parts = path_parts(declared)
    if not exp_parts or not dec_parts:
        return False
    if exp_parts == dec_parts:
        return True
    short, long = (exp_parts, dec_parts) if len(exp_parts) <= len(dec_parts) else (dec_parts, exp_parts)
    return long[-len(short):] == short


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
