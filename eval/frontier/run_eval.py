#!/usr/bin/env python3
"""AI generation benchmark for oxo-flow (issue #342 / proposal P6).

Measures the quality/cost frontier of the AI pipeline-generation paths
against the engine's three deterministic static gates:

    oxo-flow validate --json   (structure, DAG, wildcard syntax)
    oxo-flow dry-run  --json   (expansion preview, resource audit)
    oxo-flow lint     --json   (best practices)

Variants (pre-registered matched three-arm ablation, plan:109-111)
    unconstrained   Arm A: direct single-shot LLM generation, no compiler or
                    contract feedback - the prompt-axis floor, not a product
                    path. Named `minimal` before #948 P2-4 and `naive-prompt`
                    before #1315; it is a bare prompt baseline.
    syntax-only     Arm B: PipelineGenAgent (rich prompt + knowledge tools +
                    tool-calling loop) with TOML/DAG syntax + structural
                    feedback only. BioTypes (`TYPE-*`), scientific preflight
                    (`SCI-*`) and deep-check (`D00*`) feedback are DISABLED
                    (the engine's `--ai-contract-feedback` flag is omitted).
    full-contract   Arm C: PipelineGenAgent with BioTypes (`TYPE-*`) +
                    scientific_preflight (`SCI-*`) + deep_check (`D00*`)
                    feedback ENABLED (the engine is invoked with
                    `--ai-contract-feedback`).

Arms B and C share the same engine path (`oxo-flow template --ai`); they differ
by an actual feedback toggle (`--ai-contract-feedback`), never by a label.

Success is judged ONLY by the deterministic gates (never by an LLM judge),
so the numbers are cheap and the judging is deterministic; the unconstrained
arm is pinned to temperature 0 where the provider honors it but is not
fully reproducible across providers/runs (the syntax-only / full-contract arms
inherit the engine's [ai] temperature config, unset = provider default). Token
usage comes from the AI session archives (syntax-only / full-contract) or the
provider response (unconstrained); cost is derived from a configurable price
table.

Usage
    # pilot: all intents, all three arms, the model configured in the env
    python3 run_eval.py

    # subset, specific models, custom output dir
    python3 run_eval.py --filter rnaseq-star,atacseq \
        --models deepseek-chat --out results/pilot2

Credentials are read from the environment (ANTHROPIC_BASE_URL /
ANTHROPIC_AUTH_TOKEN / ANTHROPIC_MODEL), from the dedicated
~/.oxo-flow/ai_config.json, or from a pinned source with --creds-from
(auto|env|config; never from shell rc files; values are never printed).
A campaign that must not spend an ambient agent token pins
--creds-from config. Arm A speaks the Anthropic Messages protocol for a
claude provider and an OpenAI-compatible chat-completions protocol for
openai/deepseek/ollama; --arm-a-protocol can pin either explicitly. The
bare Arm A prompt itself is unchanged by the protocol switch.
"""

from __future__ import annotations

import argparse
import hashlib
import json
import os
import re
import shutil
import subprocess
import sys
import time
import urllib.request
from datetime import datetime, timezone
from pathlib import Path

REPO = Path(__file__).resolve().parents[2]

sys.path.insert(0, os.path.join(os.path.dirname(os.path.abspath(__file__)), "..", "scripts"))
import common  # noqa: E402

# USD per 1M tokens. Defaults mirror the repo's own cost constant
# (crates/oxo-flow-ai/src/types.rs: deepseek-v4-pro 0.28 in / 1.10 out,
# pricing as of 2026-08). Add per-model overrides here as needed.
# GLM/Qwen: official z.ai / Alibaba DashScope list prices (2026-09).
PRICE_PER_MTOK = {
    "GLM": (0.60, 2.20),
    "Qwen": (0.50, 2.00),
    "*": (0.28, 1.10),
}

NAIVE_SYSTEM = """You are a bioinformatics pipeline expert. Generate valid .oxoflow TOML configurations.

Intent: {intent}

Rules:
1. Output TOML in ```toml code fences
2. Use well-known bioinformatics tools with correct command-line syntax
3. Include [workflow] section with name, version, description
4. Define rules with name, shell, inputs, outputs, depends
5. Use {{sample}} wildcard for sample-varying paths
6. Specify conda environment for each rule when possible
7. Include resource hints (threads, memory) in [resources] section"""

CFG_PATH = Path.home() / ".oxo-flow" / "ai_config.json"
ANTHROPIC_CREDS_KEYS = ("ANTHROPIC_AUTH_TOKEN", "ANTHROPIC_BASE_URL", "ANTHROPIC_MODEL")

# Provider kinds the harness can drive on both generation paths. `claude`
# speaks the Anthropic Messages protocol; the other three expose an
# OpenAI-compatible chat-completions endpoint (DeepSeek natively, Ollama via
# its /v1 bridge).
PROVIDER_KINDS = ("claude", "openai", "deepseek", "ollama")

# Ambient provider credentials that must never leak into a campaign child
# process (issue #1395): a host can export an unrelated coding agent's own
# token, and a campaign must spend only the resolved oxo-flow provider.
AMBIENT_CREDENTIAL_VARS = (
    "ANTHROPIC_AUTH_TOKEN",
    "ANTHROPIC_API_KEY",
    "ANTHROPIC_BASE_URL",
    "ANTHROPIC_MODEL",
    "CLAUDE_API_KEY",
    "OPENAI_API_KEY",
    "OPENAI_BASE_URL",
    "OPENAI_MODEL",
    "DEEPSEEK_API_KEY",
    "DEEPSEEK_BASE_URL",
    "OLLAMA_HOST",
    "OLLAMA_MODEL",
    "OXO_FLOW_AI_API_KEY",
    "OXO_FLOW_AI_API_URL",
    "OXO_FLOW_AI_MODEL",
)


def _credentials_from_env() -> dict[str, str]:
    """The legacy ANTHROPIC_* environment tier (Claude Messages protocol)."""
    missing = [k for k in ANTHROPIC_CREDS_KEYS if not os.environ.get(k)]
    if missing:
        sys.exit(f"missing credentials: {', '.join(missing)} (env or {CFG_PATH})")
    return {
        "provider": "claude",
        "api_key": os.environ["ANTHROPIC_AUTH_TOKEN"],
        "api_url": os.environ["ANTHROPIC_BASE_URL"],
        "model": os.environ["ANTHROPIC_MODEL"],
    }


def _credentials_from_config() -> dict[str, str]:
    """The operator's persisted oxo-flow provider (~/.oxo-flow/ai_config.json).

    This is the same file the CLI resolves when no provider env tier is set;
    it is authoritative for a campaign that must not spend the host shell's
    unrelated credentials. Values are never printed.
    """
    if not CFG_PATH.exists():
        sys.exit(f"missing credentials: {CFG_PATH} does not exist")
    try:
        cfg = json.loads(CFG_PATH.read_text())
    except json.JSONDecodeError as exc:
        sys.exit(f"cannot parse {CFG_PATH}: {exc}")
    if not isinstance(cfg, dict):
        sys.exit(f"cannot use {CFG_PATH}: expected a JSON object")
    provider = str(cfg.get("provider") or "").strip().lower()
    if provider not in PROVIDER_KINDS:
        sys.exit(
            f"{CFG_PATH}: provider {provider!r} is not one of "
            f"{', '.join(PROVIDER_KINDS)}"
        )
    creds = {
        "provider": provider,
        "api_key": str(cfg.get("api_key") or "").strip(),
        "api_url": str(cfg.get("api_url") or "").strip(),
        "model": str(cfg.get("model") or "").strip(),
    }
    missing = [key for key in ("api_key", "api_url", "model") if not creds[key]]
    if missing:
        sys.exit(f"{CFG_PATH}: missing {', '.join(missing)}")
    return creds


def load_credentials(source: str = "auto") -> dict[str, str]:
    """Resolve provider identity, endpoint, and model.

    ``source``:
      - ``env`` — the legacy ANTHROPIC_* environment tier.
      - ``config`` — the operator's persisted oxo-flow provider. A governed
        campaign uses this so an ambient agent token cannot be spent.
      - ``auto`` — env first, then config (the historical precedence).

    Returns ``{provider, api_key, api_url, model}``. No shell-rc harvesting
    (#948 P3-12): sourcing a user's dotfile from an eval script pulls every
    exported secret into the process.
    """
    if source == "env":
        return _credentials_from_env()
    if source == "config":
        return _credentials_from_config()
    if source != "auto":
        sys.exit(f"unknown credential source: {source}")
    try:
        return _credentials_from_env()
    except SystemExit:
        return _credentials_from_config()


def find_binary() -> str:
    """Shared discovery (common.find_engine_binary); kept as an alias for rejudge.py."""
    try:
        return common.find_engine_binary()
    except FileNotFoundError as exc:
        sys.exit(str(exc))


def extract_toml(text: str) -> str | None:
    """Mirror of chat/service.rs extract_toml_from_response."""
    if "```toml" in text:
        block = text.split("```toml", 1)[1]
        if "```" in block:
            content = block.split("```", 1)[0].strip()
            if content:
                return content
    if "```" in text:
        block = text.split("```", 1)[1]
        if "```" in block:
            content = block.split("```", 1)[0].strip()
            if "[workflow]" in content:
                return content
    if "[workflow]" in text:
        return text[text.index("[workflow]"):].strip()
    return None


def anthropic_chat(creds: dict, model: str, system: str, user: str,
                   max_tokens: int, timeout: float,
                   temperature: float | None = 0.0) -> tuple[str | None, dict, str | None]:
    """One Anthropic-messages call. Returns (text, usage, error).

    temperature defaults to 0 so unconstrained-arm draws are as deterministic as
    the provider allows; the Anthropic Messages API has no seed field, so
    providers behind an Anthropic-compatible endpoint may still vary (#948
    P3-12 — only the *judging* is deterministic, never the sampling).
    """
    base = creds["api_url"].rstrip("/")
    body: dict = {
        "model": model,
        "max_tokens": max_tokens,
        "system": system,
        "messages": [{"role": "user", "content": user}],
    }
    if temperature is not None:
        body["temperature"] = temperature
    payload = json.dumps(body).encode()
    req = urllib.request.Request(
        f"{base}/v1/messages", data=payload, method="POST",
        headers={
            "content-type": "application/json",
            "anthropic-version": "2023-06-01",
            "x-api-key": creds["api_key"],
            "authorization": f"Bearer {creds['api_key']}",
        },
    )
    try:
        with urllib.request.urlopen(req, timeout=timeout) as resp:
            data = json.load(resp)
    except Exception as e:  # noqa: BLE001 — report any transport/API error verbatim
        return None, {}, str(e)[:500]
    if not isinstance(data, dict):
        return None, {}, "unexpected response shape (not a JSON object)"
    if data.get("type") == "error":
        return None, {}, data.get("error", {}).get("message", "unknown API error")[:500]
    text = "".join(b.get("text", "") for b in data.get("content", [])
                   if b.get("type") == "text")
    usage_in = data.get("usage", {}).get("input_tokens", 0)
    usage_out = data.get("usage", {}).get("output_tokens", 0)
    return text or None, {
        "input": usage_in,
        "output": usage_out,
        # The endpoint may route an alias to a different backend model; the
        # reported identity is evidence, never a substitution.
        "reported_model": data.get("model"),
    }, None


def openai_chat_url(api_url: str) -> str:
    """Resolve an OpenAI-compatible chat-completions URL.

    Mirrors ``OpenAiBackend::new_labeled`` (crates/oxo-flow-ai/src/provider.rs):
    a full ``/chat/completions`` URL passes through, a base containing
    ``/v1`` appends the suffix, any other versioned base (GLM's ``/v4``)
    appends the suffix as-is, and an unversioned base gains ``/v1`` first.
    """
    url = api_url.strip().rstrip("/")
    if "/chat/completions" in url:
        return url
    if "/v1" in url or re.search(r"/v\d", url):
        return f"{url}/chat/completions"
    return f"{url}/v1/chat/completions"


def openai_chat(creds: dict, model: str, system: str, user: str,
                max_tokens: int, timeout: float,
                temperature: float | None = 0.0) -> tuple[str | None, dict, str | None]:
    """One OpenAI-compatible chat-completions call. Returns (text, usage, error).

    This is the same wire protocol the CLI's OpenAI backend uses, so Arm A's
    bare prompt runs against the same model identity as Arms B/C when the
    operator's provider is OpenAI-compatible. The prompt is unchanged.
    """
    body = {
        "model": model,
        "messages": [
            {"role": "system", "content": system},
            {"role": "user", "content": user},
        ],
        "max_tokens": max_tokens,
    }
    if temperature is not None:
        body["temperature"] = temperature
    req = urllib.request.Request(
        openai_chat_url(creds["api_url"]),
        data=json.dumps(body).encode(),
        method="POST",
        headers={
            "content-type": "application/json",
            "authorization": f"Bearer {creds['api_key']}",
        },
    )
    try:
        with urllib.request.urlopen(req, timeout=timeout) as resp:
            data = json.load(resp)
    except Exception as e:  # noqa: BLE001 — report any transport/API error verbatim
        return None, {}, str(e)[:500]
    if not isinstance(data, dict):
        return None, {}, "unexpected response shape (not a JSON object)"
    if data.get("error"):
        error = data["error"]
        message = error.get("message") if isinstance(error, dict) else error
        return None, {}, str(message or "unknown API error")[:500]
    choices = data.get("choices") or []
    message = choices[0].get("message", {}) if choices else {}
    text = message.get("content")
    usage = data.get("usage") or {}
    return text if isinstance(text, str) and text else None, {
        "input": usage.get("prompt_tokens", 0),
        "output": usage.get("completion_tokens", 0),
        "reported_model": data.get("model"),
    }, None


def resolve_arm_a_protocol(requested: str, provider: str) -> str:
    """Pick Arm A's wire protocol; ``auto`` follows the resolved provider."""
    if requested != "auto":
        return requested
    return "anthropic" if provider == "claude" else "openai"


def build_child_env(creds: dict) -> dict[str, str]:
    """Environment for ``template --ai``, pinned to the resolved provider.

    Every ambient provider credential is scrubbed first: the host may export
    an unrelated coding agent's token, and a campaign must spend only the
    resolved oxo-flow provider (issue #1395). The generic OXO_FLOW_AI_* trio
    is honored by every provider kind, so one mapping covers claude, openai,
    deepseek, and ollama.
    """
    env = {key: value for key, value in os.environ.items()
           if key not in AMBIENT_CREDENTIAL_VARS}
    env["OXO_FLOW_AI_PROVIDER"] = creds["provider"]
    env["OXO_FLOW_AI_API_KEY"] = creds["api_key"]
    env["OXO_FLOW_AI_API_URL"] = creds["api_url"]
    env["OXO_FLOW_AI_MODEL"] = creds["model"]
    return env


ARMS = ("unconstrained", "syntax-only", "full-contract")


def template_command(variant: str, intent: str, run_dir: Path, binary: str,
                     args) -> list[str]:
    """Engine argv for Arms B/C. Arm C adds the contract-feedback toggle.

    Kept as one helper so the Arm B / Arm C distinction is testable and
    cannot silently drift: the ONLY difference is the
    ``--ai-contract-feedback`` flag.
    """
    cmd = [binary, "template", "--ai", intent, "-o", f"{run_dir}/",
           "--no-color"]
    if args.max_retries is not None:
        cmd += ["--ai-max-retries", str(args.max_retries)]
    if args.profile != "compact":
        cmd += ["--ai-team-profile", args.profile]
    if args.attempts != 1:
        cmd += ["--ai-attempts", str(args.attempts)]
    # The ONLY difference between Arm B and Arm C: whether the engine's
    # generation validator runs the BioTypes / scientific-preflight /
    # deep-check contract feedback loop (--ai-contract-feedback). Arm B
    # omits it (syntax + structural feedback only); Arm C enables it.
    if variant == "full-contract":
        cmd += ["--ai-contract-feedback"]
    return cmd


def run_generation(variant: str, intent: str, model: str, run_dir: Path,
                   creds: dict, binary: str, args) -> dict:
    """Generate a workflow. Returns usage/meta dict; writes workflow.oxoflow."""
    meta: dict = {
        "generated": False, "input_tokens": 0, "output_tokens": 0,
        "tool_calls": None, "rounds": None, "error": None,
        "provider": creds["provider"], "protocol": None,
        "requested_model": model, "reported_model": None,
        "generation_path": None, "generation_command": None,
        "contract_feedback": None,
    }
    started = time.time()
    meta["wall_s"] = None

    if variant == "unconstrained":
        protocol = resolve_arm_a_protocol(args.arm_a_protocol, creds["provider"])
        chat = anthropic_chat if protocol == "anthropic" else openai_chat
        text, usage, err = chat(
            creds, model, NAIVE_SYSTEM.format(intent=intent),
            f"## User Request\nGenerate a .oxoflow pipeline for: {intent}\n\n"
            "## Task\nGenerate the optimized .oxoflow TOML configuration now. "
            "Output inside ```toml fences.",
            max_tokens=8192, timeout=args.timeout,
        )
        meta["protocol"] = protocol
        meta["generation_path"] = "direct"
        meta["reported_model"] = usage.get("reported_model")
        meta["input_tokens"] = usage.get("input", 0)
        meta["output_tokens"] = usage.get("output", 0)
        if err:
            meta["error"] = err
            meta["wall_s"] = round(time.time() - started, 1)
            return meta
        toml = extract_toml(text or "")
        if not toml:
            meta["error"] = "no TOML found in response"
            meta["wall_s"] = round(time.time() - started, 1)
            return meta
        (run_dir / "workflow.oxoflow").write_text(toml)
        meta["generated"] = True

    elif variant in ("syntax-only", "full-contract"):
        env = build_child_env(creds)
        sessions = Path.home() / ".oxo-flow/ai_sessions"
        before = {p.name: p.stat().st_mtime for p in sessions.glob("*-template-*.json")} \
            if sessions.exists() else {}
        cmd = template_command(variant, intent, run_dir, binary, args)
        meta["protocol"] = "engine"
        meta["generation_path"] = "engine"
        meta["generation_command"] = cmd
        meta["contract_feedback"] = variant == "full-contract"
        # The argv is direct evidence that Arm C ran the contract-feedback
        # path and Arm B did not; it carries no credentials.
        (run_dir / "argv.json").write_text(json.dumps({
            "argv": cmd,
            "provider": creds["provider"],
            "requested_model": model,
            "contract_feedback": meta["contract_feedback"],
        }, indent=2))
        try:
            proc = subprocess.run(cmd, cwd=run_dir, env=env, timeout=args.timeout,
                                  capture_output=True, text=True)
            (run_dir / "cli.log").write_text(proc.stdout + "\n--- stderr ---\n" + proc.stderr)
            if proc.returncode != 0:
                meta["error"] = f"template exited {proc.returncode}"
        except subprocess.TimeoutExpired:
            meta["error"] = "generation timed out"
            meta["wall_s"] = round(time.time() - started, 1)
            return meta
        # Token accounting: ALL template sessions created by this run
        # (multi-attempt runs archive one session per attempt — the spend
        # is the sum, and quoting only the newest would undercount).
        if sessions.exists():
            new = [p for p in sessions.glob("*-template-*.json")
                   if p.stat().st_mtime > started
                   and p.name not in before]
            if new:
                snap = max(new, key=lambda p: p.stat().st_mtime)
                (run_dir / "session.json").write_text(snap.read_text())
                tin = tout = 0
                tools = 0
                for p in new:
                    try:
                        s = json.loads(p.read_text())
                        u = s.get("total_usage", {})
                        tin += u.get("prompt_tokens", 0)
                        tout += u.get("completion_tokens", 0)
                        tools += len(s.get("tool_calls", []))
                    except json.JSONDecodeError:
                        pass
                meta["input_tokens"] = tin
                meta["output_tokens"] = tout
                meta["tool_calls"] = tools
                meta["sessions"] = len(new)
                try:
                    snap_json = json.loads(snap.read_text())
                    meta["reported_model"] = snap_json.get("model") or None
                    meta["session_provider"] = snap_json.get("provider") or None
                except json.JSONDecodeError:
                    pass
        # rglob: a relative -o target resolved inside a relative run_dir can
        # nest the artifact one level deeper — find it wherever it landed.
        wf = sorted(run_dir.rglob("*.oxoflow"))
        if wf:
            meta["generated"] = True
        elif not meta["error"]:
            meta["error"] = "no .oxoflow file produced"
    else:
        sys.exit(f"unknown variant: {variant} (expected one of {', '.join(ARMS)})")

    meta["wall_s"] = round(time.time() - started, 1)
    return meta


def run_gates(binary: str, wf: Path, run_dir: Path) -> dict:
    """The three static gates. Exit 0 = pass. JSON captured separately."""
    gates = {}
    for gate in ("validate", "dry-run", "lint"):
        out = run_dir / f"{gate}.json"
        err = run_dir / f"{gate}.err"
        try:
            # Same convention as eval/scripts/runner.py: cwd = run dir, file
            # addressed by basename — safe even when `wf` is a relative path.
            proc = subprocess.run([binary, gate, "--json", wf.name],
                                  cwd=run_dir, timeout=120,
                                  capture_output=True, text=True)
        except subprocess.TimeoutExpired:
            gates[gate] = {"exit": 124, "pass": False}
            continue
        out.write_text(proc.stdout)
        err.write_text(proc.stderr)
        entry: dict = {"exit": proc.returncode, "pass": proc.returncode == 0}
        try:
            j = json.loads(proc.stdout)
            if gate == "validate":
                entry["valid"] = j.get("valid")
                entry["n_errors"] = len(j.get("errors", []))
                entry["n_missing_inputs"] = len(j.get("missing_inputs", []))
            elif gate == "lint":
                entry["n_warnings"] = len(j.get("warnings", [])) or None
        except json.JSONDecodeError:
            pass
        gates[gate] = entry
    return gates


def cost_usd(model: str, tin: int, tout: int) -> float:
    pin, pout = PRICE_PER_MTOK.get(model, PRICE_PER_MTOK["*"])
    return tin * pin / 1e6 + tout * pout / 1e6


def aggregate_report(runs: list[dict], profile: str, stamp: str, binary: str) -> str:
    """Render the campaign report.md from result rows.

    Shared by the live campaign and `--report-only` so a re-aggregation can
    never drift from the run-time one. Rows whose generation timed out carry
    `wall_s = None`; they count as 0 s here instead of poisoning the sum.
    """
    lines = ["# AI generation benchmark report", "",
             f"Stamp: `{stamp}` · binary: `{binary}` · success = all three gates exit 0", ""]
    groups: dict[tuple, list] = {}
    for r in runs:
        groups.setdefault((r["model"], r["variant"]), []).append(r)
    for (model, variant), rs in sorted(groups.items()):
        n = len(rs)
        gen_n = sum(r["generated"] for r in rs)
        v = sum(r["gates"].get("validate", {}).get("pass", False) for r in rs)
        d = sum(r["gates"].get("dry-run", {}).get("pass", False) for r in rs)
        l = sum(r["gates"].get("lint", {}).get("pass", False) for r in rs)
        allp = sum(r["all_gates_pass"] for r in rs)
        fid = sum(r["fidelity_pass"] for r in rs)
        ok = sum(r["success"] for r in rs)
        tok_in = sum(r["input_tokens"] for r in rs)
        tok_out = sum(r["output_tokens"] for r in rs)
        wall = [r.get("wall_s") or 0 for r in rs]
        cost = sum(r["est_cost_usd"] for r in rs)
        lines += [
            f"## {model} / {variant} / profile={profile}", "",
            f"- runs: {n}, generated: {gen_n}",
            f"- gate pass@1: validate {v}/{n}, dry-run {d}/{n}, lint {l}/{n}, "
            f"**all-gates {allp}/{n}** ({allp / n:.0%})",
            f"- fidelity (named tools present): {fid}/{n} — "
            f"**success (gates ∧ fidelity) {ok}/{n}** ({ok / n:.0%})",
            f"- tokens: in {tok_in:,} / out {tok_out:,} "
            f"(mean {tok_in // max(n, 1):,}/{tok_out // max(n, 1):,} per run)",
            f"- est. cost: ${cost:.4f} (price table {PRICE_PER_MTOK.get(model, PRICE_PER_MTOK['*'])}/Mtok)",
            f"- wall: mean {sum(wall) / max(n, 1):.0f}s, max {max(wall or [0]):.0f}s",
            "",
            "| tier | success (gates ∧ fidelity) | runs |",
            "|---|---|---|",
        ]
        for tier in ("easy", "medium", "hard"):
            trs = [r for r in rs if r["tier"] == tier]
            if trs:
                tp = sum(r["success"] for r in trs)
                lines.append(f"| {tier} | {tp}/{len(trs)} | {len(trs)} |")
        vague = [r for r in rs if r.get("vague")]
        if vague:
            vp = sum(r["success"] for r in vague)
            lines.append(f"| vague | {vp}/{len(vague)} | {len(vague)} |")
        lines += [
            "",
            "| intent | tier | gates | fidelity | tok in | tok out | wall s | note |",
            "|---|---|---|---|---|---|---|---|",
        ]
        for r in rs:
            note = r.get("error") or ""
            if r["missing_required_tools"]:
                note = (note + " missing:" + ",".join(r["missing_required_tools"])).strip()
            lines.append(
                f"| {r['intent_id']} | {r['tier']} | "
                f"{'✅' if r['all_gates_pass'] else '❌'} | "
                f"{'✅' if r['fidelity_pass'] else '❌'} | "
                f"{r['input_tokens']:,} | {r['output_tokens']:,} | "
                f"{r.get('wall_s', '')} | {note[:60]} |")
        lines.append("")
    return "\n".join(lines)


def sha256_file(path: Path) -> str | None:
    """Content hash for the campaign manifest (binary, harness, intents)."""
    try:
        digest = hashlib.sha256()
        with path.open("rb") as handle:
            for chunk in iter(lambda: handle.read(1 << 20), b""):
                digest.update(chunk)
        return f"sha256:{digest.hexdigest()}"
    except OSError:
        return None


BUDGET_ENV_VARS = (
    "OXO_FLOW_AI_TIMEOUT_SECS",
    "OXO_FLOW_AI_MAX_TOKENS",
    "OXO_FLOW_AI_CONTEXT_TOKENS",
)


def main() -> None:
    ap = argparse.ArgumentParser(description=__doc__.split("\n")[0])
    ap.add_argument("--intents", default=str(Path(__file__).parent / "intents.json"))
    ap.add_argument("--out", default=None, help="results directory (default: results/<stamp>)")
    ap.add_argument("--report-only", default=None, metavar="RESULTS_JSON",
                    help="rebuild report.md from an existing results.json "
                         "without calling any provider (reuses the campaign "
                         "aggregation; nothing else runs)")
    ap.add_argument("--variants", default="unconstrained,syntax-only,full-contract")
    ap.add_argument("--models", default="",
                    help="comma list; default: the resolved provider's model")
    ap.add_argument("--creds-from", default="auto", choices=["auto", "env", "config"],
                    help="credential source: env (ANTHROPIC_*), config "
                         "(~/.oxo-flow/ai_config.json), or auto (env first, "
                         "then config). A governed campaign pins `config` so "
                         "an ambient agent token cannot be spent (#1395)")
    ap.add_argument("--arm-a-protocol", default="auto",
                    choices=["auto", "anthropic", "openai"],
                    help="wire protocol for the bare Arm A prompt: auto follows "
                         "the resolved provider (claude -> anthropic, else an "
                         "OpenAI-compatible chat-completions path)")
    ap.add_argument("--campaign-note", default="",
                    help="free-text label recorded in results.json (no secrets)")
    ap.add_argument("--filter", default="", help="comma list of intent ids to include")
    ap.add_argument("--max-retries", type=int, default=None,
                    help="override the CLI tool-loop budget (--ai-max-retries); "
                         "default: whatever the binary's shipped [ai] max_retries is")
    ap.add_argument("--seeds", type=int, default=1,
                    help="repeat every cell N times (sampling variance; report is "
                         "per-intent pass-rate across seeds)")
    ap.add_argument("--binary", default=None,
                    help="pin the CLI binary for the whole campaign (default: "
                         "find_binary()); use when code may be rebuilt mid-run")
    ap.add_argument("--attempts", type=int, default=1,
                    help="fresh-draw attempts per run (--ai-attempts); pass@k "
                         "with early exit — attempt 2..N is only paid on failures")
    ap.add_argument("--profile", default="compact", choices=["compact", "full"],
                    help="team profile to benchmark (compact = single agent + "
                         "gates; full = Scientist Team roles; full needs a binary "
                         "that supports --ai-team-profile)")
    ap.add_argument("--timeout", type=float, default=900, help="per-generation timeout (s)")
    ap.add_argument("--sleep", type=float, default=1.0, help="pause between runs (s)")
    args = ap.parse_args()

    if args.report_only:
        # Re-aggregate an existing campaign without a provider call: repairs
        # a report lost to a mid-run crash or left stale by rejudge.py.
        src = Path(args.report_only)
        payload = json.loads(src.read_text())
        dst = src.parent / "report.md"
        dst.write_text(aggregate_report(
            payload["runs"],
            profile=payload.get("profile", "compact"),
            stamp=payload.get("stamp", src.parent.name),
            binary=payload.get("binary", "unknown"),
        ))
        print(f"report rebuilt: {dst}")
        return

    creds = load_credentials(args.creds_from)
    binary = args.binary or find_binary()
    if not (Path(binary).is_file() or shutil.which(binary)):
        sys.exit(f"--binary {binary} does not exist")
    binary_path = Path(binary) if Path(binary).is_file() else Path(shutil.which(binary))
    default_model = creds["model"]
    models = [m for m in args.models.split(",") if m] or [default_model]
    variants = [v for v in args.variants.split(",") if v]
    unknown = [v for v in variants if v not in ARMS]
    if unknown:
        sys.exit(f"unknown variant(s): {', '.join(unknown)} "
                 f"(expected one of {', '.join(ARMS)})")

    intents_path = Path(args.intents)
    intents = json.loads(intents_path.read_text())["intents"]
    if args.filter:
        keep = {x.strip() for x in args.filter.split(",")}
        intents = [i for i in intents if i["id"] in keep]

    stamp = datetime.now().strftime("%Y%m%d-%H%M%S")
    started_at = datetime.now(timezone.utc).isoformat()
    out_dir = Path(args.out).resolve() if args.out \
        else Path(__file__).resolve().parent / "results" / stamp
    out_dir.mkdir(parents=True, exist_ok=True)

    runs = []
    total = len(intents) * len(models) * len(variants) * args.seeds
    done = 0
    print(f"benchmark: {len(intents)} intents x {len(models)} model(s) x "
          f"{len(variants)} variant(s) x {args.seeds} seed(s) = {total} runs "
          f"-> {out_dir}")
    for model in models:
        for variant in variants:
            for spec in intents:
                for seed in range(1, args.seeds + 1):
                    done += 1
                    seed_tag = "" if args.seeds == 1 else f"__s{seed}"
                    run_dir = out_dir / f"{model}__{variant}__{spec['id']}{seed_tag}"
                    run_dir.mkdir(parents=True, exist_ok=True)
                    print(f"[{done}/{total}] {model} / {variant} / {spec['id']}"
                          f"{seed_tag} ...", flush=True)
                    gen = run_generation(variant, spec["intent"], model, run_dir,
                                         creds, binary, args)
                    wf = run_dir / "workflow.oxoflow"
                    if not wf.exists():
                        alt = sorted(run_dir.rglob("*.oxoflow"))
                        if alt:
                            wf.write_text(alt[0].read_text())
                        else:
                            wf = None
                    gates = run_gates(binary, wf, run_dir) if wf else {}
                    # Fidelity: the intent names tools; a gate-valid artifact
                    # that never mentions them passes the gates but does not
                    # do what was asked. Deterministic substring check on the
                    # lowercased artifact (tool names, not flags/paths).
                    # A requires entry may carry '|' alternates for tool
                    # equivalences (e.g. "picard|gatk": GATK4 absorbed the
                    # Picard tools, so MarkDuplicates under gatk4 satisfies a
                    # picard requirement).
                    artifact_text = wf.read_text().lower() if wf else ""
                    missing_tools = [
                        group
                        for group in spec.get("requires", [])
                        if not any(
                            alt.strip().lower() in artifact_text
                            for alt in group.split("|")
                        )
                    ]
                    result = {
                        "intent_id": spec["id"], "tier": spec["tier"],
                        "domain": spec["domain"], "model": model, "variant": variant,
                        "seed": seed, "vague": spec.get("vague", False),
                        "profile": args.profile,
                        **gen,
                        "gates": gates,
                        "all_gates_pass": bool(gates) and all(g["pass"] for g in gates.values()),
                        "missing_required_tools": missing_tools,
                        "fidelity_pass": not missing_tools,
                        "success": bool(gates)
                        and all(g["pass"] for g in gates.values())
                        and not missing_tools,
                        "workflow_file": str(wf) if wf else None,
                        "est_cost_usd": round(cost_usd(model, gen["input_tokens"],
                                                       gen["output_tokens"]), 5),
                    }
                    (run_dir / "result.json").write_text(json.dumps(result, indent=2))
                    runs.append(result)
                    print(f"    generated={result['generated']} "
                          f"gates={'PASS' if result['all_gates_pass'] else 'FAIL'} "
                          f"fidelity={'OK' if result['fidelity_pass'] else 'MISSING:' + ','.join(missing_tools)} "
                          f"tok(in/out)={result['input_tokens']}/{result['output_tokens']} "
                          f"wall={result.get('wall_s')}s", flush=True)
                    time.sleep(args.sleep)

    (out_dir / "results.json").write_text(json.dumps({
        "stamp": stamp,
        "generated_at": datetime.now(timezone.utc).isoformat(),
        "started_at": started_at,
        "finished_at": datetime.now(timezone.utc).isoformat(),
        "campaign_note": args.campaign_note,
        "creds_source": args.creds_from,
        "provider": creds["provider"],
        "arm_a_protocol": resolve_arm_a_protocol(args.arm_a_protocol,
                                                 creds["provider"]),
        "binary": binary,
        "binary_sha256": sha256_file(binary_path),
        "harness_sha256": sha256_file(Path(__file__).resolve()),
        "intents_sha256": sha256_file(intents_path),
        "budget_env": {key: os.environ[key] for key in BUDGET_ENV_VARS
                       if os.environ.get(key)},
        "default_model": default_model,
        "max_retries": args.max_retries,
        "seeds": args.seeds,
        "attempts": args.attempts,
        "profile": args.profile,
        "runs": runs,
    }, indent=2))

    (out_dir / "report.md").write_text(
        aggregate_report(runs, profile=args.profile, stamp=stamp, binary=binary))
    print(f"\ndone: {out_dir / 'report.md'}")


if __name__ == "__main__":
    main()
