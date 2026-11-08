#!/usr/bin/env python3
"""Refresh the GHCR pull-count badge for oxo-flow.

GitHub has no public API for GHCR download counts (the Packages REST API
requires auth and exposes no download fields; GraphQL has no Package
statistics either — see badges/shields#5594), but the package page HTML
embeds both the lifetime total and a 30-day daily series. This script
scrapes those two numbers and rewrites README.md's badge line in place.

Stdlib-only, idempotent: the file is only touched when the rendered label
actually changes (same value -> no-op), mirroring scripts/refresh-knowledge.
"""

from __future__ import annotations

import json
import re
import sys
import urllib.parse
import urllib.request
from pathlib import Path

REPO = "Traitome/oxo-flow"
PACKAGE_URL = f"https://github.com/{REPO}/pkgs/container/oxo-flow"
README = Path(__file__).resolve().parents[2] / "README.md"

BADGE_MARKER = "label=ghcr%20pulls"

UA_HEADERS = {"User-Agent": "oxo-flow-badge-refresh/1.0"}


def fetch_package_page() -> str:
    req = urllib.request.Request(PACKAGE_URL, headers=UA_HEADERS)
    with urllib.request.urlopen(req, timeout=60) as resp:
        if resp.status != 200:
            raise RuntimeError(f"package page returned HTTP {resp.status}")
        return resp.read().decode("utf-8", errors="replace")


def extract_stats(html: str) -> tuple[int, int]:
    """Return (total_downloads, downloads_last_30_days)."""
    total_match = re.search(
        r'Total downloads</span>\s*<h3 title="(\d+)"', html
    )
    if not total_match:
        raise RuntimeError("Total downloads marker not found on package page")
    total = int(total_match.group(1))

    rects = re.findall(
        r'data-merge-count="(\d+)"\s+data-date="(\d{4}-\d{2}-\d{2})"', html
    )
    if not rects:
        raise RuntimeError("30-day download chart not found on package page")
    last_30_days = sum(int(count) for count, _date in rects)

    return total, last_30_days


def format_count(count: int) -> str:
    if count >= 1_000_000:
        value = f"{count / 1_000_000:.2f}".rstrip("0").rstrip(".")
        return f"{value}M"
    if count >= 1_000:
        value = f"{count / 1_000:.2f}".rstrip("0").rstrip(".")
        return f"{value}K"
    return str(count)


def build_badge(total: int, last_30_days: int) -> str:
    label = f"ghcr pulls {format_count(total)}"
    message = f"{format_count(last_30_days)}/30d"
    return (
        f"[![ghcr pulls](https://img.shields.io/badge/"
        f"{urllib.parse.quote(label)}-{urllib.parse.quote(message)}"
        f"-2496ED?logo=docker&logoColor=white&style=flat-square)]"
        f"(https://github.com/{REPO}/pkgs/container/{REPO.split('/')[-1]})"
    )


def replace_badge(readme: str, badge: str) -> tuple[str, bool]:
    """Replace the existing ghcr pulls badge line, or insert after the
    docker image badge when absent. Returns (new_readme, changed)."""
    pattern = re.compile(r"^\[!\[ghcr pulls\]\(.*?\)\]\(.*?\)\n", re.M)
    if pattern.search(readme):
        new_readme = pattern.sub(badge + "\n", readme, count=1)
        return new_readme, new_readme != readme

    docker_pattern = re.compile(
        r"^(\[!\[docker image\]\(.*?ghcr\.io.*?\)\]\(.*?\)\n)", re.M
    )
    match = docker_pattern.search(readme)
    if not match:
        raise RuntimeError(
            "docker image badge line not found in README.md; "
            f"cannot place {BADGE_MARKER_NAME}"
        )
    return readme[: match.end(1)] + badge + "\n" + readme[match.end(1) :], True


BADGE_MARKER_NAME = "ghcr pulls badge"


def main() -> int:
    html = fetch_package_page()
    total, last_30_days = extract_stats(html)
    badge = build_badge(total, last_30_days)

    readme_path = README
    if not readme_path.exists():
        print(f"::error::README.md not found at {readme_path}")
        return 1
    readme = readme_path.read_text(encoding="utf-8")

    try:
        new_readme, changed = replace_badge(readme, badge)
    except RuntimeError as exc:
        print(f"::error::{exc}")
        return 1

    if new_readme == readme:
        print(f"No changes: ghcr pulls total={total}, 30d={last_30_days}")
        return 0

    readme_path.write_text(new_readme, encoding="utf-8")
    summary = json.dumps(
        {
            "total_downloads": total,
            "downloads_last_30_days": last_30_days,
            "badge": badge,
        }
    )
    print(f"Updated ghcr pulls badge: {summary}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
