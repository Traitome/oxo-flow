# Amendment Log

**Original freeze version:** 1.0.0

**Current version:** 1.0.2 (amendment 2)

**Freeze date:** 2026-11-18

**Manifest file:** [`MANIFEST.sha256`](MANIFEST.sha256) — current SHA-256
`b07721b846cbd74808cae156cebdbb93f4e6c140cf289aad217ea6cc06e07609`; the superseded v1.0.1 and v1.0.0 manifest hashes are
retained in the history table in §2.1.

**Freeze commit:** `62d9161b2992` (the commit that introduced this
directory). The amendment-1 commit is recorded in
`docs/superpowers/audits/industrial-scientific-audit-ledger.md` §7 wave 7.

## 1. Rule

Every change to PR-01 through PR-06 after the freeze commit requires an
entry here, dated and signed by the approver. An entry must record the
outcome-exposure state. A post-outcome change makes the affected endpoint
exploratory (PR-06 §1 rules 1–4). An entry that appends a reviewer
signature edits PR-05, so it changes PR-05's hash and requires the manifest
to be recomputed and committed; the entry names the pre-signature manifest
hash that was actually signed.

## 2. Entries

| # | Date | Artifact / section | Change | Reason | Outcome exposure | Effect | Approver |
| :--- | :--- | :--- | :--- | :--- | :--- | :--- | :--- |
| 1 | 2026-11-18 | PR-06 §0.1 and §C1–§C4; PR-01 §C3; PR-02 §C3; PR-03 §C3; PR-04 §2.3 table; README §2.3; AMENDMENTS §1 | v1.0.0 → v1.0.1: (D1) added the ordered evaluation algorithm and inclusive/exclusive interval boundary rules so exactly one of POSITIVE/NEGATIVE/INCONCLUSIVE/INVALID/STOPPED applies, and defined #1166's class-level NEGATIVE precedence plus `τ_ARCH` as the campaign-level threshold (class stability is 100% by definition, not a free parameter); (D2) corrected the false claim that the Nextflow version was unrecorded — `benches/comparative/README.md` records Nextflow 25.10.4 and Snakemake 9.19.0; (D3) added the missing `bwa-mem2` stub (nine stub tools, matching `fault_injection.py`); (D4) fixed the broken internal references (PR-01 → PR-02 §0.3, PR-01 → PR-03 §0, PR-02 → PR-04 §2.2); (D5) a case with zero declared outputs is INVALID for #1165 equivalence and cannot count as vacuously equivalent; (nit) corrected the signature/manifest recompute wording in README/AMENDMENTS and noted that `make eval-lint` needs a built engine binary | Independent review of PR #1225 (D1–D5 and two nits) | None — no confirmatory outcome was examined | Confirmatory; pre-data correction, no endpoint or direction changed | Campaign maintainer (pre-data correction requested by independent review) |
| 2 | 2026-11-28 | PR-01 §5; PR-02 §C4; PR-03 §C4; PR-04 §C4; PR-06 §C4 and header; preregistration README §3; AMENDMENTS §2/§3 | v1.0.1 → v1.0.2: closed the #1166 pre-data values — `τ_ARCH = 0.80` and `n_min = 3`; primary endpoint fixed as the architecture-stable artifact proportion over a frozen 2-workflow / 24-artifact corpus in 8 classes of 3; exact Clopper-Pearson 95% CI (all-identical lower bound `0.025^(1/24) = 0.8574`); environment lock = one shared multi-arch manifest-list digest `sha256:5e2bdb1b…4fa5b`, identical in-container tool versions, same engine commit with a clean workflow tree; native x86_64 (`tx-ubuntu`) vs native aarch64 (Apple Silicon Docker Desktop linux/arm64 VM, no qemu); negative control `uname -m` excluded from the primary endpoint and required to differ; missingness limit 0% and the remaining INVALID conditions; corpus and analysis-script hashes sealed in the pre-run manifest | PR-05/README freeze protocol requires every UNDECIDED/TBD value to be closed by a pre-data amendment before the campaign starts | None — no confirmatory #1166 outcome was examined. The 2026-11-19 OS-confounded field observation and the 2026-11-20 native-amd64-vs-qemu-emulated-arm64 container observation are report-local operational measurements; they are not this frozen corpus, and no cross-arm run of this corpus existed at freeze | Confirmatory; pre-data amendment. The primary endpoint, direction, denominator, and threshold are fixed before data collection. PR-05 C4 independent reviewer sign-off remains PENDING and is disclosed in the campaign report; the governed category is not described as a publication-ready confirmatory result until sign-off | Campaign lead (repository owner-directed research session); infrastructure-owner role filled in the same session; independent reviewer sign-off PENDING |
| 3 | 2026-11-28 | Pre-run harness (not a PR-01..PR-06 artifact); `docs/superpowers/audits/data/crossarch-1397/preregistration.json` | Pre-data harness revision: `CROSSARCH_STAGE=0` pre-staged image path with a fail-closed architecture check; the orchestrator pre-stages both images without `docker rmi` | Docker Desktop 4.94 crashed on `docker rmi` of the digest-only reference during the first staging attempt; no workflow artifact was produced (identity-only attempt preserved at `docs/superpowers/audits/data/crossarch-1397/attempt-1-docker-stopped/`) | None — no confirmatory outcome was examined; no workflow artifact existed | Confirmatory; pre-data harness correction. No threshold, endpoint, corpus, denominator, or environment identity changed; the same manifest-list digest remains the lock. `MANIFEST.sha256` unchanged (`b07721b846cbd74808cae156cebdbb93f4e6c140cf289aad217ea6cc06e07609`) because no PR-01..PR-06 content changed | Campaign lead (repository owner-directed research session) |

### 2.1 Hash history

The v1.0.0 hashes below remain the pre-outcome record of the original
freeze; they are superseded for current verification by the recomputed
v1.0.1 manifest. The v1.0.0 content is recoverable from commit
`62d9161b2992`.

| Version | Date | `MANIFEST.sha256` | Note |
| :--- | :--- | :--- | :--- |
| 1.0.0 | 2026-11-18 | `df1cc1b084cf7f9312ffbdcf2e8e6afdc194a05c8551ac7b8cfca77e62c40450` | Original freeze commit `62d9161b2992`; superseded by amendment 1 |
| 1.0.1 | 2026-11-18 | `d4fa650cf09afd42c7b23718e36674a0a54fdce8d326f2b757df3c89cde691b8` | Amendment 1; superseded by amendment 2 |
| 1.0.2 | 2026-11-28 | `b07721b846cbd74808cae156cebdbb93f4e6c140cf289aad217ea6cc06e07609` | Amendment 2; #1166 campaign parameters frozen |

Amendment-2 artifact hashes:

| Artifact | v1.0.2 SHA-256 |
| :--- | :--- |
| PR-01 | `efd17494e47f7d47df626ea6276180d75c19cf86a917169a53e7e4cb99a002c3` |
| PR-02 | `93d6bd19ad62a3cc0659fbbed61922267906cdd0db297ee7f4d19d9695290db0` |
| PR-03 | `489e8875818691fc5ab2cdc0b1eab5518e43aae17c57b5df4074cfa5706604f6` |
| PR-04 | `9a19109b4d3eb89c01081841ff72cac5ba36be639887a83b3298a4fcb7dffab4` |
| PR-05 | `9a65c632fcb8388fdc06b303578dad61ac413e603c8a2cbaa0ad0e6b08b1b5c5` (unchanged) |
| PR-06 | `e3e098c53d791c9a3ad6de8fd21802965b9eb8ac0237b19c9fa59308233bef21` |

Original v1.0.0 artifact hashes:

| Artifact | v1.0.0 SHA-256 |
| :--- | :--- |
| PR-01 | `11050e1a647fdb1d9a10e4cc2156f8d1f8d252c0bb7b708caa6d7ac11f788178` |
| PR-02 | `56283ea01fb32fe59482c3cfe848680a8e582141df859141c3f0593fdf084f5e` |
| PR-03 | `501cb5719c181d844098863a98317e278ce79a2df0b051c8e384a337caee6377` |
| PR-04 | `503315297fd90494417ea7c2a086b9ce750103899931ba1f7e7416b9384bb73d` |
| PR-05 | `47d9d329e923286168568bd3f29819331da41135bd08de941a24503fb737d866` |
| PR-06 | `97a999624e7b45c71c77a79d3e3e24a17416fb6c76e3b76f9cbb5eec0b14d58e` |

## 3. Open values tracker (must be closed before data collection)

Every value below is intentionally `UNDECIDED` or `TBD` in the frozen
text. Closing one requires a pre-data amendment (a new entry in §2) that
records the value, its evidence, the date, and the approver. A campaign
cannot start with any of its rows still open.

### #1163

| Open value | Artifact | Owner | Deadline |
| :--- | :--- | :--- | :--- |
| `τ_FA` (false-accept threshold) | PR-01 §5, PR-06 §C1 | Campaign lead + independent reviewer | Before data collection |
| Truth-set size and per-code case counts | PR-02 §C1 | Campaign lead | Before data collection |
| Non-verdict invalidity limit | PR-02 §C1 | Campaign lead | Before data collection |
| Measurement host | PR-03 §C1 | Campaign lead | Before data collection |
| Wall-clock budget per case and total | PR-03 §C1 | Campaign lead | Before data collection |
| Independent reviewer sign-off | PR-05 C1 | Independent reviewer | Before data collection |

### #1164

| Open value | Artifact | Owner | Deadline |
| :--- | :--- | :--- | :--- |
| `τ_R` (full-loop verified-repair threshold) | PR-01 §5, PR-06 §C2 | Campaign lead + independent reviewer | Before data collection |
| Exact model identifier (pilot recorded only "GLM via school proxy") | PR-03 §C2 | Campaign lead | Before data collection |
| Held-out confirmatory case set (hash-sealed) | PR-01 §C2, PR-03 §C2 | Campaign lead + custodian | Before data collection |
| Repeat count R and repeat policy | PR-01 §C2, PR-03 §C2 | Campaign lead | Before data collection |
| Measurement host(s) | PR-03 §C2 | Campaign lead + infrastructure owner | Before data collection |
| Total dollar and wall-clock budget | PR-03 §C2 | Campaign lead | Before data collection |
| Independent reviewer sign-off | PR-05 C2 | Independent reviewer | Before data collection |

### #1165

| Open value | Artifact | Owner | Deadline |
| :--- | :--- | :--- | :--- |
| `τ_EQ` (strict equivalence threshold) | PR-01 §5, PR-06 §C3 | Campaign lead + independent reviewer | Before data collection |
| Corpus composition, size, and per-origin-engine minimum | PR-02 §C3, PR-03 §C3 | Campaign lead | Before data collection |
| Repeats per arm R and run-order randomization seed | PR-02 §C3, PR-03 §C3 | Campaign lead | Before data collection |
| Per-case Nextflow/Snakemake build and container-digest pin (repository reference pins are Nextflow 25.10.4 and Snakemake 9.19.0 in `benches/comparative/README.md`) | PR-03 §C3 | Campaign lead | Before data collection |
| Zero-declared-output cases excluded before the run (each exclusion recorded) | PR-02 §C3, PR-06 §C3 | Campaign lead | Before data collection |
| Equivalence normalizer specification and hash | PR-01 §C3, PR-03 §C3 | Campaign lead | Before data collection |
| Measurement host | PR-03 §C3 | Campaign lead + infrastructure owner | Before data collection |
| Per-arm and total budget | PR-03 §C3 | Campaign lead | Before data collection |
| Independent reviewer sign-off | PR-05 C3 | Independent reviewer | Before data collection |

### #1166

| Open value | Artifact | Status |
| :--- | :--- | :--- |
| `n_min` and `τ_ARCH` per artifact class | PR-01 §5, PR-06 §C4 | **CLOSED 2026-11-28 (amendment 2):** `τ_ARCH = 0.80`, `n_min = 3` |
| aarch64 host identity | PR-03 §C4 | **CLOSED 2026-11-28 (amendment 2):** local Apple Silicon macOS host, Docker Desktop native linux/arm64 VM (no qemu) |
| Frozen workflow set and artifact count | PR-02 §C4, PR-03 §C4 | **CLOSED 2026-11-28 (amendment 2):** 2 workflows, 24 primary artifacts, 8 classes of 3; corpus manifest `scripts/cross-arch-provenance-governed-corpus.json` |
| Missingness invalidity limit | PR-02 §C4 | **CLOSED 2026-11-28 (amendment 2):** 0% |
| Compute-hour and storage budget | PR-03 §C4 | **CLOSED 2026-11-28 (amendment 2):** 2 hosts × 2 workflows, minutes of CPU and < 1 GiB storage; no cloud/paid calls |
| Independent reviewer sign-off | PR-05 C4 | **PENDING** — external publication-package gate. Per PR-05, no result may be described as pre-registered or confirmatory until this row is completed. The #1397 campaign report discloses this limitation and labels its category a governed decision under the frozen rule, not a publication-ready confirmatory result. It is not a campaign-validity condition in PR-02 or PR-06 §C4. |

**Protocol note (2026-11-28).** Amendment 2 closes the six
campaign-parameter values above. The independent reviewer sign-off is an
external publication gate, not a parameter this session is authorized to
self-sign; it remains visibly PENDING rather than fabricated. The campaign
is therefore run and reported as a governed decision under the frozen rule
with that limitation disclosed, and the preregistration README's honest
status ledger says exactly that.

## 4. Amendment entry template

Copy this block into §2 when closing a value or changing a frozen rule:

```text
| <n> | <YYYY-MM-DD> | <PR-id §section> | <old → new> | <reason> | <none / confirmatory outcome seen — describe> | <confirmatory / exploratory / invalid> | <name, role> |
```
