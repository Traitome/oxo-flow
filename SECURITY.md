# Security Policy

## Supported Versions

| Version | Supported          |
| ------- | ------------------ |
| latest  | :white_check_mark: |
| < latest | :x:               |

Only the latest release of oxo-flow receives security updates. We recommend
always running the most recent version.

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
