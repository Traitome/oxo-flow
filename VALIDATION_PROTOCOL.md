# Validation Protocol

This document provides IQ/OQ/PQ (Installation Qualification / Operational
Qualification / Performance Qualification) validation protocol templates for
deploying oxo-flow in clinical laboratory environments.

## Purpose

To establish documented evidence that the oxo-flow pipeline system is properly
installed, operates correctly, and consistently produces accurate results in
accordance with its specifications.

## Scope

This protocol covers:

- Installation of the oxo-flow software and its dependencies
- Verification of operational functionality
- Validation of pipeline accuracy using reference datasets

## Definitions

| Term | Definition |
| ---- | ---------- |
| **IQ** | Installation Qualification — verifies that the software is installed correctly and all components are present. |
| **OQ** | Operational Qualification — verifies that the software operates as intended across its specified operating ranges. |
| **PQ** | Performance Qualification — verifies that the software produces accurate and reproducible results using real-world or reference data. |

---

## Installation Qualification (IQ)

### IQ-1: Software Installation

| Test ID | IQ-1.1 |
| ------- | ------ |
| **Objective** | Verify oxo-flow is installed and reports the expected version. |
| **Procedure** | Run `oxo-flow --version` |
| **Acceptance criteria** | Output matches the validated version number. |
| **Result** | |
| **Pass / Fail** | |

| Test ID | IQ-1.2 |
| ------- | ------ |
| **Objective** | Verify all workspace crates compile without errors. |
| **Procedure** | Run `cargo build --release` |
| **Acceptance criteria** | Build completes with exit code 0 and no errors. |
| **Result** | |
| **Pass / Fail** | |

### IQ-2: Dependency Verification

| Test ID | IQ-2.1 |
| ------- | ------ |
| **Objective** | Verify container runtime is available. |
| **Procedure** | Run `docker --version` or `singularity --version` |
| **Acceptance criteria** | A supported version is reported. |
| **Result** | |
| **Pass / Fail** | |

| Test ID | IQ-2.2 |
| ------- | ------ |
| **Objective** | Verify environment manager is available. |
| **Procedure** | Run `conda --version` or `pixi --version` |
| **Acceptance criteria** | A supported version is reported. |
| **Result** | |
| **Pass / Fail** | |

### IQ-3: Configuration Integrity

| Test ID | IQ-3.1 |
| ------- | ------ |
| **Objective** | Verify workflow files are intact. |
| **Procedure** | Compute SHA-256 checksums of all `.oxoflow` files and compare against documented values. |
| **Acceptance criteria** | All checksums match. |
| **Result** | |
| **Pass / Fail** | |

---

## Operational Qualification (OQ)

### OQ-1: Workflow Validation

| Test ID | OQ-1.1 |
| ------- | ------ |
| **Objective** | Verify workflow file parsing and validation. |
| **Procedure** | Run `oxo-flow validate <workflow.oxoflow>` |
| **Acceptance criteria** | Validation passes with no errors. |
| **Result** | |
| **Pass / Fail** | |

### OQ-2: DAG Construction

| Test ID | OQ-2.1 |
| ------- | ------ |
| **Objective** | Verify DAG is correctly constructed from the workflow. |
| **Procedure** | Run `oxo-flow dry-run <workflow.oxoflow>` |
| **Acceptance criteria** | Dry run completes, DAG is displayed, and all tasks are listed in expected order. |
| **Result** | |
| **Pass / Fail** | |

### OQ-3: Environment Resolution

| Test ID | OQ-3.1 |
| ------- | ------ |
| **Objective** | Verify environments can be created for all rules. |
| **Procedure** | Run `oxo-flow env create <workflow.oxoflow>` |
| **Acceptance criteria** | All environments are created without errors. |
| **Result** | |
| **Pass / Fail** | |

### OQ-4: Error Handling

| Test ID | OQ-4.1 |
| ------- | ------ |
| **Objective** | Verify graceful handling of invalid input. |
| **Procedure** | Run `oxo-flow validate` with a malformed `.oxoflow` file. |
| **Acceptance criteria** | A clear error message is displayed and exit code is non-zero. |
| **Result** | |
| **Pass / Fail** | |

### OQ-5: Unit and Integration Tests

| Test ID | OQ-5.1 |
| ------- | ------ |
| **Objective** | Verify all automated tests pass. |
| **Procedure** | Run `cargo test` |
| **Acceptance criteria** | All tests pass with exit code 0. |
| **Result** | |
| **Pass / Fail** | |

---

## Performance Qualification (PQ)

### PQ-1: Reference Dataset Execution

| Test ID | PQ-1.1 |
| ------- | ------ |
| **Objective** | Verify end-to-end pipeline execution with a reference dataset. |
| **Procedure** | Run the validated pipeline against the reference dataset. |
| **Acceptance criteria** | Pipeline completes without errors. All output files are produced. |
| **Result** | |
| **Pass / Fail** | |

### PQ-2: Output Accuracy

| Test ID | PQ-2.1 |
| ------- | ------ |
| **Objective** | Verify output accuracy against known results. |
| **Procedure** | Compare pipeline outputs (e.g., variant calls) against a truth set using standard metrics (sensitivity, specificity, F1). |
| **Acceptance criteria** | Metrics meet or exceed predefined thresholds. |
| **Result** | |
| **Pass / Fail** | |

### PQ-3: Reproducibility

| Test ID | PQ-3.1 |
| ------- | ------ |
| **Objective** | Verify identical results across repeated runs. |
| **Procedure** | Run the pipeline twice on the same input data. Compare output checksums. |
| **Acceptance criteria** | Output file checksums are identical between runs. |
| **Result** | |
| **Pass / Fail** | |

| Test ID | PQ-3.2 |
| ------- | ------ |
| **Objective** | Verify identical results across different machines. |
| **Procedure** | Run the pipeline on two different systems using the same containerized environment. Compare output checksums. |
| **Acceptance criteria** | Output file checksums are identical across systems. |
| **Result** | |
| **Pass / Fail** | |

### PQ-4: Performance Benchmarks

| Test ID | PQ-4.1 |
| ------- | ------ |
| **Objective** | Verify pipeline execution time is within acceptable limits. |
| **Procedure** | Record wall-clock time for end-to-end pipeline execution on the reference dataset. |
| **Acceptance criteria** | Execution time is within ±20% of the documented benchmark. |
| **Result** | |
| **Pass / Fail** | |

---

## Validation Summary

| Phase | Total Tests | Passed | Failed | N/A |
| ----- | ----------- | ------ | ------ | --- |
| IQ | | | | |
| OQ | | | | |
| PQ | | | | |

## Sign-Off

| Role | Name | Signature | Date |
| ---- | ---- | --------- | ---- |
| Validation lead | | | |
| Quality assurance | | | |
| Laboratory director | | | |

---

*This protocol is a template. Adapt test IDs, acceptance criteria, and
procedures to match your specific pipeline, reference datasets, and regulatory
requirements. For change control, see [CHANGE_CONTROL.md](CHANGE_CONTROL.md).*
