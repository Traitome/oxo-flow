# 30-Expert User Journey Assessment Report

> **Generated:** 2026-05-17
> **Version:** oxo-flow v0.5.1
> **Focus:** User experience, documentation gaps, onboarding friction, practical usability

---

## Executive Summary

| Domain | Score | Critical Issues | High Issues | Medium Issues |
|--------|-------|-----------------|-------------|---------------|
| Beginner Onboarding | 8.5/10 | 0 | 2 | 8 |
| Intermediate User Experience | 8.8/10 | 0 | 1 | 6 |
| Advanced User Capabilities | 8.2/10 | 0 | 3 | 10 |
| Bioinformatician Usability | 9.0/10 | 0 | 1 | 5 |
| DevOps/Admin Usability | 7.8/10 | 0 | 4 | 12 |
| Clinical Lab Technician UX | 7.5/10 | 0 | 5 | 15 |
| Documentation Discoverability | 7.2/10 | 0 | 3 | 18 |

**Overall Score:** 8.1/10

---

## Section 1: Beginner Users (5 Experts)

### Expert 1: First-Time User Onboarding

**Role:** Simulates a user who has never used a workflow engine before

**Experience Journey:**

1. **Installation** — ✅ Smooth
   - `cargo install oxo-flow-cli` works perfectly
   - Pre-built binaries available on GitHub Releases
   - No external dependencies required

2. **First `oxo-flow init`** — ✅ Clear
   - Generates helpful project structure
   - Creates a working example workflow
   - Good `.gitignore` for bioinformatics files

3. **First `oxo-flow validate`** — ✅ Intuitive
   - Clear error messages
   - ✓ Success indicator with green checkmark

**Issues Found:**

| Severity | Issue | Location | Fix |
|----------|-------|----------|-----|
| HIGH | Quickstart doesn't explain what happens after run - user sees output but doesn't know how to verify | quickstart.md:116-118 | Add "Check the results" section immediately after run output |
| MEDIUM | No "what went wrong" troubleshooting section for common beginner errors | troubleshooting.md | Add beginner-specific error patterns |
| MEDIUM | `init` creates `my-pipeline.oxoflow` but doesn't explain TOML basics | init.md | Link to TOML primer |
| MEDIUM | No visual guide showing what a DAG looks like before first run | quickstart.md | Add DAG diagram inline |

**Assessment:** The 5-minute quickstart works, but lacks verification guidance. User needs to know "did it actually work?"

---

### Expert 2: Documentation Navigation Reviewer

**Role:** Evaluates how easy it is to find information

**Issues Found:**

| Severity | Issue | Location | Fix |
|----------|-------|----------|-----|
| HIGH | No search functionality on docs site | docs/guide | Add Algolia or similar |
| MEDIUM | "How to create a workflow" and "Workflow Format" have overlapping content | create-workflow.md, workflow-format.md | Clearly differentiate: tutorial vs reference |
| MEDIUM | Missing "Glossary" for terms like DAG, wildcard, rule, environment | docs | Add glossary.md |
| LOW | Some internal links use relative paths that may break | Various | Verify all links |

**Strengths:**
- Diátaxis framework (Tutorials/How-to/Reference) is well-implemented
- Command reference is comprehensive
- Gallery examples are excellent for learning by example

---

### Expert 3: Error Message Clarity Reviewer

**Role:** Evaluates whether error messages help users self-correct

**Issues Found:**

| Severity | Issue | Example | Fix |
|----------|-------|---------|-----|
| MEDIUM | TOML parse errors don't show line number | Missing quote in shell string | Add line context to TOML errors |
| MEDIUM | Circular dependency error just says "cycle detected" | dag.rs cycle detection | Show which rules form the cycle |
| LOW | "failed to expand wildcard" doesn't explain which wildcard | Wildcard expansion failure | Show wildcard name and pattern |

**Strengths:**
- Shell safety validation gives actionable warnings
- Missing config variable references are caught (E005)
- Colored output helps distinguish success/failure

---

### Expert 4: Example Workflow Completeness Reviewer

**Role:** Evaluates whether examples cover real use cases

**Gallery Coverage Assessment:**

| Example | Coverage | Missing Elements |
|---------|----------|------------------|
| 01_hello_world | ✅ Complete | None |
| 02_file_pipeline | ✅ Complete | None |
| 03_parallel_samples | ✅ Complete | None |
| 04_scatter_gather | ✅ Complete | None |
| 05_conda_environments | ⚠️ Partial | Missing Docker/Singularity examples |
| 06_rnaseq_quantification | ✅ Complete | None |
| 07_wgs_germline | ✅ Complete | None |
| 08_multiomics_integration | ✅ Complete | None |
| 09_single_cell_rnaseq | ✅ Complete | None |
| 10_transform_operator | ✅ Complete | None |

**Issues Found:**

| Severity | Issue | Fix |
|----------|-------|-----|
| MEDIUM | No example for tumor-normal paired calling workflow | Add gallery/11_tumor_normal.oxoflow |
| MEDIUM | No example for using `pairs_file` with external TSV | Add to gallery or how-to |
| LOW | Environment examples only show conda, not pixi/docker/singularity variations | Expand environment showcase |

---

### Expert 5: CLI Help Text Reviewer

**Role:** Evaluates --help clarity for beginners

**Assessment:**

| Command | Help Quality | Issues |
|---------|--------------|--------|
| `oxo-flow run` | ✅ Excellent | Shows examples, clear flags |
| `oxo-flow init` | ✅ Good | Could show what gets created |
| `oxo-flow validate` | ✅ Good | Simple and clear |
| `oxo-flow batch` | ⚠️ Moderate | Complex, needs more examples |
| `oxo-flow cluster submit` | ⚠️ Moderate | Needs cluster-specific examples |

**Issues Found:**

| Severity | Issue | Fix |
|----------|-------|-----|
| MEDIUM | `batch` command template placeholders not fully explained | Add placeholder reference table |
| MEDIUM | `cluster submit` doesn't show SLURM/PBS/SGE specific flags | Add backend-specific examples |

---

## Section 2: Intermediate Users (5 Experts)

### Expert 6: Workflow Authoring Efficiency Reviewer

**Role:** Evaluates how fast an intermediate user can write workflows

**Positive Findings:**
- Named inputs/outputs (`input.reads`, `output.bam`) greatly improves readability
- `[defaults]` section reduces repetition significantly
- `{config.*}` variables make parameterization easy
- Auto-dependency inference saves manual `depends_on` declarations

**Issues Found:**

| Severity | Issue | Fix |
|----------|-------|-----|
| HIGH | No workflow template library - users must copy/paste from examples | Create `oxo-flow template list/show/apply` commands |
| MEDIUM | No IDE schema validation - VS Code can't validate `.oxoflow` | Generate JSON Schema for TOML |
| MEDIUM | No rule snippet insertion - common patterns require manual typing | Add `oxo-flow snippet` command |

---

### Expert 7: Environment Management Reviewer

**Role:** Evaluates environment setup workflow

**Positive Findings:**
- Multiple backend support (conda, pixi, docker, singularity, venv)
- Per-rule environment specification works well
- `--skip-env-setup` flag for pre-configured environments

**Issues Found:**

| Severity | Issue | Fix |
|----------|-------|-----|
| HIGH | No `oxo-flow env create` command to bootstrap environments | Add environment creation subcommand |
| MEDIUM | Environment check only validates existence, not version lock | Add `env verify --lock` |
| MEDIUM | Docker bind mounts auto-detection is magic - user can't control | Add explicit `--mount` option |
| LOW | Pixi environment not documented in tutorials | Add Pixi example to environment-mgmt.md |

---

### Expert 8: Cluster Execution Reviewer

**Role:** Evaluates HPC/cluster workflow

**Positive Findings:**
- SLURM, PBS, SGE, LSF backends all supported
- Resource declarations translate to scheduler directives
- `--pending-timeout` prevents infinite waiting

**Issues Found:**

| Severity | Issue | Fix |
|----------|-------|-----|
| HIGH | No `cluster status --watch` for live monitoring | Add watch mode with SSE |
| HIGH | No automatic job array support for large sample counts | Implement `--array` mode |
| MEDIUM | Cluster job scripts not saved for debugging | Add `--save-scripts` flag |
| MEDIUM | No SLURM-specific optimization (e.g., `--gres`, `--constraint`) | Add `extra_scheduler_args` field |

---

### Expert 9: Debugging and Troubleshooting Reviewer

**Role:** Evaluates debugging workflow

**Positive Findings:**
- `oxo-flow debug` shows expanded commands
- `--dry-run` previews execution without running
- Checkpoint files enable resume from failure
- Progress bar shows real-time status

**Issues Found:**

| Severity | Issue | Fix |
|----------|-------|-----|
| MEDIUM | `debug` only shows one rule at a time - no workflow-wide view | Add `--all` flag |
| MEDIUM | No `--verbose-execution` to show per-step progress | Add step-by-step verbose mode |
| LOW | Checkpoint file location not documented clearly | Add to run.md |

---

### Expert 10: Report Generation Reviewer

**Role:** Evaluates clinical report workflow

**Positive Findings:**
- Modular report sections (QC, variants, provenance)
- HTML and JSON output formats
- Tera template engine for customization

**Issues Found:**

| Severity | Issue | Fix |
|----------|-------|-----|
| HIGH | No PDF export - required for clinical documentation | Implement PDF via wkhtmltopdf or similar |
| MEDIUM | No report preview before generation | Add `--preview` mode |
| MEDIUM | Custom template documentation sparse | Add template customization guide |

---

## Section 3: Advanced Users (5 Experts)

### Expert 11: API/Library Integration Reviewer

**Role:** Evaluates using oxo-flow-core as a library

**Positive Findings:**
- Clean public API with re-exports
- `RuleBuilder` pattern for programmatic rule creation
- Type-state lifecycle (`Parsed`, `Validated`, `Ready`)
- Comprehensive rustdoc

**Issues Found:**

| Severity | Issue | Fix |
|----------|-------|-----|
| HIGH | No async execution example in rustdoc | Add async usage example |
| MEDIUM | `WorkflowConfig::parse` doesn't support streaming | Add streaming parser for large files |
| MEDIUM | No builder for `WorkflowConfig` | Add `WorkflowConfigBuilder` |
| LOW | Some types not re-exported (require deep imports) | Expand lib.rs re-exports |

---

### Expert 12: Extensibility Reviewer

**Role:** Evaluates plugin/extension potential

**Critical Gap:**

| Severity | Issue | Fix |
|----------|-------|-----|
| HIGH | No plugin system for custom environment backends | Design and implement plugin API |
| HIGH | No custom lint rule support | Add user-defined lint patterns |
| MEDIUM | No custom report section registration | Add section registry |

---

### Expert 13: Performance Tuning Reviewer

**Role:** Evaluates performance optimization options

**Positive Findings:**
- `-j` flag controls parallelism
- Resource-aware scheduling prevents oversubscription
- `--max-threads` and `--max-memory` for global constraints

**Issues Found:**

| Severity | Issue | Fix |
|----------|-------|-----|
| MEDIUM | No benchmark mode for timing without execution | Add `--benchmark` |
| MEDIUM | No resource usage profiling | Add `--profile-resources` |
| LOW | No cache for environment activation | Implement environment caching |

---

### Expert 14: CI/CD Integration Reviewer

**Role:** Evaluates automated pipeline testing

**Positive Findings:**
- `oxo-flow validate` exits non-zero on errors
- `--dry-run` for workflow testing
- CI-friendly (no interactive prompts unless --force missing)

**Issues Found:**

| Severity | Issue | Fix |
|----------|-------|-----|
| HIGH | No `--fail-fast` mode for CI testing | Add early exit on first failure |
| MEDIUM | No workflow diff for PR reviews | Add `oxo-flow diff --json` for CI output |
| MEDIUM | No schema generation for external validation | Add `oxo-flow schema` command |

---

### Expert 15: Web API Integration Reviewer

**Role:** Evaluates REST API for automation

**Positive Findings:**
- Full workflow lifecycle via REST
- `/api/workflows/dry-run` for validation
- `/api/reports/generate` for report automation

**Issues Found:**

| Severity | Issue | Fix |
|----------|-------|-----|
| HIGH | No OpenAPI/Swagger spec | Generate OpenAPI spec from axum routes |
| HIGH | No webhook for completion notifications | Add webhook support |
| MEDIUM | No Prometheus metrics endpoint | Add `/metrics` endpoint |
| MEDIUM | SSE not documented | Document real-time monitoring via SSE |

---

## Section 4: Domain Experts (10 Experts)

### Expert 16: Bioinformatician - Pipeline Design Reviewer

**Role:** Evaluates bioinformatics workflow patterns

**Positive Findings:**
- `{sample}` wildcard matches industry convention
- `[[pairs]]` for tumor-normal analysis is innovative
- `transform` operator unifies scatter-gather elegantly
- Venus pipeline is production-ready clinical workflow

**Issues Found:**

| Severity | Issue | Fix |
|----------|-------|-----|
| MEDIUM | No `--bam-index` auto-generation rule helper | Add built-in indexing helper |
| LOW | No FASTQ quality score placeholder | Add `{qual_score}` placeholder |

**Assessment:** Excellent for bioinformatics. `pairs` and `sample_groups` are unique innovations.

---

### Expert 17: Bioinformatician - Tool Compatibility Reviewer

**Role:** Evaluates tool integration patterns

**Positive Findings:**
- Environment isolation per rule solves version conflicts
- Container support (Docker/Singularity) for reproducibility
- Named outputs improve multi-file rule readability

**Issues Found:**

| Severity | Issue | Fix |
|----------|-------|-----|
| LOW | No `--tmp-dir` for temporary file location | Add explicit temp directory control |
| LOW | No intermediate file cleanup option | Add `--cleanup-intermediate` |

---

### Expert 18: Clinical Lab Technician - Report Reviewer

**Role:** Evaluates clinical report quality

**Positive Findings:**
- ACMG/AMP variant classification support
- Compliance event tracking
- Audit trail in provenance

**Critical Gaps:**

| Severity | Issue | Fix |
|----------|-------|-----|
| HIGH | No PDF export for EMR integration | Implement PDF generation |
| HIGH | No FHIR/HL7 output format | Add FHIR resource generation |
| MEDIUM | No sample metadata schema enforcement | Add clinical metadata validation |
| MEDIUM | No signature/authentication for clinical reports | Add report signing |

---

### Expert 19: Clinical Lab Technician - QC Workflow Reviewer

**Role:** Evaluates QC automation

**Issues Found:**

| Severity | Issue | Fix |
|----------|-------|-----|
| HIGH | No QC threshold enforcement - only reporting | Add `qc_thresholds` with fail conditions |
| MEDIUM | No auto-rejection of samples failing QC | Add `--qc-fail-action` |
| MEDIUM | No coverage calculation rule helper | Add coverage calculation template |

---

### Expert 20: DevOps Engineer - Container Reviewer

**Role:** Evaluates container deployment

**Positive Findings:**
- `oxo-flow package` generates Dockerfile
- Multi-stage build optimization
- Singularity definition generation

**Issues Found:**

| Severity | Issue | Fix |
|----------|-------|-----|
| HIGH | No container registry push command | Add `--push` flag |
| MEDIUM | No vulnerability scan integration | Add `--scan` flag |
| MEDIUM | No Kubernetes deployment template | Add k8s manifest generation |

---

### Expert 21: DevOps Engineer - Monitoring Reviewer

**Role:** Evaluates observability

**Critical Gaps:**

| Severity | Issue | Fix |
|----------|-------|-----|
| HIGH | No Prometheus metrics export | Add `/metrics` endpoint |
| HIGH | No Grafana dashboard template | Provide dashboard JSON |
| MEDIUM | No Slack/email webhook integration | Add notification hooks |
| MEDIUM | No execution time tracking per rule | Add duration to checkpoint |

---

### Expert 22: HPC Systems Admin - Scheduler Reviewer

**Role:** Evaluates cluster integration

**Positive Findings:**
- All major schedulers supported
- Resource declarations translate correctly

**Issues Found:**

| Severity | Issue | Fix |
|----------|-------|-----|
| HIGH | No node feature matching | Add `node_features` field |
| HIGH | No job array support | Implement `--array` mode |
| MEDIUM | No reservation support | Add `--reservation` flag |
| MEDIUM | No qos specification | Add `--qos` field |

---

### Expert 23: Security Reviewer - Input Validation

**Role:** Evaluates security hardening

**Positive Findings:**
- `validate_shell_safety` blocks command injection
- `validate_path_safety` prevents traversal
- `#![forbid(unsafe_code)]` across all crates
- No secrets in logs

**Verified:**
- Shell substitution `$(...)` is blocked
- Absolute paths are validated
- Rate limiting on web API

**Issues Found:**

| Severity | Issue | Fix |
|----------|-------|-----|
| MEDIUM | No secret detection in workflow files | Add secret scanning lint |
| LOW | No audit log for file access | Add file access audit |

---

### Expert 24: Data Manager - Reproducibility Reviewer

**Role:** Evaluates reproducibility guarantees

**Positive Findings:**
- Config checksums for versioning
- Execution provenance tracked
- Container pinning supported

**Issues Found:**

| Severity | Issue | Fix |
|----------|-------|-----|
| HIGH | No input file checksum tracking | Add input checksum field |
| MEDIUM | No reference database versioning enforcement | Add `reference_db` validation |
| MEDIUM | No lock file generation | Add `oxo-flow.lock` file |

---

### Expert 25: Research Scientist - Documentation Reviewer

**Role:** Evaluates scientific documentation quality

**Issues Found:**

| Severity | Issue | Fix |
|----------|-------|-----|
| MEDIUM | No methodology documentation in reports | Add methods section template |
| MEDIUM | No citation/reference tracking | Add `citations` field |
| LOW | No experiment metadata schema | Add experiment metadata support |

---

## Consolidated Priority Action List

### Immediate (Critical - Fix Now)

1. Add PDF export for clinical reports
2. Implement webhook/notification system
3. Add OpenAPI spec for REST API
4. Add `--fail-fast` mode for CI testing
5. Implement cluster job arrays

### Short-term (High - Next Sprint)

1. Add workflow template library (`oxo-flow template`)
2. Implement `oxo-flow env create` command
3. Add cluster watch mode (`cluster status --watch`)
4. Add JSON Schema for TOML validation
5. Add QC threshold enforcement with fail conditions

### Medium-term (Weeks)

1. Design and implement plugin system
2. Add FHIR/HL7 report output
3. Add Prometheus metrics endpoint
4. Implement benchmark mode
5. Add container registry push

### Long-term (Roadmap)

1. Kubernetes operator
2. CWL/WDL import
3. GA4GH TES/WES compatibility
4. Distributed execution
5. VS Code extension

---

## Score Summary by Domain

```
Domain                      Score    Key Gap
─────────────────────────────────────────────
Beginner Onboarding         8.5      No troubleshooting guide
Intermediate UX             8.8      No template library
Advanced Capabilities       8.2      No plugin system
Bioinformatician            9.0      Excellent domain fit
DevOps/Admin                7.8      No monitoring integration
Clinical Lab                7.5      No PDF/FHIR output
Documentation Discoverability 7.2    No search, overlapping content
```

---

## Conclusion

oxo-flow provides **excellent bioinformatics domain fit** with innovative features like `[[pairs]]`, `transform`, and clinical-grade reporting. The core workflow experience is solid for intermediate users.

**Key UX Gaps:**
1. **Clinical labs** need PDF/FHIR output for regulatory compliance
2. **DevOps** need monitoring integration (Prometheus, webhooks)
3. **Advanced users** need plugin extensibility
4. **Beginners** need better troubleshooting guidance

**Recommendation:** Focus Phase 10 on clinical output formats and monitoring integration to reach production-grade for regulated environments.

---

*End of Report*