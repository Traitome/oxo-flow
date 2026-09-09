# Security Model

oxo-flow implements defense-in-depth security across three layers: command execution, file system access, and credential protection.

---

## Layer 1 — Shell Injection Prevention

All shell commands are validated before execution against a set of blocked and warning patterns.

### Blocked Patterns (Hard Errors)

These patterns **halt execution** — the workflow will not run:

| Category | Patterns Blocked | Error Code |
|----------|-----------------|------------|
| Recursive deletion | `rm -rf /`, `rm -rf ~`, `rm -r /` | E011 |
| Filesystem destruction | `mkfs`, `mkswap`, `dd` to `/dev/sd*` | E011 |
| Permission escalation | `chmod 777 /`, `chmod -R 777` | E011 |
| Block device writes | `> /dev/sd*`, `>> /dev/sd*` | E011 |
| Remote code execution | `curl/wget ... \| sh/bash/dash` | E011 |
| Fork bombs | `() { :\|:& };:` patterns | E011 |
| Data destruction | `dd if=/dev/zero/random/urandom` | E011 |

### Warning Patterns (Non-Blocking)

These emit **warnings** but allow execution (common in bioinformatics scripts):

| Pattern | Warning |
|---------|---------|
| `$(command)` substitution | Command substitution detected |
| Backtick `` `command` `` | Backtick command substitution |
| `rm -rf` | Dangerous recursive deletion |
| `chmod 777` | Overly permissive chmod |
| `eval` | eval usage detected |
| `curl/wget` piped to shell or `&& bash` | Remote pipe to shell detected |

---

## Layer 2 — Path Traversal Protection

Output paths are validated to prevent file system escape:

| Check | Behavior | Error Code |
|-------|----------|------------|
| `..` in path | Blocked — prevents directory traversal | E009 |
| Absolute paths outside workdir | Lint warning (W017); blocked at runtime when they escape the workdir | W017 |
| Interpreter paths | Only simple names or paths under `/usr/bin`, `/usr/local/bin`, `/opt`, `/home`, `/Users` | Run time (script execution) |

Interpreter paths are enforced at **run time**, when a script rule's `interpreter` override is resolved (`validate_interpreter_path`): a rejected path is logged as a warning and the override is ignored, so the script runs without its declared interpreter. `validate` does not check interpreter paths.

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

Additionally, workflow config values declared with `sensitive = true` in a `[config]` definition are masked as `***` in logs, `--help`, and error output.

---

## Layer 4 — Rate Limiting

The web server applies per-IP rate limiting across all API endpoints:

| Setting | Default |
|---------|---------|
| Max requests | 100 per window |
| Window duration | 60 seconds |
| Response | HTTP 429 with structured error body (`code: "RATE_LIMITED"`) and `Retry-After` header |

Rate limiting is active in all deployment modes (personal, team, hpc).

---

## Best Practices

1. **Never hardcode secrets** — Use environment variables (`{env.VAR}`) instead
2. **Review shell commands** — Use `oxo-flow dry-run --ai` to audit for safety issues
3. **Keep outputs in workdir** — All rule outputs should be within the workflow directory
4. **Use script files for complex logic** — The `script` field avoids shell escaping issues

---

## See Also

- [Workflow Format](./workflow-format.md) — rule field reference
- [AI CLI](./ai-cli.md) — AI-powered workflow analysis
- [Troubleshooting](../how-to/troubleshooting.md) — common issues
