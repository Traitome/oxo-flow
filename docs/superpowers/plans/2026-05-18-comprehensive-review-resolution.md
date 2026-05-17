# Resolution Plan for 100-User Comprehensive Review

**Reference**: `docs/superpowers/reviews/2026-05-18-100-user-comprehensive-review.md`
**Goal**: 100% resolution of all findings.

This plan breaks down the 60+ findings into manageable phases, prioritizing the Top 10 critical issues first.

## Phase 1: Top 10 Priority Actions (Immediate Impact)
1. **Fix `script`/`envvars` executor integration (D1, D2)**: Update `executor.rs` to read and apply the `script` and `envvars` fields from `Rule`.
2. **Fix timeout skipping retries (R1)**: Modify the retry loop in `executor.rs` to handle timeout results appropriately.
3. **Default `clean` to `--dry-run` (F1)**: Update `oxo-flow-cli` `Clean` command to require `--force` or default to `--dry-run`.
4. **Add webhook documentation (D6)**: Create user documentation for webhooks in `docs/guide/src/reference/webhook.md` and link it.
5. **Split executor.rs (M1)**: Refactor `crates/oxo-flow-core/src/executor.rs` into an `executor` module with submodules (`process.rs`, `timeout.rs`, etc.).
6. **Extract frontend from lib.rs string literal (M3)**: Move inline HTML/JS in `oxo-flow-web` to static files and use `include_str!`.
7. **Add file size limits to pairs/sample_groups reads (R4)**: Add a limit (e.g., 10MB) in `config.rs` when loading CSV/TSV files.
8. **Generate JSON Schema for .oxoflow format (S4)**: Create a schema generator or schema file for tooling.
9. **Add OpenAPI specification for REST API (F10)**: Use `utoipa` or manually write `openapi.yaml` for the web UI endpoints.
10. **Fix CSV parsing to use `csv` crate (R5, R6)**: Refactor `config.rs` CSV parsing to use the robust `csv` crate.

## Phase 2: Core Reliability & Modularity
- **Reliability Fixes**:
  - Batch mode panic (R2).
  - Child PID `unwrap_or(0)` fix (R3).
  - Cross-platform memory (R7) & num_cpus in Docker (R8).
  - LSF walltime formatting (R9).
  - Web UI security: Auth over HTTP, LocalStorage (R10, R11).
  - Better `validate_shell_safety` coverage (R12).
  - Error messages in web UI (R13) and HPC ResourceExhausted (R14).
  - `Ok(TimedOut)` executor return type (R15).
- **Modularity Enhancements**:
  - Split `main.rs` in `oxo-flow-cli` (M2).
  - Decouple CLI from Web crate (M4).
  - Remove clinical types from core `config.rs` (M5).
  - Make `EnvironmentBackend` easily extensible (M6).
  - Design basic plugin architecture (M7, G5).

## Phase 3: Missing Features & Workflow Improvements
- **Missing Commands**: `env create`, `template`, `resume`, `watch` (F6-F9).
- **Major Features**: Cloud execution stubs (G1), remote execution stubs (G2), PDF reports (G3), Cache (G4).
- **Format Improvements**: URL includes (S7), `when` expression docs (S8), format versioning (S5).
- **Misc Edge Cases**: O(n^2) DAG validation (E1), Deadlock detection (E3), TOML datetime (E5).

## Execution Strategy
I will execute this plan sequentially, starting with **Phase 1**. Each task will involve targeted codebase changes followed by testing. I will commit changes progressively if requested.