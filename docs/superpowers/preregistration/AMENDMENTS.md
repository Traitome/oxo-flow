# Amendment Log

**Freeze version:** 1.0.0

**Freeze date:** 2026-11-18

**Manifest file:** [`MANIFEST.sha256`](MANIFEST.sha256) — SHA-256
`df1cc1b084cf7f9312ffbdcf2e8e6afdc194a05c8551ac7b8cfca77e62c40450`

**Freeze commit:** the commit that introduced this directory, recorded in
`docs/superpowers/audits/industrial-scientific-audit-ledger.md` §7 wave 7.

## 1. Rule

Every change to PR-01 through PR-06 after the freeze commit requires an
entry here, dated and signed by the approver. An entry must record the
outcome-exposure state. A post-outcome change makes the affected endpoint
exploratory (PR-06 §1 rules 1–4). An entry that only appends a reviewer
signature (PR-05) records the signed manifest hash and does not alter the
frozen analysis content.

## 2. Entries

| # | Date | Artifact / section | Change | Reason | Outcome exposure | Effect | Approver |
| :--- | :--- | :--- | :--- | :--- | :--- | :--- | :--- |
| — | — | — | No amendments as of freeze | — | None | Version 1.0.0 stands | — |

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
| Nextflow version pin (Snakemake 9.19.0 is the only recorded version) | PR-03 §C3 | Campaign lead | Before data collection |
| Equivalence normalizer specification and hash | PR-01 §C3, PR-03 §C3 | Campaign lead | Before data collection |
| Measurement host | PR-03 §C3 | Campaign lead + infrastructure owner | Before data collection |
| Per-arm and total budget | PR-03 §C3 | Campaign lead | Before data collection |
| Independent reviewer sign-off | PR-05 C3 | Independent reviewer | Before data collection |

### #1166

| Open value | Artifact | Owner | Deadline |
| :--- | :--- | :--- | :--- |
| `n_min` and `τ_ARCH` per artifact class | PR-01 §5, PR-06 §C4 | Campaign lead + infrastructure owner + independent reviewer | Before data collection |
| aarch64 host identity | PR-03 §C4 | Infrastructure owner | Before data collection |
| Frozen workflow set and artifact count | PR-02 §C4, PR-03 §C4 | Campaign lead | Before data collection |
| Missingness invalidity limit | PR-02 §C4 | Campaign lead | Before data collection |
| Compute-hour and storage budget | PR-03 §C4 | Campaign lead | Before data collection |
| Independent reviewer sign-off | PR-05 C4 | Independent reviewer | Before data collection |

## 4. Amendment entry template

Copy this block into §2 when closing a value or changing a frozen rule:

```text
| <n> | <YYYY-MM-DD> | <PR-id §section> | <old → new> | <reason> | <none / confirmatory outcome seen — describe> | <confirmatory / exploratory / invalid> | <name, role> |
```
