# Change Control

This document provides a change control template for deploying oxo-flow in
regulated environments such as clinical laboratories, CLIA-certified labs, and
facilities operating under ISO 15189 or CAP accreditation.

## Purpose

To ensure that all changes to the oxo-flow pipeline system are documented,
reviewed, assessed for risk, and approved before implementation in a production
environment.

## Scope

This change control process applies to:

- Updates to the oxo-flow software version
- Modifications to workflow definitions (`.oxoflow` files)
- Changes to reference data (genomes, annotations, databases)
- Changes to environment specifications (containers, conda envs)
- Infrastructure changes (hardware, OS, cluster configuration)

## Change Control Record

### Change Information

| Field | Value |
| ----- | ----- |
| **Change ID** | CC-YYYY-NNN |
| **Date requested** | |
| **Requested by** | |
| **Change description** | |
| **Reason for change** | |
| **Affected systems** | |
| **Priority** | Routine / Urgent / Emergency |

### Risk Assessment

| Risk Factor | Assessment |
| ----------- | ---------- |
| Patient impact | None / Low / Medium / High |
| Data integrity risk | None / Low / Medium / High |
| System downtime required | Yes / No |
| Rollback complexity | Simple / Moderate / Complex |
| Regulatory impact | None / Notification / Re-validation |

### Impact Analysis

- **Affected workflows**: List all pipelines and rules impacted by this change.
- **Affected outputs**: Describe how results may differ.
- **Validation required**: Specify whether IQ/OQ/PQ re-validation is needed
  (see [VALIDATION_PROTOCOL.md](VALIDATION_PROTOCOL.md)).
- **Training required**: Identify any user training needs.

### Approval

| Role | Name | Signature | Date |
| ---- | ---- | --------- | ---- |
| Requester | | | |
| Technical reviewer | | | |
| Quality manager | | | |
| Laboratory director | | | |

### Implementation Plan

1. **Pre-implementation**
   - Back up current configuration and workflow files
   - Document current software versions and checksums
   - Notify affected users

2. **Implementation**
   - Apply the change in a staging environment
   - Run validation tests (see VALIDATION_PROTOCOL.md)
   - Compare outputs against expected results
   - Document any deviations

3. **Post-implementation**
   - Verify the change in the production environment
   - Update documentation and SOPs
   - Archive the change control record
   - Monitor for unexpected behavior

### Verification

| Test | Expected Result | Actual Result | Pass/Fail |
| ---- | --------------- | ------------- | --------- |
| | | | |
| | | | |
| | | | |

### Rollback Plan

Describe the steps to revert this change if issues are discovered:

1. Restore previous oxo-flow version / workflow files from backup
2. Verify rollback by running validation tests
3. Notify affected users
4. Document the rollback and initiate a new change request if needed

---

*This template is provided as a starting point. Adapt it to your organization's
quality management system and regulatory requirements.*
