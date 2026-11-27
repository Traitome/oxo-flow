# Security Model

oxo-flow implements defense-in-depth security across seven layers: command execution, file system access, credential protection, web API rate limiting, HTTP security headers, audit trail integrity, and tenant execution isolation.

---

## Layer 1 — Shell Injection Prevention

All shell commands are validated before execution against a set of blocked and warning patterns.

### Blocked Patterns (Hard Errors)

These patterns **halt execution** — the workflow will not run:

| Category | Patterns Blocked | Error Code |
|----------|-----------------|------------|
| Recursive deletion | `rm -rf /`, `rm -rf ~`, `rm -r /` — and any `rm -rf`/`rm -r` target resolving **outside** the run workdir | E011 |
| Filesystem destruction | `mkfs`, `mkswap`, `dd` to `/dev/sd*` | E011 |
| Permission escalation | `chmod 777 /`, `chmod -R 777` | E011 |
| Block device writes | `> /dev/sd*`, `>> /dev/sd*` | E011 |
| Remote code execution | `curl/wget ... \| sh/bash/dash` | E011 |
| Fork bombs | `() { :\|:& };:` patterns | E011 |
| Data destruction | `dd if=/dev/zero/random/urandom` | E011 |

Recursive-deletion targets are judged after rendering: `rm -rf {config.out_dir}/...`
with an absolute `out_dir` is **allowed** when the resolved path lies inside the
run workdir (pipelines legitimately clean up their own outputs), while root,
home (`~`), `--no-preserve-root` forms, and any target outside the workdir
remain hard errors. The static lint ([E011](./glossary.md#diagnostic-codes)) still flags raw `rm -rf /`-style
templates regardless.

Quoted operands are parsed, not blanket-rejected: each operand may be
individually wrapped in balanced quotes (`rm -rf $prefix "$prefix.fa"` — the
standard defensive spelling in shell loops), and the quotes are stripped
before the in-workdir check. Command substitution (`$(...)`, backticks),
unbalanced quotes, and quote pairs spanning multiple operands
(`rm -rf "a b" c`) still fail closed as unparseable.

One unbalanced shape is repaired rather than rejected: the shell-trap idiom
`trap 'rm -rf "$D"' EXIT` wraps the command in single quotes, and the closing
apostrophe rides along inside the captured operand (`"..." ' — five quote
characters, odd parity). When exactly one token carries that stray trailing
apostrophe and the double-quote parity is intact, it is stripped as the trap
artifact and the operand is judged normally — `trap 'rm -rf "$WORKDIR/interim"' EXIT` inside the workdir passes, outside fails with the usual
outside-the-workdir error. Any other unbalanced-quote shape still fails
closed, with the error's suggestion naming the trap apostrophe so the fix is
obvious.

### Warning Patterns (Non-Blocking)

These emit **warnings** but allow execution — the rule runs either way. The
warning exists because such idioms can also ride in on rendered wildcard
values (a `$()` baked into a sample name), so verify flagged commands are
intentional:

| Pattern | Warning |
|---------|---------|
| `$(command)` substitution | Command substitution detected |
| Backtick `` `command` `` | Backtick command substitution |
| `rm -rf` | Dangerous recursive deletion |
| `chmod 777` | Overly permissive chmod |
| `eval` | eval usage detected |
| `curl/wget` piped to shell or `&& bash` | Remote pipe to shell detected |

At run time (and in `dry-run` output) each warning reads e.g.

```
Command substitution detected in 'echo $(date)': this shell idiom is allowed
and the rule will run; it is flagged because such content can also ride in on
rendered wildcard values — verify it is intentional
```

The static lint for the same idioms is [W023](./glossary.md#diagnostic-codes) (aggregated one diagnostic per
command).

---

## Layer 2 — Path Traversal Protection

Output paths are validated to prevent file system escape:

| Check | Behavior | Error Code |
|-------|----------|------------|
| `..` in path | Blocked — prevents directory traversal | E009 |
| Absolute paths outside workdir | Lint warning (W017); blocked at runtime when they escape the workdir | W017 |
| Interpreter paths | Only simple names or paths under `/usr/bin`, `/usr/local/bin`, `/opt`, `/home`, `/Users` | Parse/validate (E002) + run time |

Interpreter paths are enforced at **two points** (`validate_interpreter_path`):

- **Parse/validate time**: an `interpreter` override that fails the checks
  fails workflow validation (`Rule::validate_field_formats`), so `oxo-flow
  validate` reports it as E002 and the workflow refuses to load — the script
  can never silently run under a different interpreter.
- **Run time** (defense in depth): command construction re-checks the
  override and fails the rule with the validation error instead of falling
  back to auto-detection.

---

## Layer 3 — Secret & Credential Scanning

Hardcoded credentials in workflow TOML content are detected by the `oxo-flow lint` command (`format::scan_for_secrets`), which emits S008 warnings; it does not block execution.

`scan_for_secrets` detects exactly nine patterns — finding one emits an
S008 warning naming it. Four token-shaped patterns are anchored regexes
requiring a credential body (a bare `sk-` prefix is not a secret —
`task-list` must not flag); five word patterns are case-insensitive
substring tests, so both true positives and some false positives
survive — review the flagged lines yourself:

### Detected Secret Patterns

| Pattern | Matching | Warning message |
|---------|----------|-----------------|
| `AKIA…` | regex: `\bakia[0-9a-z]{12,}` (case-insensitive) | Possible AWS Access Key |
| `sk-…` | regex: `\bsk-[a-z0-9_-]{8,}` | Possible Stripe/OpenAI secret key |
| `ghp_…` | regex: `\bghp_[a-z0-9]{8,}` | Possible GitHub personal access token |
| `glpat-…` | regex: `\bglpat-[a-z0-9_-]{8,}` | Possible GitLab personal access token |
| `password` | substring | Possible password in configuration |
| `secret` | substring | Possible secret in configuration |
| `api_key` | substring | Possible API key in configuration |
| `access_token` | substring | Possible access token in configuration |
| `private_key` | substring | Possible private key in configuration |

Additionally, workflow config values declared with `sensitive = true` in a `[config]` definition are masked in all non-execution output — `****` in config logs, `***` in recorded commands, stderr, and plan previews (the value is env-routed instead of staying on the command line), and a `sha256:` digest in the checkpoint's config snapshot.

---

## Dependency Auditing

CI runs `cargo audit` (via `make ci`) against `Cargo.lock` files. When interpreting
audit results, note how the tools differ:

- **`cargo audit` is graph-aware.** It resolves the lockfile against the actual
  dependency graph, so an entry that no crate can reach is not reported.
- **`osv-scanner` (and similar lockfile scanners) are not.** They match every
  entry in the lockfile text. Some entries can therefore be false positives:
  optional dependencies recorded because *some* feature combination of a
  dependency activates them, even though the workspace's own feature set never
  builds them. Verify with `cargo tree -i <crate>@<version> --all-features`
  (add `--target all` for target-gated deps) before treating a lockfile hit as
  a real vulnerability. If no path exists, the entry is compile-inert — it
  cannot ship in a binary — and there is nothing to upgrade.

Two long-lived accepted risks, re-check after any major GUI-stack upgrade:

- **Desktop `glib 0.18` / `proc-macro-error 1.0`** (RUSTSEC-2024-0429 and the
  unmaintained advisory on `proc-macro-error`): `oxo-flow-desktop` is excluded
  from the workspace and its code never touches `glib` directly — the crate
  arrives through the Linux-only `tao 0.37 → gtk 0.18 → glib 0.18` chain of
  the windowing stack. Fixing requires a major `wry`/`tao` upgrade (glib
  0.20+), which is a deliberate, separately-scheduled migration. The affected
  symbols are unreachable from desktop's own code.
- **Desktop `toml 0.8.2`**: pinned exactly (via `proc-macro-crate 2.0`, required
  by `glib-macros 0.18`) to `toml_edit =0.20.2` / `toml_datetime =0.6.3`, which
  forces the older `toml` line into the desktop lockfile. Same resolution path
  as above; unfixable without the GUI-stack upgrade.

---

## Layer 4 — Rate Limiting

The web server applies per-IP rate limiting across all API endpoints:

| Setting | Default |
|---------|---------|
| Max requests | 100 per window |
| Window duration | 60 seconds |
| Response | HTTP 429 with structured error body (`code: "RATE_LIMITED"`) and `Retry-After` header |

Rate limiting is active in all deployment modes (personal, team, hpc). Two
additional behaviors worth knowing: run-submission endpoints have a separate
limiter whose 429 body uses the code `RUN_RATE_LIMITED`, and
`OXO_FLOW_DISABLE_RATE_LIMIT=1` disables the middleware entirely (test/dev
escape hatch, e.g. for Playwright e2e runs — do not use in production).

## Layer 5 — HTTP Security Headers

Every response the web server emits carries a baseline set of security
headers (`X-Content-Type-Options: nosniff`, `X-Frame-Options: DENY`,
`Referrer-Policy: strict-origin-when-cross-origin`, and a
Content-Security-Policy):

```
default-src 'self'; script-src 'self'; style-src 'self' 'unsafe-inline';
img-src 'self' data: blob:; connect-src 'self'
```

Scripts may only load from bundled files — inline `<script>` execution is
blocked, so a future XSS cannot run an inline payload in the same shot it
would need to read the session token from `localStorage`. The server
delivers the sub-path mount prefix (`--base-path`) as a
`<meta name="oxo-base">` tag that the bundled JS reads from the DOM — no
inline bootstrap script. Inline *style* remains allowed because the policy
sets no `style-src-attr`, so style attributes (React's `style={{…}}` props,
the pipeline editor's runtime-injected CodeMirror theme) fall back to
`style-src`, whose `'unsafe-inline'` covers them. The exact policy string is
pinned by the `csp_gate` integration test, which drift-checks both the built
`static/index.html` and the *served* index against inline script — a
weakening fails CI. The baseline has no route-shaped holes: under a
sub-path mount (`--base-path /oxo-flow`), the mount-root SPA index
(`GET /oxo-flow/`) is routed through the same middleware as the nested
app, pinned by the `base_path_mount_headers` integration test.

---

## Layer 6 — Audit Trail Integrity

The web server's audit trail is tamper-evident, not just append-intended
(issue #1372). Every state-changing request writes its `pending` audit row
ahead of the mutation and finalizes it afterwards; each of those writes also
appends an immutable event to a SHA-256 hash chain (`audit_chain` plus a
one-row head) **in the same transaction**. Each event links to its
predecessor; the head names the chain's end (genesis = seq 0, 64 hex zeros).
Appends serialize on the backend (SQLite `BEGIN IMMEDIATE`, PostgreSQL
`FOR UPDATE` on the head), so the sequence cannot gap or duplicate under
concurrent writers. This is true by construction (the lock is the
serialization point); the measured PostgreSQL evidence is one host — two
instances on that host and in separate Linux network namespaces produced
byte-identical exports, and an asymmetric partition plus a crash during an
uncommitted transaction left a verifiable chain. Separate physical hosts,
production scale, database media corruption, and cross-host clock skew
remain unmeasured.

Two admin-only endpoints expose it:

- `GET /api/audit/chain/export` — the deterministic
  `oxo-flow.audit-chain/v1` document (head, events, full audit-row snapshot).
- `GET /api/audit/chain/verify` — 200 with `ok:false` findings for a deleted,
  reordered, inserted, or mutated event/row, duplicate phase, bad genesis, or
  a truncated chain.

`oxo-flow audit verify <export.json>` repeats the whole check offline with no
database or network access (exit 0 valid, 2 invalid/tampered, 4 malformed
with `AUDIT_EXPORT_INVALID`). See the
[`audit` command reference](../commands/audit.md).

### Threat model — what it does and does not prove

- **Detects**: accidental or partial database edits, naive row deletion, a
  reordered/inserted event, a mutated audit field, a truncated table, and a
  log row inserted without its chain event. Both the mutable `audit_logs`
  table and the chain must agree for verification to pass.
- **Does not detect by itself**: an actor with write access to the entire
  database who rewrites every event AND the head self-consistently — including
  deleting the whole chain and resetting the head to genesis. Hash
  chains are not signatures; there is no secret and no external anchor.
  Holding a previously exported document (archived off-host, attached to a
  change record) is what exposes such a rewrite — the attacker cannot edit
  the copy you already have.
- **No external notarization, legal compliance, or confidentiality is
  claimed.** The chain records what the server wrote; it cannot prove an
  event was truthful, and it is readable by anyone who can read the database.
- **Historical rows**: `audit_logs` rows older than the first chain event are
  not covered (the upgrade does not backfill), and deleting a user
  cascade-deletes their audit rows while their chain events persist — the
  verifier reports `MISSING_LOG_ROW` so the shrinkage is visible.
- **Structural corruption is fail-closed, not a finding**: a missing or
  deleted `audit_chain_head` row makes the snapshot unrepresentable, so both
  endpoints answer `500 DB_ERROR` (and every audited write also fails) rather
  than a 200 `ok:false`; an export without a head is malformed input (exit 4)
  to the offline verifier.
- **Availability**: the export is admin-only and unbounded in size (it grows
  with the audit trail); treat a very large export as an administrative
  operation, not a per-request health check.

Backup/restore guidance for the chain is in
[Deployment Modes](../how-to/deploy-modes.md#mode-2-team).

---

## Layer 7 — Tenant Execution Isolation

Workflow rule shells are authored by tenants and run with the server
account's privileges unless an OS-level boundary is installed. The web
spawn path therefore layers opt-in, fail-closed controls on the local
`oxo-flow` CLI and every rule shell it forks (issue #1129 and the #1401
residual):

- **Environment allowlist (always on)** — the child environment is rebuilt
  from the engine's rule-shell policy; `DATABASE_URL`,
  `OXO_FLOW_MASTER_KEY`, AI provider keys, MCP tokens, and sign-in
  passwords never reach a tenant-authored command.
- **Resource ceilings (Linux, opt-in)** — `RLIMIT_DATA` from the workflow's
  declared memory, `RLIMIT_CPU` / `RLIMIT_NPROC` from operator knobs.
- **Landlock sandbox (Linux 5.13+, opt-in)** —
  `OXO_FLOW_WEB_SANDBOX=landlock` denies `/proc`, `/sys`, `/etc`, the
  server `$HOME`, the workspace root, and other tenants' workdirs at the
  kernel level, and denies TCP on kernel 6.7+.
- **User + PID namespace (Linux, opt-in, local runs)** —
  `OXO_FLOW_WEB_ISOLATION=userns` runs the CLI as root inside a private
  user namespace (the server uid outside) and as PID 1 of a private PID
  namespace with a private `/proc`, so a run cannot see or signal host or
  sibling-tenant processes; cancel tears the whole namespace down
  (`--kill-child`).

Both modes are **fail-closed**: a requested mode that cannot be installed
(wrong kernel, LSM denial, bad knob value, unsupported combination) refuses
the spawn instead of running unisolated. They are mutually exclusive: the
Landlock ruleset cannot grant `unshare`'s dynamic `/proc/<pid>` map writes
without re-exposing host `/proc` (measured; see the web API reference).
Residuals are listed there too: single-uid mapping, per-real-uid
`RLIMIT_NPROC`, UDP/ICMP, no filesystem confinement under namespaces alone,
host prerequisites (unprivileged userns; Ubuntu 24.04 needs an AppArmor
`userns,` profile), and remote/cluster runs are outside the boundary.
See [Create Run → Tenant isolation](web-api.md#tenant-isolation-and-the-run-sandbox)
for the enforced-vs-residual matrix and the adversarial test evidence.

---

## Best Practices

1. **Never hardcode secrets** — Use environment variables (`{env.VAR}`) instead
2. **Review shell commands** — Use `oxo-flow dry-run --ai` to audit for safety issues
3. **Keep outputs in workdir** — All rule outputs should be within the workflow directory
4. **Use script files for complex logic** — The `script` field avoids shell escaping issues

---

## Agent-Driven Use

The run-time gates above protect **rule shells** — they do not constrain an
AI agent's own terminal. An agent that probes inputs, authors workflows, and
repairs failures needs the operating contract in
[Agent Safety & Workspace Discipline](../how-to/agent-safety.md): read-only
inputs, snapshot-before-mutate, one scratch tree inside the project, an
append-only operation journal, and `[engine] protected_data_trees` as a
drift backstop. The canonical agent skill ([SKILL.md](../SKILL.md)) embeds
the same rules so they load with the skill.

---

## See Also

- [Workflow Format](./workflow-format.md) — rule field reference
- [AI CLI](./ai-cli.md) — AI-powered workflow analysis
- [Agent Safety & Workspace Discipline](../how-to/agent-safety.md) — operating contract for AI agents
- [Audit command](../commands/audit.md) — offline verification of the hash-chained audit export
- [Troubleshooting](../how-to/troubleshooting.md) — common issues
