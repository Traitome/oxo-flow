# Custom Skills

oxo-flow's AI layer combines three skill mechanisms:

| Layer | Source | Status |
|---|---|---|
| Built-in bioSkills | 562 curated Agent Skills compiled into the binary | ✅ active (domain-matched injection, `lookup_skill`) |
| Pipeline knowledge graph | 78 workflow skills / 483 literature-backed transitions (465 edges) | ✅ active (`lookup_pipeline`) |
| Skill protocol docs | full SKILL.md document per documented pipeline-graph skill (77 of 78 nodes; the `shared` pseudo-node has none) | ✅ active (`lookup_doc`) |
| **User-defined skills** | `.skill.toml` or `SKILL.md` files you write | ✅ knowledge skills, ✅ MCP tool skills |

This page documents the user-defined layer.

---

## Discovery ≠ Activation

The security model has two strictly separated steps:

1. **Discovery** — `oxo-flow` scans `~/.oxo-flow/skills/` (user level) and
   `<project>/.oxo-flow/skills/` (project level) for `*.skill.toml` files.
   This is **read-only**: discovered skills are listed by `oxo-flow ai`
   (quick status), but nothing is loaded or executed.
2. **Activation** — a skill only takes effect when its name is explicitly
   declared in the workflow's `[ai]` section. This is the trust boundary:
   the same explicit-approval model used by MCP clients.

Activated **knowledge** skills contribute **prompt text only** — there is
zero code execution and no filesystem access. Activated **tool** skills do
register tools: each `mcp://` requirement in their `requires` list is
bridged as an `mcp_<server>_<tool>` tool via the MCP client (Streamable
HTTP only — the engine never spawns server processes). See
[Tool Skills via MCP](#tool-skills-via-mcp).

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
skill_type = "knowledge"       # or "tool" (MCP bridge, see below)

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
| `skill_type` | ✅ | `"knowledge"` (prompt text) or `"tool"` (MCP bridge) |
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

- The server-asserted `annotations.readOnlyHint` is **advisory only** —
  it never authorizes auto-execution (#518: a hostile server can claim it
  for an effectful tool).
- **Every MCP tool invocation is refused unless a human approves that
  specific call** — the AI prompts on the terminal (`Allow execution?
  [y/N]`); non-interactive sessions (CI, `--json`, redirected stdin)
  always refuse.

The endpoint itself is SSRF-screened at connect time (#518): internal
address space (loopback, RFC1918/ULA, link-local, cloud-metadata ranges,
CGNAT 100.64/10, benchmarking 198.18/15, NAT64 `64:ff9b::/96`, 6to4
`2002::/16`) and site-local names (`localhost`, `.local`, `.internal`) are refused
unless the exact host is exempted via `OXO_FLOW_AI_FETCH_ALLOW` — the
same opt-out documented for `fetch_url`. A trusted in-house MCP server on
a private address must be listed there explicitly.

When the endpoint resolves through DNS, the vetted address is pinned for
the connection (#948): the client connects to exactly what was screened,
closing the DNS-rebinding window between screening and connecting
(allowlisted hosts and IP literals skip DNS and need no pin). Redirect
hops are re-screened the same way — a hop that lands in internal space is
refused, and an approved hop connects to its own vetted address.

This preserves the trust boundary: the AI never executes autonomously,
and attacker-influenced skill manifests cannot aim it at internal
services.

### Protocol Version and Authentication

On connect the client proposes the newest MCP protocol version it speaks
(`2025-06-18`) at `initialize` and uses whatever version the server
answers with; a server answering with a version this client does not
understand (`2025-06-18`, `2025-03-26`, `2024-11-05`) fails the session
closed instead of proceeding against an unvetted dialect. The negotiated
version is sent back as `MCP-Protocol-Version` on every subsequent
request.

MCP servers requiring bearer authentication read the token from the
environment — never from a skill manifest, which is attacker-influenced
text (#518):

- `OXO_FLOW_MCP_TOKEN` — generic token sent to every MCP server
- `OXO_FLOW_MCP_TOKEN_<SERVER>` — per-server token (server name
  uppercased, non-alphanumeric characters folded to `_`; e.g. server
  `fs-gdrive` → `OXO_FLOW_MCP_TOKEN_FS_GDRIVE`)

The per-server variable wins when both are set. The `<SERVER>` name is
folded from the endpoint URL's host, port and path (everything after the
scheme — `mcp://fs-gdrive:9000/v1` → `OXO_FLOW_MCP_TOKEN_FS_GDRIVE_9000_V1`),
so two servers sharing a host on different paths get distinct variables.
Tokens are attached as `Authorization: Bearer …` on every request to the
endpoint's origin and are never logged; a redirect to a different origin
(checked per hop) carries no `Authorization`, session or protocol headers.

Because the token rides `Authorization` on every request, prefer
`https://` endpoints when a token is configured — a plain-`http://`
endpoint sends the credential unencrypted on the wire.

### Bounded Agent Tools

The agent's built-in `read_file` tool refuses files larger than **256 KiB**
(#948) — a model-directed read of a huge data file would otherwise be
buffered whole and dumped into the model transcript (an OOM and
prompt-injection vector). The error steers the model to targeted reads
(`head`, `grep` over a shell channel) or a smaller summary file.
(`fetch_url` carries the same bound under its 256 KiB response cap.)

## Roadmap

- SKILL.md `metadata.allowed-tools` and license fields may be honored in a
  future release once the ecosystem settles on their semantics.
