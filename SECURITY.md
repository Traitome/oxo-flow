# Security Policy

## Supported Versions

| Version | Supported          |
| ------- | ------------------ |
| latest  | :white_check_mark: |
| < latest | :x:               |

Only the latest release of oxo-flow receives security updates. We recommend
always running the most recent version.

## Known Security Limitations

oxo-flow implements several security mechanisms, but has known limitations that users should understand:

### Shell Command Sanitization

**What's protected:**
- Basic command substitution patterns (`$()`, backticks)
- Dangerous commands (`rm -rf /`, `chmod 777`, `eval`)
- Redirects to `/dev/`

**Limitations:**
- **Wildcard value injection**: Wildcard values (e.g., `{sample}`) are substituted *after* sanitization checks. A malicious wildcard value could bypass detection.
- **Encoded commands**: Base64-encoded commands, `python -c`, `perl -e`, and similar patterns bypass detection.
- **Remote execution**: SSH commands and `curl | sh` patterns are not flagged.

**Recommendation:** Only use trusted workflow files from verified sources. Never accept wildcard values from untrusted input.

### Path Traversal Protection

**What's protected:**
- Basic `..` traversal in output paths via `validate_path_safety()`
- Canonical path resolution preventing escapes

**Limitations:**
- URL-encoded traversal (`%2e%2e%2f`) bypasses detection
- Windows-style traversal (`..\..`) not handled on Unix systems
- Include directive paths are not validated (potential malicious workflow loading)
- Environment file paths (conda, docker, singularity) are not validated

**Recommendation:** Review all `include` directives and environment paths in workflow files before execution.

### Secret Detection

**Current status:** oxo-flow does not detect secrets embedded in workflow files.

**What's not detected:**
- AWS access keys and secret keys
- API keys (OpenAI, GitHub, etc.)
- Database passwords
- Private keys and certificates
- JWT tokens

**Recommendation:** Use external tools like `gitleaks` or `trufflehog` to scan workflow files before committing. Use environment variables or secret managers instead of embedding secrets.

### Input Validation

**What's validated:**
- Rule names (alphanumeric, underscores, dashes)
- Memory format (`8G`, `16384M`, etc.)
- Circular dependencies in DAG
- Duplicate rule names

**Limitations:**
- Input file paths not validated with same rigor as output paths
- Shell command arguments not validated for injection patterns

## Reporting a Vulnerability

**Please do not open a public GitHub issue for security vulnerabilities.**

Instead, report vulnerabilities responsibly by emailing:

> **security@oxo-flow.org**

Include the following in your report:

- A clear description of the vulnerability
- Steps to reproduce the issue
- The potential impact (e.g., data exposure, arbitrary code execution)
- Any suggested mitigations or fixes, if available
- Your name and affiliation (optional, for credit in the advisory)

## Response Timeline

| Stage | Timeframe |
| ----- | --------- |
| Acknowledgement of report | Within 48 hours |
| Initial assessment | Within 5 business days |
| Fix development & review | Within 30 days (critical), 90 days (non-critical) |
| Public disclosure | After a fix is released |

We will keep you informed of our progress throughout the process.

## Security Update Process

1. **Triage** — The maintainers assess severity using CVSS scoring.
2. **Fix** — A patch is developed on a private branch.
3. **Review** — The fix undergoes internal review and testing.
4. **Release** — A new version is published with the fix included.
5. **Advisory** — A GitHub Security Advisory is published describing the
   vulnerability, affected versions, and remediation steps.
6. **Notification** — Users subscribed to releases are notified.

## Scope

The following are in scope for security reports:

- The oxo-flow core library (`oxo-flow-core`)
- The CLI binary (`oxo-flow-cli`)
- The web interface (`oxo-flow-web`)
- Pipeline execution and environment isolation
- Container build and packaging utilities

Third-party dependencies are out of scope but we appreciate reports about
vulnerable transitive dependencies so we can update them promptly.

## Acknowledgements

We gratefully acknowledge security researchers who responsibly disclose
vulnerabilities. With your permission, we will credit you in the security
advisory.

---

This project is licensed under the [Apache License 2.0](LICENSE).
