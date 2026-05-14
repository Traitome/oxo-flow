# Clinical Bioinformatics Expert Review: oxo-flow

**Reviewer**: Clinical Bioinformatics Manager (CLIA/CAP perspective)
**Date**: 2026-05-14
**Version Reviewed**: 0.3.1

---

## Executive Summary

oxo-flow demonstrates solid foundations for clinical-grade pipeline execution but requires several critical enhancements before deployment in CLIA/CAP-certified laboratories. The existing framework provides adequate reproducibility mechanisms and report generation capabilities, but gaps exist in audit logging, QC threshold enforcement, sample tracking, and regulatory compliance documentation.

**Recommendation**: Conditional approval for pilot deployment with mandatory implementation of identified gaps before full clinical production use.

---

## 1. Reproducibility and Traceability

### Current State

| Feature | Status | Evidence |
|---------|--------|----------|
| Workflow checksums | Implemented | `config.rs::checksum()` computes deterministic SHA-256 of workflow definition |
| Input file checksums | Implemented | `executor.rs::compute_file_checksum()` uses FNV-1a 64-bit hash |
| Output file checksums | Implemented | `ExecutionProvenance::output_checksums` captures post-execution checksums |
| Content-aware caching | Implemented | `executor.rs::should_skip_rule_content_aware()` prevents false re-runs |
| Reference database versioning | Implemented | `config.rs::ReferenceDatabase` tracks version, source, checksum |
| Workflow fingerprint | Documented | `REPRODUCIBILITY.md` describes workflow fingerprint concept |
| Software version tracking | Implemented | `ExecutionProvenance::software_versions` HashMap |

### Strengths

1. **Deterministic hashing**: Workflow checksums provide reproducibility verification
2. **Execution provenance**: Comprehensive provenance struct captures version, hostname, timestamps, checksums
3. **Reference database tracking**: `ReferenceDatabase` type captures genome/annotation versions
4. **Lineage tracking**: `parent_run_id` field supports re-execution audit trails

### Gaps (CRITICAL)

1. **No SHA-256 for file checksums**: Current FNV-1a is fast but non-standard for clinical compliance. CLIA/CAP typically require SHA-256 or MD5 (deprecated but still accepted).

   **Location**: `/crates/oxo-flow-core/src/executor.rs:951-965`

   ```rust
   // Current: FNV-1a 64-bit
   pub fn compute_file_checksum(path: &Path) -> Result<String> {
       // Uses FNV-1a, not SHA-256
   }
   ```

   **Recommendation**: Implement SHA-256 checksum computation with `sha2` crate for clinical-grade integrity verification.

2. **Missing provenance persistence**: `ExecutionProvenance` is defined but not automatically persisted to disk.

   **Location**: `/crates/oxo-flow-core/src/executor.rs:1067-1146`

   **Recommendation**: Auto-save provenance JSON to `.oxo-flow/provenance.json` after each execution.

3. **No input file integrity verification before execution**: Checksums are computed but not verified against expected values from previous runs.

   **Recommendation**: Add `verify_input_integrity()` function that compares current input checksums against recorded values before execution begins.

4. **Missing lock file generation**: Documentation mentions `conda-lock.yml` and `pixi.lock` but no implementation generates these automatically.

   **Recommendation**: Implement `oxo-flow lock` command to generate conda-lock/pixi.lock files for environment version pinning.

---

## 2. Audit Logging and Reporting

### Current State

| Feature | Status | Evidence |
|---------|--------|----------|
| Execution events | Implemented | `executor.rs::ExecutionEvent` with structured JSON logging |
| NDJSON logging | Implemented | `ExecutionEvent::to_json_log()` produces structured logs |
| Web audit logs | Partial | `db.rs::AuditLog` table exists in web module |
| Checkpoint persistence | Implemented | `CheckpointState::save_to_file()` for resumable execution |
| Prometheus metrics | Implemented | `CheckpointState::to_prometheus_metrics()` |
| Clinical disclaimer | Implemented | `report.rs::clinical_disclaimer_section()` |

### Strengths

1. **Structured event logging**: NDJSON format suitable for log aggregation (Elasticsearch, Datadog)
2. **Timestamp precision**: ISO 8601 timestamps in all events
3. **Event granularity**: Tracks workflow_started, rule_started, rule_completed, rule_skipped, workflow_completed
4. **Clinical disclaimer**: Proper regulatory disclaimer in reports

### Gaps (CRITICAL)

1. **Audit log table lacks clinical context**: `db.rs::AuditLog` only captures user_id, action, target, timestamp. Missing:

   - Sample/specimen ID
   - Run ID linkage
   - Operator signature/authentication
   - Reason/justification for action
   - Before/after state for changes

   **Location**: `/crates/oxo-flow-web/src/db.rs:49-55`

   ```rust
   // Current schema lacks clinical context
   CREATE TABLE IF NOT EXISTS audit_logs (
       id TEXT PRIMARY KEY,
       user_id TEXT NOT NULL,
       action TEXT NOT NULL,
       target TEXT NOT NULL,
       timestamp DATETIME NOT NULL,
       FOREIGN KEY(user_id) REFERENCES users(id)
   );
   ```

   **Recommendation**: Extend audit schema to include:
   ```sql
   specimen_id TEXT,
   run_id TEXT,
   reason TEXT,
   old_value TEXT,  -- JSON snapshot of previous state
   new_value TEXT,  -- JSON snapshot of new state
   actor_role TEXT, -- 'analyst', ' reviewer', 'director'
   authentication_method TEXT -- 'password', 'badge', 'biometric'
   ```

2. **No audit trail for result review/approval**: Clinical workflows require documented result review and approval before release.

   **Recommendation**: Implement `ResultReview` workflow:
   - Analyst completes analysis -> Pending Review state
   - Second analyst reviews -> Approved state
   - Laboratory director signs off -> Released state
   - Each transition logged with timestamp, actor, comments

3. **Missing immutable audit storage**: Audit logs should be append-only with no delete capability.

   **Recommendation**: Implement audit log immutability:
   - Disable DELETE operations on audit_logs table
   - Store audit logs in separate database/file
   - Implement audit log archival process

4. **No electronic signature support**: CLIA requires documented signatures for result release.

   **Recommendation**: Implement signature capture:
   - Username/password authentication
   - Optional second-factor authentication
   - Signature timestamp and hash stored with results

---

## 3. QC Threshold Validation

### Current State

| Feature | Status | Evidence |
|---------|--------|----------|
| QcThreshold type | Implemented | `config.rs::QcThreshold` with min/max bounds and `passes()` method |
| QcMetric reporting | Implemented | `report.rs::QcMetric` captures coverage, mapping rate, duplicates |
| Threshold documentation | Documented | `VALIDATION_PROTOCOL.md` describes acceptance criteria |
| Biomarker thresholds | Implemented | `config.rs::BiomarkerResult::threshold` field |

### Strengths

1. **Threshold type with validation**: `QcThreshold::passes()` correctly validates min/max bounds
2. **QC metric capture**: Comprehensive QC metrics captured (total_reads, mapped_reads, mapping_rate, mean_coverage, duplicate_rate)
3. **Biomarker classification**: Supports threshold-based classification (e.g., MSI-H vs MSI-L)

### Gaps (HIGH PRIORITY)

1. **No automatic QC enforcement**: Thresholds are defined but not enforced during execution. Pipeline continues even if QC metrics fail.

   **Location**: `/crates/oxo-flow-core/src/config.rs:389-418`

   ```rust
   pub struct QcThreshold {
       pub metric: String,
       pub min: Option<f64>,
       pub max: Option<f64>,
       pub description: Option<String>,
   }
   // passes() method exists but never called during execution
   ```

   **Recommendation**: Implement QC validation step after QC-generating rules:
   - Parse QC output (fastp.json, alignment metrics)
   - Compare against defined thresholds
   - Fail pipeline if thresholds not met
   - Log QC status in provenance

2. **Missing QC threshold configuration in workflow**: No workflow-level QC threshold definition.

   **Recommendation**: Add `[qc_thresholds]` section to WorkflowConfig:
   ```toml
   [qc_thresholds]
   min_mapping_rate = 0.90
   min_mean_coverage = 30
   max_duplicate_rate = 0.20
   ```

3. **No QC alerting mechanism**: Failed QC should generate alerts before proceeding.

   **Recommendation**: Implement QC gate:
   - `oxo-flow qc-check` command
   - Integration with `oxo-flow run` to halt on QC failure
   - Option for `--qc-strict` (fail) vs `--qc-warn` (continue with warning)

4. **Missing QC trend tracking**: Historical QC metrics should be tracked for assay performance monitoring.

   **Recommendation**: Implement QC database:
   - Store QC metrics per run/sample
   - Generate QC trend reports
   - Flag runs with anomalous QC patterns

---

## 4. Clinical Sample Tracking

### Current State

| Feature | Status | Evidence |
|---------|--------|----------|
| SampleInfo type | Implemented | `report.rs::SampleInfo` with sample_id, patient_id, sample_type, collection_date |
| Sample struct (Venus) | Implemented | `venus/lib.rs::Sample` with name, fastq paths, is_tumor flag |
| TumorSampleMeta | Implemented | `config.rs::TumorSampleMeta` with tumor_purity, ploidy, match_id |
| Sample sheet validation | Implemented | `config.rs::validate_sample_sheet()` checks duplicates, format |
| GenePanel type | Implemented | `config.rs::GenePanel` for targeted analysis tracking |

### Strengths

1. **Sample metadata capture**: Comprehensive sample information fields
2. **Tumor-normal pairing**: `match_id` field supports paired analysis tracking
3. **Sample sheet validation**: Detects duplicate sample IDs, validates format
4. **Panel tracking**: Gene panel definition for targeted assays

### Gaps (HIGH PRIORITY)

1. **No LIMS integration**: Samples tracked internally but no integration with laboratory information management systems.

   **Recommendation**: Implement LIMS connector:
   - REST API for sample accessioning
   - Barcode/QR code sample identification
   - Sample status synchronization
   - Specimen receiving workflow

2. **Missing sample lifecycle tracking**: No tracking of sample states (received, extracted, sequenced, analyzing, complete).

   **Recommendation**: Implement sample lifecycle:
   ```rust
   enum SampleStatus {
       Received, Extracted, Sequenced, QCPassed,
       Analyzing, PendingReview, Approved, Released, Archived
   }
   ```

3. **No chain of custody documentation**: Clinical samples require documented chain of custody.

   **Recommendation**: Implement custody tracking:
   - Sample receipt timestamp and operator
   - Storage location tracking
   - Transfer documentation
   - Disposal tracking

4. **Missing sample batching**: Clinical labs process samples in batches; current design is sample-by-sample.

   **Recommendation**: Implement batch processing:
   - Batch accession number
   - Batch-level QC review
   - Batch release authorization

---

## 5. Venus Clinical Pipeline

### Current State

| Feature | Status | Evidence |
|---------|--------|----------|
| Pipeline steps | Implemented | Full somatic workflow: QC -> Align -> Dedup -> BQSR -> Call -> Filter -> Annotate -> Report |
| Analysis modes | Implemented | TumorOnly, NormalOnly, TumorNormal |
| Environment specs | Defined | Conda YAML specs for each tool |
| Clinical report rule | Implemented | `clinical_report` rule with HTML output |
| Software BOM | Documented | `venus-pipeline.md` lists versions |

### Strengths

1. **Complete workflow**: Implements industry-standard somatic calling pipeline
2. **Multiple callers**: Supports Mutect2 and Strelka2 for consensus calling
3. **CNV calling**: CNVkit integration for copy number analysis
4. **MSI detection**: Msisensor2 for microsatellite instability
5. **TMB calculation**: Tumor mutation burden calculation
6. **Clinical report generation**: Dedicated report output step

### Gaps (CRITICAL for Clinical Use)

1. **Missing validation against truth sets**: No reference sample validation integrated.

   **Recommendation**: Implement validation workflow:
   - NA12878/Genome in a Bottle for germline
   - Synthetic tumor datasets for somatic
   - Sensitivity/specificity metrics
   - Annual validation refresh

2. **No confirmatory testing workflow**: Clinical results often require Sanger confirmation.

   **Recommendation**: Implement confirmation workflow:
   - Flag variants requiring confirmation
   - Generate primer design request
   - Track confirmation results
   - Update variant status after confirmation

3. **Missing variant classification workflow**: ACMG/AMP classification not integrated.

   **Location**: `/crates/oxo-flow-core/src/config.rs:312-348` - `VariantClassification` enum exists but no classification workflow.

   **Recommendation**: Implement classification workflow:
   - Integrate ClinVar database lookup
   - Support ACMG criteria documentation
   - Allow manual classification override with justification
   - Track classification history

4. **No actionability annotation integration**: ActionabilityAnnotation type exists but not populated from databases.

   **Location**: `/crates/oxo-flow-core/src/config.rs:471-481`

   **Recommendation**: Integrate actionability databases:
   - OncoKB API integration
   - CIViC database lookup
   - FDA approval status
   - Clinical trial matching

5. **Missing proficiency testing workflow**: Clinical labs require documented proficiency testing.

   **Recommendation**: Implement PT workflow:
   - Quarterly PT sample processing
   - PT result submission tracking
   - PT performance review
   - Corrective action documentation

---

## 6. Report Generation (HTML/JSON Reports)

### Current State

| Feature | Status | Evidence |
|---------|--------|----------|
| HTML reports | Implemented | `report.rs::to_html()` generates self-contained HTML |
| JSON reports | Implemented | `report.rs::to_json()` for machine-readable output |
| Template engine | Implemented | Tera template system with `TemplateEngine` |
| Clinical disclaimer | Implemented | `clinical_disclaimer_section()` |
| Sample information | Implemented | `sample_info_section()` |
| QC metrics section | Implemented | `qc_metrics_section()` |
| Variant summary | Implemented | `variant_summary_section()` |
| Provenance section | Implemented | `provenance_section()` |
| Dark mode support | Implemented | CSS `prefers-color-scheme: dark` |
| Execution time chart | Implemented | SVG bar chart for timing visualization |

### Strengths

1. **Self-contained HTML**: All CSS embedded, no external dependencies
2. **Structured JSON**: Suitable for downstream processing and database ingestion
3. **Table of contents**: Auto-generated from section headings
4. **Clinical disclaimer**: Proper regulatory disclaimer included
5. **Provenance capture**: Reports include execution provenance

### Gaps (MEDIUM PRIORITY)

1. **Missing PDF generation**: Clinical reports typically require PDF format for archival.

   **Recommendation**: Implement PDF output:
   - Use `printpdf` crate or headless browser rendering
   - Support print-ready PDF with proper margins
   - Include page numbers and headers/footers

2. **No report archival system**: Generated reports not automatically archived.

   **Recommendation**: Implement report archival:
   - Auto-save reports with run ID timestamp
   - Version control for report regeneration
   - Long-term storage with retrieval interface

3. **Missing signature block**: Clinical reports require documented signatures.

   **Recommendation**: Add signature section:
   - Analyst signature with timestamp
   - Reviewer signature with timestamp
   - Director approval signature

4. **No report amendment workflow**: Reports may need post-release amendments.

   **Recommendation**: Implement amendment workflow:
   - Amendment reason documentation
   - Amendment versioning
   - Amendment notification to stakeholders

5. **Missing structured report sections**: `ClinicalReportSection` enum exists but not enforced.

   **Location**: `/crates/oxo-flow-core/src/config.rs:495-528`

   **Recommendation**: Enforce required clinical sections:
   - Specimen Information (mandatory)
   - Methodology (mandatory)
   - Results (mandatory)
   - Interpretation (mandatory)
   - Quality Control (mandatory)
   - Limitations (mandatory)
   - References (recommended)
   - Appendix (optional)

---

## 7. Regulatory Compliance Documentation

### Current State

| Feature | Status | Evidence |
|---------|--------|----------|
| IQ/OQ/PQ templates | Implemented | `docs/VALIDATION_PROTOCOL.md` |
| Change control template | Implemented | `docs/CHANGE_CONTROL.md` |
| Reproducibility documentation | Implemented | `REPRODUCIBILITY.md` |

### Strengths

1. **Validation protocol templates**: Comprehensive IQ/OQ/PQ documentation
2. **Change control process**: Defined change request workflow
3. **Reproducibility mechanisms**: Documented determinism guarantees

### Gaps (HIGH PRIORITY)

1. **No SOP templates**: Standard operating procedures not provided.

   **Recommendation**: Provide SOP templates for:
   - Sample receiving
   - Analysis workflow execution
   - QC review procedure
   - Result review and approval
   - Report generation and release
   - Corrective action procedure

2. **Missing training documentation**: No operator training materials.

   **Recommendation**: Develop training materials:
   - User guide with screenshots
   - Training checklist
   - Competency assessment form
   - Annual re-training schedule

3. **No incident management workflow**: Missing incident/corrective action process.

   **Recommendation**: Implement incident management:
   - Incident reporting form
   - Root cause analysis documentation
   - Corrective action plan
   - Effectiveness verification

---

## Summary of Critical Gaps

| Category | Gap | Priority | Regulatory Impact |
|----------|-----|----------|-------------------|
| Provenance | SHA-256 checksums | CRITICAL | Data integrity requirement |
| Provenance | Automatic persistence | CRITICAL | Audit trail requirement |
| Audit | Clinical context in logs | CRITICAL | CAP checklist requirement |
| Audit | Result review workflow | CRITICAL | CLIA result release requirement |
| Audit | Immutable audit storage | CRITICAL | Regulatory compliance |
| QC | Automatic enforcement | HIGH | Quality assurance requirement |
| QC | Threshold configuration | HIGH | Assay validation requirement |
| Sample | LIMS integration | HIGH | Sample tracking requirement |
| Sample | Chain of custody | HIGH | CAP sample handling requirement |
| Venus | Truth set validation | CRITICAL | Assay verification requirement |
| Venus | Variant classification | CRITICAL | Clinical interpretation requirement |
| Report | PDF generation | MEDIUM | Report archival requirement |
| Report | Signature blocks | MEDIUM | CLIA report requirement |
| Compliance | SOP templates | HIGH | CAP documentation requirement |
| Compliance | Training materials | HIGH | CLIA personnel requirement |

---

## Recommended Implementation Roadmap

### Phase 1: Foundation (CRITICAL - Before Pilot)

1. Implement SHA-256 checksum computation
2. Auto-persist execution provenance
3. Extend audit log schema with clinical context
4. Implement QC threshold enforcement
5. Add result review workflow

### Phase 2: Clinical Integration (HIGH - Before Production)

1. LIMS integration capability
2. Sample lifecycle tracking
3. Chain of custody documentation
4. Truth set validation workflow
5. Variant classification integration
6. SOP template generation

### Phase 3: Production Readiness (MEDIUM - Before Full Deployment)

1. PDF report generation
2. Report archival system
3. Signature blocks
4. Training documentation
5. Incident management workflow
6. Proficiency testing integration

---

## Conclusion

oxo-flow provides a well-architectured foundation for clinical pipeline execution with strong reproducibility guarantees and comprehensive reporting capabilities. However, critical gaps in audit logging, QC enforcement, sample tracking, and regulatory workflow integration must be addressed before deployment in CLIA/CAP-certified laboratories.

The existing documentation (VALIDATION_PROTOCOL.md, CHANGE_CONTROL.md, REPRODUCIBILITY.md) demonstrates awareness of regulatory requirements, but implementation of these requirements is incomplete.

**Approval Status**: Conditional approval for pilot deployment with documented plan for gap remediation. Full production deployment requires completion of Phase 1 and Phase 2 implementations.

---

**Reviewer Signature**: [Pending]
**Review Date**: 2026-05-14
**Next Review**: After Phase 1 implementation