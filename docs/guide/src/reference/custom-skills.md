# Custom Skills

oxo-flow's AI layer combines three skill mechanisms:

| Layer | Source | Status |
|---|---|---|
| Built-in bioSkills | 562 curated Agent Skills compiled into the binary | ✅ active (domain-matched injection, `lookup_skill`) |
| Pipeline knowledge graph | 78 workflow skills / 465 literature-backed transitions | ✅ active (`lookup_pipeline`) |
| **User-defined skills** | `.skill.toml` or `SKILL.md` files you write | ✅ knowledge skills, ✅ MCP tool skills |

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
| `description` | ✅ | Shown in `oxo-flow ai` (status output) |
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
oxo-flow ai
```

```
Custom skills:
  qc-expert (knowledge) — Advises on FASTQ QC thresholds for WGS
```

---

## SKILL.md Format (Alternative)

Skills may also be written in the emerging SKILL.md markdown standard —
a directory containing a `SKILL.md` file with YAML frontmatter and a
markdown body:

```
~/.oxo-flow/skills/
└── rnaseq-reviewer/
    └── SKILL.md
```

```markdown
---
name: rnaseq-reviewer
description: Reviews RNA-seq pipelines for strandedness mistakes
version: 1.2.0
---
# Guidance

Always check that featureCounts `-s` matches the library prep, and that
STAR `--sjdbOverhang` is read-length minus one.
```

Discovery scans both `~/.oxo-flow/skills/<name>/SKILL.md` and
`<project>/.oxo-flow/skills/<name>/SKILL.md`. The whole markdown body
becomes the skill's prompt content; activation works exactly like
`.skill.toml` (`[ai] skills = ["rnaseq-reviewer"]`). `.skill.toml`
remains the format for machine metadata (`requires` MCP endpoints,
`skill_type = "tool"`).

## Tool Skills via MCP

A `skill_type = "tool"` skill references an MCP server in `requires`:

```toml
# ~/.oxo-flow/skills/clinical-db.skill.toml
name = "clinical-db"
version = "1.0.0"
description = "Queries the clinical variants MCP server"
skill_type = "tool"
requires = ["mcp://127.0.0.1:5050/mcp"]
```

On activation, oxo-flow connects over **Streamable HTTP** (JSON-RPC
over POST with SSE response support), discovers the server's tools, and
registers each as `mcp_<server>_<tool>` in the agent's tool registry.
The engine never spawns MCP server processes — the server must already
be reachable over HTTP (stdio servers are not supported).

### Human Approval for Non-Read-Only Tools

MCP tools execute under the same approval policy as all agent tools:

- Tools the server marks with `annotations.readOnlyHint = true` may run
  automatically.
- **Everything else is refused unless a human approves the specific
  invocation** — the AI prompts on the terminal (`Allow execution? [y/N]`);
  non-interactive sessions (CI, `--json`, redirected stdin) always refuse.

This preserves the trust boundary: the AI never executes autonomously.

## Roadmap

- SKILL.md `metadata.allowed-tools` and license fields may be honored in a
  future release once the ecosystem settles on their semantics.
