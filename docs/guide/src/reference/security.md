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
| Remote code execution | `curl/wget \| sh/bash/sudo` | E011 |
| Fork bombs | `() { :\|:& };:` patterns | E011 |
| Data destruction | `dd if=/dev/zero/random/urandom` | E011 |

### Warning Patterns (Non-Blocking)

These emit **warnings** but allow execution (common in bioinformatics scripts):

| Pattern | Warning |
|---------|---------|
| `$(command)` substitution | Command substitution detected |
| Backtick `` `command` `` | Backtick command substitution |
| `>/dev/` redirects | Redirect to /dev/ detected |
| `eval` | eval usage detected |
| `rm -rf /` (in shell) | Dangerous recursive deletion |
| `chmod 777` | Overly permissive chmod |
| `curl/wget` piped to shell or `&& bash` | Remote pipe to shell detected |

---

## Layer 2 — Path Traversal Protection

Output paths are validated to prevent file system escape:

| Check | Behavior | Error Code |
|-------|----------|------------|
| `..` in path | Blocked — prevents directory traversal | E009 |
| Absolute paths outside workdir | Lint warning (W017); blocked at runtime when they escape the workdir | W017 |
| Interpreter paths | Only simple names or paths under `/usr/bin`, `/usr/local/bin`, `/opt`, `/home`, `/Users` | Validation |

---

## Layer 3 — Secret & Credential Scanning

Hardcoded credentials in workflow TOML content are detected by the `oxo-flow lint` command (`format::scan_for_secrets`), which emits S008 warnings; it does not block execution.

### Detected Secret Types

| Pattern | Examples |
|---------|----------|
| API keys / tokens | `API_KEY=sk-...`, `AUTH_TOKEN=...` |
| Passwords | `password = "hunter2"`, `pwd = "..."` |
| Anthropic keys | `sk-ant-api03-...`, `sk-proj-...` |
| OpenAI / DeepSeek keys | `sk-<32+ chars>` |
| GitHub tokens | `ghp_...`, `gho_...`, `ghu_...`, `ghs_...`, `ghr_...` |
| AWS keys | `AKIA...`, `ASIA...` |
| Private keys (PEM) | `-----BEGIN RSA PRIVATE KEY-----` |
| DB connection strings | `postgresql://user:pass@host/db` |

Secret values are **redacted** in findings — only the first and last 4 characters are shown.

Additionally, workflow config values declared with `sensitive = true` in a `[config]` definition are masked as `****` in logs, `--help`, and error output.

---

## Layer 4 — Rate Limiting

The web server applies per-IP rate limiting across all API endpoints:

| Setting | Default |
|---------|---------|
| Max requests | 100 per window |
| Window duration | 60 seconds |
| Response | HTTP 429 with `retry_after_secs` |

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
