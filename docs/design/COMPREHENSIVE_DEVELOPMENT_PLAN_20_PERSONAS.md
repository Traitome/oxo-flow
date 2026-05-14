# Comprehensive Development Plan (20 Personas)

## Executive Summary
This document outlines a comprehensive, reality-grounded development and testing plan for the `oxo-flow` bioinformatics pipeline engine. It is derived from the synthesis of 20 distinct personas spanning the entire spectrum of bioinformatics research, clinical application, system administration, and infrastructure engineering. The goal is to ensure `oxo-flow` is robust, scalable, secure, and user-friendly for real-world scenarios.

---

## Persona-Driven Gap Analysis & Feature Planning

### 1. Top Bioinformatics Expert (顶级生信专家)
**Needs:** High-performance DAG resolution, dynamic resource scaling, support for extremely complex multi-omics workflows.
**Current State:** Supports static resources and retries.
**Action Plan:** Implement dynamic resource adjustment on retry (e.g., if OOM, double memory on retry). Enhance DAG visualization for massive graphs (collapsible subgraphs).

### 2. Cluster Expert/Admin (集群专家)
**Needs:** Robust integration with SLURM/PBS/SGE/LSF, fair-share compliance, account/partition management.
**Current State:** Basic job script generation. `status` and `cancel` commands only print hints.
**Action Plan:** Implement active polling for `cluster status` (parsing `squeue`/`qstat` output). Track job IDs emitted during `cluster submit`.

### 3. Ops/DevOps Expert (运维专家)
**Needs:** Health checks, Prometheus metrics, CI/CD integration, rootless containers.
**Current State:** Basic web server with `/api/health`.
**Action Plan:** Add Prometheus metrics endpoint (`/api/metrics`) to the web server tracking workflow runs, rule execution times, and resource usage.

### 4. Junior Bioinformatics Workflow User (初级生信用户)
**Needs:** Easy onboarding, working examples out-of-the-box, clear error messages.
**Current State:** `init` generates commented-out, non-functional template.
**Action Plan:** Modify `init` to generate a functional, simple "hello world" pipeline that runs successfully out-of-the-box. Add a `--template` flag for advanced setups.

### 5. Senior Bioinformatics Workflow Developer (高级生信开发者)
**Needs:** Modular workflows (`include`), static analysis (`lint`), pipeline debugging (`debug`), CI tools.
**Current State:** Has `lint` and `debug`.
**Action Plan:** Add strict mode to `lint` checking for unused outputs or missing resource definitions. Implement a `--json` output for `dry-run` for CI pipeline integration.

### 6. Bioinformatics Lab Manager/PI (实验室管理员/PI)
**Needs:** Resource cost estimation, execution history, comprehensive PDF/HTML reports.
**Current State:** HTML/JSON reports generated, but lack cost/resource aggregation.
**Action Plan:** Enhance `report` command to summarize total CPU hours and memory usage.

### 7. Clinical Bioinformatics Director (临床生信主任)
**Needs:** ACMG/AMP compliance, checksum validation, immutability, provenance tracking.
**Current State:** Has basic checksum fields.
**Action Plan:** Enforce strict file checksum validation in clinical profiles.

### 8. Cloud Infrastructure Engineer (云架构师)
**Needs:** Native execution on AWS Batch, Google Cloud Life Sciences, or Kubernetes.
**Current State:** Only local and HPC clusters supported.
**Action Plan:** Design a `cloud` executor trait. First target: Kubernetes Jobs.

### 9. Data Scientist (数据科学家)
**Needs:** Interactive exploration, Jupyter Notebook/RMarkdown support.
**Current State:** Terminal and Web UI focus.
**Action Plan:** Add capability to export a rule as a Jupyter Notebook stub.

### 10. System Security Engineer (系统安全工程师)
**Needs:** Prevention of shell injection and path traversal.
**Current State:** `touch` prevents some path traversal.
**Action Plan:** Implement strict path normalization and sanitization across `clean`, `touch`, and input/output parsing. Prevent writing outside the workspace.

### 11. Software QA Engineer (测试工程师)
**Needs:** Robust unit/integration testing framework for pipelines.
**Current State:** Framework tests exist, but no pipeline-level testing tool.
**Action Plan:** Introduce `oxo-flow test` to mock inputs and assert outputs for individual rules.

### 12. UX/UI Designer (用户体验设计师)
**Needs:** Intuitive interfaces, clear status indicators.
**Current State:** Web server provides basic REST API.
**Action Plan:** Improve CLI output formatting. Add a TUI (Terminal UI) mode for `run` and `status` using a library like `ratatui`.

### 13. Technical Writer (技术文档作者)
**Needs:** Accurate, comprehensive, easily navigable documentation.
**Current State:** Docs exist in MkDocs format but need synchronization with code changes.
**Action Plan:** Automate CLI help to Markdown generation. Audit all MkDocs files.

### 14. Bioinformatics Trainee/Student (生信实习生)
**Needs:** Clear help commands, autocompletion.
**Current State:** Completions exist.
**Action Plan:** Expand `--help` text with concrete examples for every command.

### 15. HPC User Support Specialist (超算用户支持)
**Needs:** Easy troubleshooting, safe cleanups.
**Current State:** `clean` command deletes files.
**Action Plan:** Add dry-run by default to `clean` (require `--force` or `-y` to actually delete) to prevent accidental data loss. Log actions clearly.

### 16. Pipeline Integrator (Pipeline集成人员)
**Needs:** Export to standard formats like CWL or WDL.
**Current State:** Exports to Docker/Singularity/TOML.
**Action Plan:** Implement experimental CWL (Common Workflow Language) export.

### 17. Resource Optimizer (资源优化工程师)
**Needs:** Profiling, bottleneck identification.
**Current State:** `config stats` provides basic counts.
**Action Plan:** Generate a Chrome Trace Event format (`.json`) file during `run` for visualization in `chrome://tracing` or Perfetto.

### 18. Release Manager (发布经理)
**Needs:** Reproducible builds, changelog management.
**Current State:** CI/CD in place.
**Action Plan:** Standardize `RELEASING.md` and automate binary checksum generation in releases.

### 19. Bioinformatics Vendor/ISV (生信软件供应商)
**Needs:** Obfuscation, secure distribution.
**Current State:** Cleartext `.oxoflow` files.
**Action Plan:** Create a `compile` or `pack` command to bundle pipelines into binary or encrypted formats.

### 20. Open Source Contributor (开源贡献者)
**Needs:** Clear architecture diagrams, contribution guidelines.
**Current State:** `CONTRIBUTING.md` exists.
**Action Plan:** Update `ARCHITECTURE.md` with deep-dives into the DAG engine and Executor traits.

---

## Phase 1 Execution Strategy (Immediate Implementation)
In this initial execution phase, we will implement high-impact, low-effort features that touch the most critical personas:
1. **Improve `init` command** to generate functional code (Junior User).
2. **Improve `clean` command** security against path traversal and default to safer interactions (Security Engineer, HPC Support).
3. **Enhance `cluster status` and `cluster cancel`** to actually execute the shell commands instead of just printing hints (Cluster Admin).
4. **Update `README.md` and documentation** to reflect these robust changes (Technical Writer).
5. **Ensure all changes pass tests and linting** (QA Engineer).
