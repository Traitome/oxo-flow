#!/usr/bin/env python3
"""AI generation benchmark for oxo-flow (issue #342 / proposal P6).

Measures the quality/cost frontier of the AI pipeline-generation paths
against the engine's three deterministic static gates:

    oxo-flow validate --json   (structure, DAG, wildcard syntax)
    oxo-flow dry-run  --json   (expansion preview, resource audit)
    oxo-flow lint     --json   (best practices)

Variants
    cli      full CLI path: `oxo-flow template --ai` (rich prompt + knowledge
             tools + tool-calling loop) — the "strong" path.
    minimal  faithful replication of the weakest embedded path (web chat
             `process_chat` prompt: 7 generic rules, no tools, no knowledge
             injection, single shot) — the "floor" path.

Success is judged ONLY by the deterministic gates (never by an LLM judge),
so the numbers are reproducible and cheap. Token usage comes from the AI
session archives (cli) or the provider response (minimal); cost is derived
from a configurable price table.

Usage
    # pilot: all intents, both variants, the model configured in the env
    python3 run_eval.py

    # subset, specific models, custom output dir
    python3 run_eval.py --filter rnaseq-star,atacseq \
        --models deepseek-chat --out results/pilot2

Credentials are read from the environment (ANTHROPIC_BASE_URL /
ANTHROPIC_AUTH_TOKEN / ANTHROPIC_MODEL) or, as a fallback, parsed silently
from ~/.zshrc. Values are never printed.
"""

from __future__ import annotations

import argparse
import json
import os
import re
import subprocess
import sys
import time
import urllib.request
from datetime import datetime, timezone
from pathlib import Path

REPO = Path(__file__).resolve().parents[2]

# USD per 1M tokens. Defaults mirror the repo's own cost constant
# (crates/oxo-flow-ai/src/types.rs: deepseek-v4-pro 0.28 in / 1.10 out,
# pricing as of 2026-08). Add per-model overrides here as needed.
PRICE_PER_MTOK = {
    "*": (0.28, 1.10),
}

ZSHRC = Path.home() / ".zshrc"
ZSHRC_KEYS = ("ANTHROPIC_AUTH_TOKEN", "ANTHROPIC_BASE_URL", "ANTHROPIC_MODEL")

MINIMAL_SYSTEM = """You are a bioinformatics pipeline expert. Generate valid .oxoflow TOML configurations.

Intent: {intent}

Rules:
1. Output TOML in ```toml code fences
2. Use well-known bioinformatics tools with correct command-line syntax
3. Include [workflow] section with name, version, description
4. Define rules with name, shell, inputs, outputs, depends
5. Use {{sample}} wildcard for sample-varying paths
6. Specify conda environment for each rule when possible
7. Include resource hints (threads, memory) in [resources] section"""


def load_credentials() -> dict[str, str]:
    """ANTHROPIC_* from the environment, falling back to ~/.zshrc exports."""
    creds = {k: os.environ.get(k, "") for k in ZSHRC_KEYS}
    if all(creds.values()):
        return creds
    if ZSHRC.exists():
        for line in ZSHRC.read_text().splitlines():
            m = re.match(r"\s*export\s+(ANTHROPIC_[A-Z_]+)=(.*)$", line)
            if not m:
                continue
            key, val = m.group(1), m.group(2).strip().strip("\"'")
            if not creds.get(key):
                creds[key] = val
    missing = [k for k in ZSHRC_KEYS if not creds.get(k)]
    if missing:
        sys.exit(f"missing credentials: {', '.join(missing)} (env or {ZSHRC})")
    return creds


def find_binary() -> str:
    for cand in (REPO / "target/debug/oxo-flow", "oxo-flow"):
        p = Path(cand)
        if p.is_file() or which(cand):
            return str(p)
    sys.exit("oxo-flow binary not found (build with: cargo build -p oxo-flow-cli)")


def which(name: str) -> str | None:
    for d in os.environ.get("PATH", "").split(os.pathsep):
        p = Path(d) / name
        if p.is_file() and os.access(p, os.X_OK):
            return str(p)
    return None


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
                   max_tokens: int, timeout: float) -> tuple[str | None, dict, str | None]:
    """One Anthropic-messages call. Returns (text, usage, error)."""
    base = creds["ANTHROPIC_BASE_URL"].rstrip("/")
    body = json.dumps({
        "model": model,
        "max_tokens": max_tokens,
        "system": system,
        "messages": [{"role": "user", "content": user}],
    }).encode()
    req = urllib.request.Request(
        f"{base}/v1/messages", data=body, method="POST",
        headers={
            "content-type": "application/json",
            "anthropic-version": "2023-06-01",
            "x-api-key": creds["ANTHROPIC_AUTH_TOKEN"],
            "authorization": f"Bearer {creds['ANTHROPIC_AUTH_TOKEN']}",
        },
    )
    try:
        with urllib.request.urlopen(req, timeout=timeout) as resp:
            data = json.load(resp)
    except Exception as e:  # noqa: BLE001 — report any transport/API error verbatim
        return None, {}, str(e)[:500]
    if data.get("type") == "error":
        return None, {}, data.get("error", {}).get("message", "unknown API error")[:500]
    text = "".join(b.get("text", "") for b in data.get("content", [])
                   if b.get("type") == "text")
    usage_in = data.get("usage", {}).get("input_tokens", 0)
    usage_out = data.get("usage", {}).get("output_tokens", 0)
    return text or None, {"input": usage_in, "output": usage_out}, None


def run_generation(variant: str, intent: str, model: str, run_dir: Path,
                   creds: dict, binary: str, args) -> dict:
    """Generate a workflow. Returns usage/meta dict; writes workflow.oxoflow."""
    meta: dict = {"generated": False, "input_tokens": 0, "output_tokens": 0,
                  "tool_calls": None, "rounds": None, "error": None}
    started = time.time()
    meta["wall_s"] = None

    if variant == "minimal":
        text, usage, err = anthropic_chat(
            creds, model, MINIMAL_SYSTEM.format(intent=intent),
            f"## User Request\nGenerate a .oxoflow pipeline for: {intent}\n\n"
            "## Task\nGenerate the optimized .oxoflow TOML configuration now. "
            "Output inside ```toml fences.",
            max_tokens=8192, timeout=args.timeout,
        )
        meta["input_tokens"] = usage.get("input", 0)
        meta["output_tokens"] = usage.get("output", 0)
        if err:
            meta["error"] = err
            return meta
        toml = extract_toml(text or "")
        if not toml:
            meta["error"] = "no TOML found in response"
            return meta
        (run_dir / "workflow.oxoflow").write_text(toml)
        meta["generated"] = True

    elif variant == "cli":
        env = dict(os.environ)
        env["OXO_FLOW_AI_PROVIDER"] = "claude"
        env["ANTHROPIC_MODEL"] = model
        sessions = Path.home() / ".oxo-flow/ai_sessions"
        before = {p.name: p.stat().st_mtime for p in sessions.glob("*-template-*.json")} \
            if sessions.exists() else {}
        cmd = [binary, "template", "--ai", intent, "-o", f"{run_dir}/",
               "--no-color"]
        if args.max_retries is not None:
            cmd += ["--ai-max-retries", str(args.max_retries)]
        if args.profile != "compact":
            cmd += ["--ai-team-profile", args.profile]
        if args.attempts != 1:
            cmd += ["--ai-attempts", str(args.attempts)]
        try:
            proc = subprocess.run(cmd, cwd=run_dir, env=env, timeout=args.timeout,
                                  capture_output=True, text=True)
            (run_dir / "cli.log").write_text(proc.stdout + "\n--- stderr ---\n" + proc.stderr)
            if proc.returncode != 0:
                meta["error"] = f"template exited {proc.returncode}"
        except subprocess.TimeoutExpired:
            meta["error"] = "generation timed out"
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
        # rglob: a relative -o target resolved inside a relative run_dir can
        # nest the artifact one level deeper — find it wherever it landed.
        wf = sorted(run_dir.rglob("*.oxoflow"))
        if wf:
            meta["generated"] = True
        elif not meta["error"]:
            meta["error"] = "no .oxoflow file produced"
    else:
        sys.exit(f"unknown variant: {variant}")

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


def main() -> None:
    ap = argparse.ArgumentParser(description=__doc__.split("\n")[0])
    ap.add_argument("--intents", default=str(Path(__file__).parent / "intents.json"))
    ap.add_argument("--out", default=None, help="results directory (default: results/<stamp>)")
    ap.add_argument("--variants", default="minimal,cli")
    ap.add_argument("--models", default="", help="comma list; default: env ANTHROPIC_MODEL")
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

    creds = load_credentials()
    binary = args.binary or find_binary()
    if not (Path(binary).is_file() or which(binary)):
        sys.exit(f"--binary {binary} does not exist")
    default_model = creds["ANTHROPIC_MODEL"]
    models = [m for m in args.models.split(",") if m] or [default_model]
    variants = [v for v in args.variants.split(",") if v]

    intents = json.loads(Path(args.intents).read_text())["intents"]
    if args.filter:
        keep = {x.strip() for x in args.filter.split(",")}
        intents = [i for i in intents if i["id"] in keep]

    stamp = datetime.now().strftime("%Y%m%d-%H%M%S")
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
        "binary": binary,
        "default_model": default_model,
        "max_retries": args.max_retries,
        "seeds": args.seeds,
        "attempts": args.attempts,
        "profile": args.profile,
        "runs": runs,
    }, indent=2))

    # Aggregate report
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
        wall = [r.get("wall_s", 0) for r in rs]
        cost = sum(r["est_cost_usd"] for r in rs)
        lines += [
            f"## {model} / {variant} / profile={args.profile}", "",
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
    (out_dir / "report.md").write_text("\n".join(lines))
    print(f"\ndone: {out_dir / 'report.md'}")


if __name__ == "__main__":
    main()
