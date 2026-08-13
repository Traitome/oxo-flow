# Custom Skills

oxo-flow's AI layer combines three skill mechanisms:

| Layer | Source | Status |
|---|---|---|
| Built-in bioSkills | 562 curated Agent Skills compiled into the binary | ✅ active (domain-matched injection, `lookup_skill`) |
| Pipeline knowledge graph | 79 workflow skills / 469 literature-backed transitions | ✅ active (`lookup_pipeline`) |
| **User-defined skills** | `.skill.toml` files you write | ✅ **Phase 1: knowledge skills** (this page) |

This page documents the user-defined layer.

---

## Discovery ≠ Activation

The security model has two strictly separated steps:

1. **Discovery** — `oxo-flow` scans `~/.oxo-flow/skills/` (user level) and
   `<project>/.oxo-flow/skills/` (project level) for `*.skill.toml` files.
   This is **read-only**: discovered skills are listed by `oxo-flow ai
   status`, but nothing is loaded or executed.
2. **Activation** — a skill only takes effect when its name is explicitly
   declared in the workflow's `[ai]` section. This is the trust boundary:
   the same explicit-approval model used by MCP clients.

Activated skills contribute **prompt text only** — there is zero code
execution, no filesystem access, no tool registration (tool-type skills
are reserved for the future MCP path).

---

## Skill Format

A skill is a small TOML file named `<name>.skill.toml`:

```toml
# ~/.oxo-flow/skills/qc-expert.skill.toml
name = "qc-expert"
version = "1.0.0"
description = "Advises on FASTQ QC thresholds for WGS"
author = "Your Lab"            # optional
domains = ["qc", "wgs"]        # optional, free-form tags
skill_type = "knowledge"       # "knowledge" (Phase 1); "tool" reserved for MCP

prompt_additions = [           # optional — appended to the system prompt
  "Prefer fastp with --qualified_quality_phred 20 for human WGS reads.",
  "Require a QC report before alignment for clinical samples.",
]
```

| Field | Required | Notes |
|---|---|---|
| `name` | ✅ | Unique; must match the activation entry |
| `version` | ✅ | Semantic version |
| `description` | ✅ | Shown in `oxo-flow ai status` |
| `skill_type` | ✅ | `"knowledge"` now; `"tool"` is reserved for the MCP phase |
| `author`, `domains`, `prompt_additions`, `requires`, `entry` | — | Optional |

Invalid manifests (missing required fields) are skipped during discovery,
never fatal.

---

## Activation

Declare the skills to activate in the workflow's `[ai]` section:

```toml
[ai]
enabled = true
skills = ["qc-expert", "somatic-review"]
```

Only discovered skills whose names appear in this list are activated.
Activated `prompt_additions` are appended to the system prompt of
AI-powered commands (`template --ai`, `dry-run --ai`, `validate --ai`,
`lint --ai`), each clearly headed with `## Skill: <name>`.

Project-level activation can also live in `<project>/.oxo-flow/ai.toml`
(the same `[ai]` table shape) so the workflow file stays clean.

---

## Inspecting Skills

```bash
# Lists discovered skills (user + project level) — no AI provider needed
oxo-flow ai status
```

```
Custom skills:
  qc-expert (knowledge) — Advises on FASTQ QC thresholds for WGS
```

---

## Roadmap

- **Phase 2 — tool skills via MCP**: `skill_type = "tool"` skills reference
  MCP servers (`requires = ["mcp://..."]`) and register their tools through
  the existing MCP bridge. Non-read-only tools still require human approval
  per invocation.
- **Phase 3 — SKILL.md compatibility**: evaluate supporting the emerging
  SKILL.md markdown standard, aligned with the built-in bioSkills format.
