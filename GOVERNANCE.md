# Project Governance

This document describes the governance model for the oxo-flow project.

## Principles

- **Openness** — All technical decisions are made in public via GitHub issues,
  pull requests, and discussions.
- **Meritocracy** — Contributions and demonstrated expertise determine
  influence.
- **Consensus-seeking** — We prefer consensus but have clear escalation paths
  when agreement cannot be reached.

## Roles

### Maintainer

Maintainers have full commit access and are responsible for:

- Reviewing and merging pull requests
- Triaging issues and setting priorities
- Making release decisions
- Enforcing the Code of Conduct
- Guiding the project's technical direction

New maintainers are nominated by existing maintainers and approved by consensus.

### Reviewer

Reviewers have demonstrated expertise in specific areas of the codebase and are
trusted to review pull requests in their domain. Reviewers:

- Approve pull requests in their area of expertise
- Mentor new contributors
- Participate in design discussions

Contributors who consistently provide high-quality reviews may be nominated as
reviewers by a maintainer.

### Contributor

Anyone who submits a pull request, files an issue, or participates in
discussions is a contributor. Contributors are expected to:

- Follow the [Code of Conduct](CODE_OF_CONDUCT.md)
- Follow the [Contributing Guide](CONTRIBUTING.md)
- Engage respectfully with maintainers and other contributors

## Decision-Making Process

### Day-to-day decisions

Routine decisions (bug fixes, minor improvements, documentation updates) are
made by the maintainer or reviewer who merges the pull request. A single
approving review from a maintainer or designated reviewer is sufficient.

### Significant changes

Changes that affect the public API, architecture, or project direction require:

1. A GitHub issue or discussion describing the proposed change
2. At least two approving reviews from maintainers
3. A minimum comment period of 5 business days for community feedback
4. No unresolved objections from maintainers

### RFC Process for Major Changes

Major changes (new execution backends, breaking API changes, new file formats)
follow a lightweight RFC process:

1. **Proposal** — Open a GitHub issue with the `rfc` label. Include:
   - Motivation and use cases
   - Detailed technical design
   - Alternatives considered
   - Migration path for existing users
2. **Discussion** — The community has at least 14 days to provide feedback.
3. **Decision** — Maintainers reach consensus to accept, reject, or request
   revisions. The decision and rationale are recorded in the issue.
4. **Implementation** — Accepted RFCs are implemented via standard pull
   requests referencing the RFC issue.

## Conflict Resolution

When contributors or maintainers disagree:

1. **Discussion** — Attempt to resolve the disagreement through respectful
   discussion on the relevant issue or pull request.
2. **Mediation** — If discussion stalls, any party may request mediation from a
   maintainer not involved in the disagreement.
3. **Vote** — As a last resort, maintainers vote. A simple majority decides.
   Ties are broken by the project lead.

## Amendments

This governance document may be amended through the RFC process described above.

---

This project is licensed under the [Apache License 2.0](LICENSE).
