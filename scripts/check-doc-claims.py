#!/usr/bin/env python3
"""check-doc-claims.py - fail when documented claims drift from their sources.

The repository makes quantitative and maturity claims in README.md,
ROADMAP.md, AGENTS.md, and docs/guide/src/**. Those numbers used to be
maintained by hand, so they silently rotted (vision.md still said "3/10"
repairs after the 13-case campaign landed; a cloud-maturity row still said
Kubernetes had only mock tests). Every machine-checkable claim now lives in
docs/claims.json: a claim names a deterministic source (the built CLI's
help/JSON output, a Rust constant table, a checked-in evidence JSON, the
generated OpenAPI spec via a small example binary, or a directory listing)
and the exact assertions the user-facing docs must satisfy.

The gate is stdlib-only, makes no network calls, and never hardcodes an
absolute path: paths are relative to the repository root, which is derived
from this script's location. It runs after `make build` because it executes
the built `oxo-flow` binary and the `openapi_counts` example.

Usage:  python3 scripts/check-doc-claims.py        (make claims-check)
Exit:   0 = every claim matches; 1 = claim drift; 2 = malformed registry or
        a source could not be read.
"""

from __future__ import annotations

import json
import re
import subprocess
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
REGISTRY = ROOT / "docs" / "claims.json"
BINARY_TIMEOUT_SECONDS = 120


class ClaimError(Exception):
    """A registry entry or one of its sources is unusable."""


def read_text(relative_path: str) -> str:
    path = ROOT / relative_path
    try:
        return path.read_text(encoding="utf-8")
    except FileNotFoundError as exc:
        raise ClaimError(f"missing file: {relative_path}") from exc
    except OSError as exc:
        raise ClaimError(f"cannot read {relative_path}: {exc}") from exc


def run_binary(relative_path: str, args: list[str]) -> str:
    path = ROOT / relative_path
    if not path.is_file():
        raise ClaimError(
            f"missing binary {relative_path} - run `make claims-check` "
            "(its prerequisites build target/debug/oxo-flow and the "
            "openapi_counts example)"
        )
    try:
        proc = subprocess.run(
            [str(path), *args],
            cwd=ROOT,
            capture_output=True,
            text=True,
            timeout=BINARY_TIMEOUT_SECONDS,
            check=False,
        )
    except (OSError, subprocess.TimeoutExpired) as exc:
        raise ClaimError(f"cannot run {relative_path} {' '.join(args)}: {exc}") from exc
    if proc.returncode != 0:
        stderr = proc.stderr.strip() or "<no stderr>"
        raise ClaimError(
            f"{relative_path} {' '.join(args)} exited {proc.returncode}: {stderr}"
        )
    return proc.stdout


def source_cli_help_subcommands(spec: dict) -> dict[str, int]:
    output = run_binary(spec["binary"], ["--help"])
    match = re.search(r"^Commands:\n(.*?)(?:\n\n|\Z)", output, re.MULTILINE | re.DOTALL)
    if not match:
        raise ClaimError("`--help` output has no Commands: block")
    names = re.findall(r"^  ([a-z][a-z0-9-]*)\s{2,}", match.group(1), re.MULTILINE)
    # clap's implicit `help` subcommand is not part of the product surface.
    names = sorted({name for name in names if name != "help"})
    if not names:
        raise ClaimError("`--help` Commands: block parsed to zero subcommands")
    return {"value": len(names)}


def source_cli_diagnostics_count(spec: dict) -> dict[str, int]:
    catalog = spec["catalog"]
    output = run_binary(spec["binary"], ["diagnostics", "list", catalog, "--json"])
    try:
        payload = json.loads(output)
        node = payload[catalog]
        count = int(node["count"])
        rows = node.get("patterns") or node.get("rules") or []
    except (KeyError, TypeError, ValueError) as exc:
        raise ClaimError(
            f"`diagnostics list {catalog} --json` has an unexpected shape: {exc}"
        ) from exc
    if count != len(rows):
        raise ClaimError(
            f"`diagnostics list {catalog} --json` reports count={count} "
            f"but lists {len(rows)} entries"
        )
    return {"value": count}


def source_rust_const_strings(spec: dict) -> dict[str, str]:
    text = read_text(spec["path"])
    values = sorted(set(re.findall(spec["regex"], text)))
    if not values:
        raise ClaimError(f"regex found no values in {spec['path']}: {spec['regex']}")
    return {"value": len(values), "first": values[0], "last": values[-1]}


def source_regex_unique(spec: dict) -> dict[str, str]:
    text = read_text(spec["path"])
    values = sorted(set(re.findall(spec["regex"], text)))
    if not values:
        raise ClaimError(f"regex found no values in {spec['path']}: {spec['regex']}")
    return {"value": len(values), "first": values[0], "last": values[-1]}


def source_json_number(spec: dict) -> dict[str, int]:
    try:
        payload = json.loads(read_text(spec["path"]))
    except json.JSONDecodeError as exc:
        raise ClaimError(f"{spec['path']} is not valid JSON: {exc}") from exc
    key = spec["key"]
    if key not in payload:
        raise ClaimError(f"{spec['path']} has no `{key}` field")
    value = payload[key]
    if not isinstance(value, int):
        raise ClaimError(f"{spec['path']}#{key} is not an integer: {value!r}")
    row_key = spec.get("must_equal_len_of")
    if row_key is not None:
        rows = payload.get(row_key)
        if not isinstance(rows, list) or len(rows) != value:
            raise ClaimError(
                f"{spec['path']} is internally inconsistent: {key}={value} "
                f"but {row_key} has {0 if not isinstance(rows, list) else len(rows)} rows"
            )
    return {"value": value}


def source_pilot_repair_ratio(spec: dict) -> dict[str, int]:
    text = read_text(spec["path"])
    applied = re.search(r"repair applied (\d+)/(\d+)", text)
    verified = re.search(r"verified rerun (\d+)/(\d+)", text)
    if not applied or not verified:
        raise ClaimError(
            f"{spec['path']} must state `repair applied N/M` and "
            "`verified rerun N/M`"
        )
    applied_n, applied_m = (int(group) for group in applied.groups())
    verified_n, verified_m = (int(group) for group in verified.groups())
    if (applied_n, applied_m) != (verified_n, verified_m):
        raise ClaimError(
            f"{spec['path']} disagrees with itself: applied "
            f"{applied_n}/{applied_m} vs verified rerun {verified_n}/{verified_m}"
        )
    if applied_n > applied_m:
        raise ClaimError(
            f"{spec['path']} reports more applied repairs than cases "
            f"({applied_n}/{applied_m})"
        )
    return {"applied": applied_n, "total": applied_m}


def source_openapi_counts(spec: dict) -> dict[str, int]:
    output = run_binary(spec["binary"], [])
    try:
        payload = json.loads(output)
        paths = int(payload["paths"])
        operations = int(payload["operations"])
    except (KeyError, TypeError, ValueError) as exc:
        raise ClaimError(
            f"{spec['binary']} did not print paths/operations JSON: {exc}"
        ) from exc
    return {"paths": paths, "operations": operations}


def source_directory_count(spec: dict) -> dict[str, int]:
    path = ROOT / spec["path"]
    try:
        count = sum(1 for entry in path.iterdir() if entry.is_dir())
    except OSError as exc:
        raise ClaimError(f"cannot list {spec['path']}: {exc}") from exc
    if count == 0:
        raise ClaimError(f"{spec['path']} contains no directories")
    return {"value": count}


SOURCES = {
    "cli_help_subcommands": source_cli_help_subcommands,
    "cli_diagnostics_count": source_cli_diagnostics_count,
    "rust_const_strings": source_rust_const_strings,
    "regex_unique": source_regex_unique,
    "json_number": source_json_number,
    "pilot_repair_ratio": source_pilot_repair_ratio,
    "openapi_counts": source_openapi_counts,
    "directory_count": source_directory_count,
}


def render(template: str, values: dict[str, object]) -> str:
    rendered = template
    for key, value in values.items():
        rendered = rendered.replace("{" + key + "}", str(value))
    unresolved = re.findall(r"\{[a-z_]+\}", rendered)
    if unresolved:
        raise ClaimError(
            f"unresolved placeholder(s) {sorted(set(unresolved))} in {template!r}"
        )
    return rendered


def as_list(value: object) -> list[str]:
    if value is None:
        return []
    if isinstance(value, str):
        return [value]
    if isinstance(value, list) and all(isinstance(item, str) for item in value):
        return value
    raise ClaimError(f"expected a string or list of strings, got {value!r}")


def check_claim(claim: dict) -> tuple[list[str], dict[str, object]]:
    """Return (failure strings, computed source values) for one claim."""
    failures: list[str] = []
    source = claim.get("source")
    values: dict[str, object] = {}
    if source is not None:
        kind = source.get("kind")
        handler = SOURCES.get(kind)
        if handler is None:
            raise ClaimError(f"{claim['id']}: unknown source kind {kind!r}")
        try:
            values = handler(source)
        except ClaimError as exc:
            return [f"source: {exc}"], values
        for key, minimum in (source.get("min") or {}).items():
            if not isinstance(values.get(key), int) or values[key] < minimum:
                failures.append(
                    f"source: {key}={values.get(key)!r} is below the documented "
                    f"floor {minimum}"
                )
        for key, maximum in (source.get("max") or {}).items():
            if not isinstance(values.get(key), int) or values[key] > maximum:
                failures.append(
                    f"source: {key}={values.get(key)!r} exceeds the documented "
                    f"ceiling {maximum}"
                )

    for check in claim.get("checks", []):
        path = check.get("path")
        if not path:
            raise ClaimError(f"{claim['id']}: a check has no path")
        text = read_text(path)
        lines = text.splitlines()

        for needle in as_list(check.get("contains")):
            rendered = render(needle, values)
            if rendered not in text:
                failures.append(f"{path}: missing {rendered!r}")

        for pattern in as_list(check.get("regex")):
            rendered = render(pattern, values)
            if re.search(rendered, text, re.MULTILINE) is None:
                failures.append(f"{path}: regex did not match {rendered!r}")

        for needle in as_list(check.get("not_contains")):
            rendered = render(needle, values)
            if rendered in text:
                failures.append(f"{path}: forbidden stale text {rendered!r}")

        row_prefix = check.get("row_starts_with")
        if row_prefix is not None:
            prefix = render(row_prefix, values)
            row = next((line for line in lines if line.startswith(prefix)), None)
            if row is None:
                failures.append(f"{path}: no row starts with {prefix!r}")
            else:
                for needle in as_list(check.get("row_contains")):
                    rendered = render(needle, values)
                    if rendered not in row:
                        failures.append(
                            f"{path}: row {prefix!r} is missing {rendered!r}"
                        )
                for needle in as_list(check.get("row_not_contains")):
                    rendered = render(needle, values)
                    if rendered in row:
                        failures.append(
                            f"{path}: row {prefix!r} carries forbidden stale text "
                            f"{rendered!r}"
                        )

    return failures, values


def main() -> int:
    try:
        registry = json.loads(read_text("docs/claims.json"))
    except json.JSONDecodeError as exc:
        print(f"claims-check: FAIL - docs/claims.json is not valid JSON: {exc}", file=sys.stderr)
        return 2

    claims = registry.get("claims")
    if not isinstance(claims, list) or not claims:
        print("claims-check: FAIL - docs/claims.json defines no claims", file=sys.stderr)
        return 2

    failures: list[tuple[str, list[str]]] = []
    assertion_count = 0
    for claim in claims:
        claim_id = claim.get("id")
        if not claim_id:
            print("claims-check: FAIL - a claim has no id", file=sys.stderr)
            return 2
        assertion_count += len(claim.get("checks", []))
        try:
            claim_failures, values = check_claim(claim)
        except ClaimError as exc:
            claim_failures, values = [str(exc)], {}
        if claim_failures:
            failures.append((claim_id, claim_failures))
        else:
            print(f"[ok] {claim_id}" + (f" = {values}" if values else ""))

    if failures:
        for claim_id, claim_failures in failures:
            print(f"[FAIL] {claim_id}")
            for failure in claim_failures:
                print(f"  - {failure}")
        print(
            f"claims-check: FAIL - {len(failures)} of {len(claims)} claim(s) drifted "
            f"across {assertion_count} assertion(s); fix the docs or the claim registry"
        )
        return 1

    print(
        f"claims-check: PASS - {len(claims)} claims / {assertion_count} assertions "
        "match their machine-readable sources"
    )
    return 0


if __name__ == "__main__":
    sys.exit(main())
