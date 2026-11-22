#!/usr/bin/env python3
"""Fault detection and repair-stage benchmark harness for oxo-flow.

Systematically introduces controlled biological and infrastructure faults into
candidate pipelines and execution environments to quantify independent stages:
1. Pre-execution static detection.
2. Diagnostic attribution to the affected rule.
3. Repair-proposal availability.
4. Repair application (measured — issue #1164).
5. Repair verification (measured — issue #1164).
6. Successful workflow execution after repair (measured — issue #1164).

Stages 4-6 run the REAL CLI per case in a "repair arm":

  oxo-flow run workflow_ai.oxoflow --ai-recover --ai-max-retries 1

with `[ai] auto_fix = "always"` in the workflow, then re-runs the (possibly
patched) workflow and parses the engine's repair narration (`REPAIR_APPLIED`
and sibling `REPAIR_*` prefixes, with a legacy fallback to the pre-#1164
"Fix applied to" line) plus the checkpoint's `repair_record` when present.
Without a provider configured (`OXO_FLOW_AI_PROVIDER` unset or `disabled`)
the repair stages report false with reason "provider not configured" —
never fabricated. The AI provider is read from the environment, never
hardcoded.

Executable-repair campaign arm: every case ships a reference repair (a full
workflow TOML) and runs against stub tool binaries (`bin/` prepended to
PATH) so the repaired workflow genuinely re-runs to success on the
measurement host WITHOUT real bioinformatics tools. Detection still fires
because the TYPE-*/SCI-* checks pattern-match the workflow TOML text and
command strings, not the binaries. Each case verifies locally that (a)
detection still fires on the seeded workflow and (b) the reference-repaired
workflow executes to success with stubs (`executability_verified`).

Controlled Fault Categories:
- BioTypes Contract Violations (fault manifests as a type-admission abort
  before rule execution; since #1265 that bail is routed through the same
  verified-repair loop as a rule failure, so `--ai-recover` engages here):
  * TYPE_SORT_MISMATCH: Queryname-sorted BAM piped to coordinate consumer (TYPE-002).
  * TYPE_REF_BUILD_CONFLICT: Assembly build conflict (e.g., GRCh37 vs GRCh38) (TYPE-004).
  * TYPE_INDEX_MISSING: Missing companion index (.bai/.csi/.tbi) (TYPE-003).
  * TYPE_COMPRESSION_CORRUPT: Non-BGZF or plain uncompressed stream (TYPE-006).
- Scientific Preflight Guardrail Violations (advisory at run time — warn,
  never abort — so the seeded workflow executes successfully):
  * SCI_VQSR_COHORT: VQSR invoked with small cohort N < 30 (SCI-VQSR-COHORT).
  * SCI_FEATURECOUNTS_STRAND: featureCounts invoked without strandness specification (SCI-FEATURECOUNTS-STRAND).
  * SCI_MUTECT2_TUMOR_ONLY: Somatic Mutect2 executed without panel of normals (SCI-MUTECT2-TUMOR-ONLY).
- Infrastructure Resource & System Faults (fault manifests as a genuine rule
  failure, parameterized by a declared `[config]` key so a resource change
  repair actually fixes it; rerun can succeed with stubs):
  * INFRA_OOM_SIGNAL: Task terminated via SIGKILL / exit code 137 (OOM Killer).
  * INFRA_TIMEOUT_SIGNAL: Task terminated via SIGXCPU (signal 24; shell exit 152 = 128+24).
  * INFRA_DISK_PRESSURE: Temporary scratch volume capacity exhaustion (ENOSPC).
  * INFRA_CPU_STARVATION: CPU-time-limit / EAGAIN thread starvation.
  * INFRA_OPEN_FILE_LIMIT: Open-file descriptor budget exhaustion (EMFILE).
  * INFRA_MISSING_SHARED_LIB: Exit 127 missing shared-library dependency.

Usage:
  python3 eval/frontier/fault_injection.py [--binary /path/to/oxo-flow]
      [--out-json report.json] [--logs-dir results/fault_repair_logs]
      [--skip-repair]
"""

from __future__ import annotations

import argparse
import hashlib
import json
import os
import re
import subprocess
import sys
import tempfile
import tomllib
from dataclasses import dataclass, field
from pathlib import Path
from typing import Any, Dict, List, Optional

sys.path.insert(0, os.path.join(os.path.dirname(os.path.abspath(__file__)), "..", "scripts"))
import common  # noqa: E402


@dataclass
class FaultCase:
    fault_id: str
    category: str
    description: str
    target_rule: str
    expected_diagnostic_code: str
    expected_repair_action: str
    toml_template: str
    # How the fault manifests at run time (drives the honest WHY for stages
    # 4-6 when the repair path cannot engage):
    #   "rule_failure"     — a rule exits non-zero; --ai-recover engages.
    #   "admission_abort"  — the BioType admission gate bails out before any
    #                        rule executes; since #1265 that bail is routed
    #                        through the same verified-repair machinery as a
    #                        rule failure (diagnose → patch → in-process
    #                        re-admission), so --ai-recover engages here too.
    #   "advisory_warning" — the finding is advisory (warn, never abort); the
    #                        seeded workflow executes successfully, so there
    #                        is no failure for AI recovery to engage.
    runtime_class: str
    # External input files the workflow's shell commands reference but no
    # rule produces; the harness touches them (empty) so stub-backed rules
    # never fail on missing inputs.
    seed_inputs: List[str] = field(default_factory=list)
    # Full repaired workflow TOML (the reference repair). The harness runs
    # it against the stub bin to prove the case is executable-repairable —
    # the repaired workflow genuinely re-runs to success without real tools.
    repaired_toml: Optional[str] = None
    executability_note: str = ""


def _vqsr_repaired(template: str) -> str:
    """Reference repair for SCI-VQSR-COHORT: raise the cohort to N=30."""
    samples = ", ".join(f'"S{i}"' for i in range(1, 31))
    return template.replace(
        'samples = ["S1", "S2"]', f"samples = [{samples}]"
    )


FAULT_CASES: List[FaultCase] = [
    FaultCase(
        fault_id="bio_sort_order_mismatch",
        category="biotypes",
        description="Queryname-sorted BAM supplied to downstream rule requiring coordinate sort order",
        target_rule="call_variants",
        expected_diagnostic_code="TYPE-002",
        expected_repair_action="InsertBridgeRule",
        runtime_class="admission_abort",
        seed_inputs=["ref.fa", "r1.fq", "r2.fq"],
        executability_note=(
            "reference repair inserts a samtools-sort bridge rule; the whole "
            "workflow runs to success against the stub bin"
        ),
        toml_template="""[workflow]
name = "fault_test_sort"

[types.bam_queryname]
format = "bam"
sort_order = "queryname"

[types.bam_coordinate]
format = "bam"
sort_order = "coordinate"

[[rules]]
name = "align_reads"
output = ["align.bam"]
shell = "bwa mem ref.fa r1.fq r2.fq > align.bam"
[rules.types]
outputs = { default = "bam_queryname" }

[[rules]]
name = "call_variants"
input = ["align.bam"]
output = ["raw.vcf"]
shell = "gatk HaplotypeCaller -I align.bam -O raw.vcf"
[rules.types]
inputs = { default = "bam_coordinate" }
""",
        repaired_toml="""[workflow]
name = "fault_test_sort"

[types.bam_queryname]
format = "bam"
sort_order = "queryname"

[types.bam_coordinate]
format = "bam"
sort_order = "coordinate"

[[rules]]
name = "align_reads"
output = ["align.bam"]
shell = "bwa mem ref.fa r1.fq r2.fq > align.bam"
[rules.types]
outputs = { default = "bam_queryname" }

[[rules]]
name = "sort_alignments"
input = ["align.bam"]
output = ["align.coord.bam"]
shell = "samtools sort -o align.coord.bam align.bam"
[rules.types]
inputs = { default = "bam_queryname" }
outputs = { default = "bam_coordinate" }

[[rules]]
name = "call_variants"
input = ["align.coord.bam"]
output = ["raw.vcf"]
shell = "gatk HaplotypeCaller -I align.coord.bam -O raw.vcf"
[rules.types]
inputs = { default = "bam_coordinate" }
""",
    ),
    FaultCase(
        fault_id="bio_missing_companion_index",
        category="biotypes",
        description="Downstream variant caller demands indexed BAM but producer omitted index",
        target_rule="index_dependent_analysis",
        expected_diagnostic_code="TYPE-003",
        expected_repair_action="InsertBridgeRule",
        runtime_class="admission_abort",
        seed_inputs=["in.sam"],
        executability_note=(
            "reference repair inserts an index bridge rule producing the "
            "indexed BAM type; runs to success against the stub bin"
        ),
        toml_template="""[workflow]
name = "fault_test_index"

[types.bam_indexed]
format = "bam"
sort_order = "coordinate"
index = "bai"

[types.bam_unindexed]
format = "bam"
sort_order = "coordinate"

[[rules]]
name = "produce_bam"
output = ["aligned.bam"]
shell = "samtools view -b in.sam > aligned.bam"
[rules.types]
outputs = { default = "bam_unindexed" }

[[rules]]
name = "index_dependent_analysis"
input = ["aligned.bam"]
output = ["stats.txt"]
shell = "samtools idxstats aligned.bam > stats.txt"
[rules.types]
inputs = { default = "bam_indexed" }
""",
        repaired_toml="""[workflow]
name = "fault_test_index"

[types.bam_indexed]
format = "bam"
sort_order = "coordinate"
index = "bai"

[types.bam_unindexed]
format = "bam"
sort_order = "coordinate"

[[rules]]
name = "produce_bam"
output = ["aligned.bam"]
shell = "samtools view -b in.sam > aligned.bam"
[rules.types]
outputs = { default = "bam_unindexed" }

[[rules]]
name = "index_alignments"
input = ["aligned.bam"]
output = ["aligned.idx.bam", "aligned.idx.bam.bai"]
shell = "samtools view -b -o aligned.idx.bam aligned.bam && samtools index aligned.idx.bam"
[rules.types]
inputs = { default = "bam_unindexed" }
outputs = { default = "bam_indexed" }

[[rules]]
name = "index_dependent_analysis"
input = ["aligned.idx.bam"]
output = ["stats.txt"]
shell = "samtools idxstats aligned.idx.bam > stats.txt"
[rules.types]
inputs = { default = "bam_indexed" }
""",
    ),
    FaultCase(
        fault_id="bio_reference_build_conflict",
        category="biotypes",
        description="Producer aligns to GRCh37 while consumer assumes GRCh38 reference coordinates",
        target_rule="annotate_variants",
        expected_diagnostic_code="TYPE-004",
        expected_repair_action="InsertBridgeRule",
        runtime_class="admission_abort",
        seed_inputs=["b37.fa", "in.bam", "hg38.map"],
        executability_note=(
            "reference repair inserts a liftover bridge rule producing the "
            "GRCh38 VCF type; runs to success against the stub bin"
        ),
        toml_template="""[workflow]
name = "fault_test_ref_build"

[types.vcf_b37]
format = "vcf"
reference_build = "GRCh37"

[types.vcf_hg38]
format = "vcf"
reference_build = "GRCh38"

[[rules]]
name = "call_germline"
output = ["raw.vcf"]
shell = "gatk HaplotypeCaller -R b37.fa -I in.bam -O raw.vcf"
[rules.types]
outputs = { default = "vcf_b37" }

[[rules]]
name = "annotate_variants"
input = ["raw.vcf"]
output = ["annotated.vcf"]
shell = "vep --assembly GRCh38 -i raw.vcf -o annotated.vcf"
[rules.types]
inputs = { default = "vcf_hg38" }
""",
        repaired_toml="""[workflow]
name = "fault_test_ref_build"

[types.vcf_b37]
format = "vcf"
reference_build = "GRCh37"

[types.vcf_hg38]
format = "vcf"
reference_build = "GRCh38"

[[rules]]
name = "call_germline"
output = ["raw.vcf"]
shell = "gatk HaplotypeCaller -R b37.fa -I in.bam -O raw.vcf"
[rules.types]
outputs = { default = "vcf_b37" }

[[rules]]
name = "lift_to_hg38"
input = ["raw.vcf"]
output = ["raw.hg38.vcf"]
shell = "bcftools annotate --rename-chrs hg38.map -o raw.hg38.vcf raw.vcf"
[rules.types]
inputs = { default = "vcf_b37" }
outputs = { default = "vcf_hg38" }

[[rules]]
name = "annotate_variants"
input = ["raw.hg38.vcf"]
output = ["annotated.vcf"]
shell = "vep --assembly GRCh38 -i raw.hg38.vcf -o annotated.vcf"
[rules.types]
inputs = { default = "vcf_hg38" }
""",
    ),
    FaultCase(
        fault_id="bio_compression_mismatch",
        category="biotypes",
        description="Plain uncompressed VCF provided where BGZF block compression is required",
        target_rule="tabix_index",
        expected_diagnostic_code="TYPE-006",
        expected_repair_action="InsertBridgeRule",
        runtime_class="admission_abort",
        seed_inputs=["in.bcf"],
        executability_note=(
            "reference repair inserts a bgzip bridge rule producing the BGZF "
            "VCF type; runs to success against the stub bin"
        ),
        toml_template="""[workflow]
name = "fault_test_bgzf"

[types.vcf_plain]
format = "vcf"
compression = "none"

[types.vcf_bgzf]
format = "vcf"
compression = "bgzip"

[[rules]]
name = "emit_vcf"
output = ["uncompressed.vcf"]
shell = "bcftools call in.bcf > uncompressed.vcf"
[rules.types]
outputs = { default = "vcf_plain" }

[[rules]]
name = "tabix_index"
input = ["uncompressed.vcf"]
output = ["uncompressed.vcf.tbi"]
shell = "tabix -p vcf uncompressed.vcf"
[rules.types]
inputs = { default = "vcf_bgzf" }
""",
        repaired_toml="""[workflow]
name = "fault_test_bgzf"

[types.vcf_plain]
format = "vcf"
compression = "none"

[types.vcf_bgzf]
format = "vcf"
compression = "bgzip"

[[rules]]
name = "emit_vcf"
output = ["uncompressed.vcf"]
shell = "bcftools call in.bcf > uncompressed.vcf"
[rules.types]
outputs = { default = "vcf_plain" }

[[rules]]
name = "bgzip_vcf"
input = ["uncompressed.vcf"]
output = ["uncompressed.vcf.gz"]
shell = "bcftools view -O z -o uncompressed.vcf.gz uncompressed.vcf"
[rules.types]
inputs = { default = "vcf_plain" }
outputs = { default = "vcf_bgzf" }

[[rules]]
name = "tabix_index"
input = ["uncompressed.vcf.gz"]
output = ["uncompressed.vcf.gz.tbi"]
shell = "tabix -p vcf uncompressed.vcf.gz"
[rules.types]
inputs = { default = "vcf_bgzf" }
""",
    ),
    FaultCase(
        fault_id="sci_vqsr_small_cohort",
        category="preflight",
        description="VQSR invoked with cohort size below statistical power floor",
        target_rule="recalibrate_variants",
        expected_diagnostic_code="SCI-VQSR-COHORT",
        expected_repair_action="ScaleResources",
        runtime_class="advisory_warning",
        seed_inputs=["ref.fa", "raw.vcf"],
        executability_note=(
            "reference repair raises the cohort to N=30; the workflow runs "
            "to success against the stub bin (the finding was advisory, so "
            "the seeded workflow also runs — no runtime failure exists)"
        ),
        toml_template="""[workflow]
name = "fault_test_vqsr"

[[sample_groups]]
name = "cohort"
samples = ["S1", "S2"]

[[rules]]
name = "recalibrate_variants"
input = ["raw.vcf"]
output = ["recal.table"]
shell = "gatk VariantRecalibrator -R ref.fa -V raw.vcf -O recal.table"
""",
        repaired_toml=_vqsr_repaired(
            """[workflow]
name = "fault_test_vqsr"

[[sample_groups]]
name = "cohort"
samples = ["S1", "S2"]

[[rules]]
name = "recalibrate_variants"
input = ["raw.vcf"]
output = ["recal.table"]
shell = "gatk VariantRecalibrator -R ref.fa -V raw.vcf -O recal.table"
"""
        ),
    ),
    FaultCase(
        fault_id="sci_featurecounts_no_strand",
        category="preflight",
        description="featureCounts quantification executed without explicit library strandness flag",
        target_rule="quantify_genes",
        expected_diagnostic_code="SCI-FEATURECOUNTS-STRAND",
        expected_repair_action="ScaleResources",
        runtime_class="advisory_warning",
        seed_inputs=["aligned.bam", "annotation.gtf"],
        executability_note=(
            "reference repair adds the explicit strandness flag (-s 2); runs "
            "to success against the stub bin (the finding was advisory)"
        ),
        toml_template="""[workflow]
name = "fault_test_strand"

[[rules]]
name = "quantify_genes"
input = ["aligned.bam"]
output = ["counts.txt"]
shell = "featureCounts -a annotation.gtf -o counts.txt aligned.bam"
""",
        repaired_toml="""[workflow]
name = "fault_test_strand"

[[rules]]
name = "quantify_genes"
input = ["aligned.bam"]
output = ["counts.txt"]
shell = "featureCounts -s 2 -a annotation.gtf -o counts.txt aligned.bam"
""",
    ),
    FaultCase(
        fault_id="sci_mutect2_tumor_only",
        category="preflight",
        description="Somatic variant discovery via Mutect2 without paired normal or panel of normals",
        target_rule="call_somatic",
        expected_diagnostic_code="SCI-MUTECT2-TUMOR-ONLY",
        expected_repair_action="ScaleResources",
        runtime_class="advisory_warning",
        seed_inputs=["tumor.bam", "ref.fa", "pon.vcf"],
        executability_note=(
            "reference repair adds a panel of normals; runs to success "
            "against the stub bin (the finding was advisory)"
        ),
        toml_template="""[workflow]
name = "fault_test_mutect2"

[[rules]]
name = "call_somatic"
input = ["tumor.bam"]
output = ["raw_somatic.vcf"]
shell = "gatk Mutect2 -R ref.fa -I tumor.bam -O raw_somatic.vcf"
""",
        repaired_toml="""[workflow]
name = "fault_test_mutect2"

[[rules]]
name = "call_somatic"
input = ["tumor.bam"]
output = ["raw_somatic.vcf"]
shell = "gatk Mutect2 -R ref.fa -I tumor.bam --panel-of-normals pon.vcf -O raw_somatic.vcf"
""",
    ),
    FaultCase(
        fault_id="infra_oom_failure",
        category="infrastructure",
        description=(
            "Process exceeded its memory allocation and terminated with exit "
            "code 137; the allocation is a declared [config] key so a "
            "resource-change repair genuinely fixes the fault"
        ),
        target_rule="high_memory_step",
        expected_diagnostic_code="OOM",
        expected_repair_action="ScaleResources",
        runtime_class="rule_failure",
        executability_note=(
            "fault is parameterized by [config] mem_mb (2048) against a "
            "workload needing 8192 MB; raising the allocation re-runs to "
            "success against the simtool stub"
        ),
        toml_template="""[workflow]
name = "fault_test_oom"

[config]
mem_mb = 2048

[[rules]]
name = "high_memory_step"
output = ["out.txt"]
shell = "simtool memhog --need-mb 8192 --have-mb {config.mem_mb} -o out.txt"

[rules.resources]
memory = "2G"
""",
        repaired_toml="""[workflow]
name = "fault_test_oom"

[config]
mem_mb = 8192

[[rules]]
name = "high_memory_step"
output = ["out.txt"]
shell = "simtool memhog --need-mb 8192 --have-mb {config.mem_mb} -o out.txt"

[rules.resources]
memory = "8G"
""",
    ),
    FaultCase(
        fault_id="infra_timeout_failure",
        category="infrastructure",
        description=(
            "Process exceeded wallclock allocation and received SIGXCPU "
            "(shell exit 152); the allocation is a declared [config] key so "
            "a resource-change repair genuinely fixes the fault"
        ),
        target_rule="compute_heavy_step",
        expected_diagnostic_code="timeout",
        expected_repair_action="ScaleResources",
        runtime_class="rule_failure",
        executability_note=(
            "fault is parameterized by [config] wall_min (10) against a "
            "workload needing 30 min; raising the allocation re-runs to "
            "success against the simtool stub"
        ),
        toml_template="""[workflow]
name = "fault_test_timeout"

[config]
wall_min = 10

[[rules]]
name = "compute_heavy_step"
output = ["summary.txt"]
shell = "simtool longrun --need-min 30 --have-min {config.wall_min} -o summary.txt"

[rules.resources]
time_limit = "10m"
""",
        repaired_toml="""[workflow]
name = "fault_test_timeout"

[config]
wall_min = 30

[[rules]]
name = "compute_heavy_step"
output = ["summary.txt"]
shell = "simtool longrun --need-min 30 --have-min {config.wall_min} -o summary.txt"

[rules.resources]
time_limit = "30m"
""",
    ),
    FaultCase(
        fault_id="infra_disk_pressure_enospc",
        category="infrastructure",
        description=(
            "Scratch volume space exhaustion causing ENOSPC error in stderr "
            "stream; the scratch budget is a declared [config] key so a "
            "storage-change repair genuinely fixes the fault"
        ),
        target_rule="spill_to_disk",
        expected_diagnostic_code="disk",
        expected_repair_action="RerouteScratch",
        runtime_class="rule_failure",
        executability_note=(
            "fault is parameterized by [config] scratch_gb (1) against a "
            "workload needing 10 GB; raising the budget re-runs to success "
            "against the simtool stub"
        ),
        toml_template="""[workflow]
name = "fault_test_enospc"

[config]
scratch_gb = 1

[[rules]]
name = "spill_to_disk"
output = ["temp_output.dat"]
shell = "simtool bigscratch --need-gb 10 --free-gb {config.scratch_gb} -o temp_output.dat"
""",
        repaired_toml="""[workflow]
name = "fault_test_enospc"

[config]
scratch_gb = 10

[[rules]]
name = "spill_to_disk"
output = ["temp_output.dat"]
shell = "simtool bigscratch --need-gb 10 --free-gb {config.scratch_gb} -o temp_output.dat"
""",
    ),
    FaultCase(
        fault_id="infra_cpu_thread_starvation",
        category="infrastructure",
        description=(
            "Thread-starved tool aborts with a CPU-time-limit / EAGAIN "
            "error; the thread allocation is a declared [config] key so a "
            "resource-change repair genuinely fixes the fault"
        ),
        target_rule="thread_heavy_step",
        expected_diagnostic_code="cpu_limit",
        expected_repair_action="ScaleResources",
        runtime_class="rule_failure",
        executability_note=(
            "fault is parameterized by [config] cpu_threads (2) against a "
            "workload needing 8 threads; raising the allocation re-runs to "
            "success against the simtool stub"
        ),
        toml_template="""[workflow]
name = "fault_test_cpu"

[config]
cpu_threads = 2

[[rules]]
name = "thread_heavy_step"
output = ["align_summary.txt"]
shell = "simtool cpushare --need-cpu 8 --have-cpu {config.cpu_threads} -o align_summary.txt"

[rules.resources]
threads = 2
""",
        repaired_toml="""[workflow]
name = "fault_test_cpu"

[config]
cpu_threads = 8

[[rules]]
name = "thread_heavy_step"
output = ["align_summary.txt"]
shell = "simtool cpushare --need-cpu 8 --have-cpu {config.cpu_threads} -o align_summary.txt"

[rules.resources]
threads = 8
""",
    ),
    FaultCase(
        fault_id="infra_open_file_limit",
        category="infrastructure",
        description=(
            "Tool exhausts its open-file descriptor budget and aborts with "
            "EMFILE (too many open files); the descriptor budget is a "
            "declared [config] key so a resource-change repair genuinely "
            "fixes the fault"
        ),
        target_rule="many_files_step",
        expected_diagnostic_code="ulimit",
        expected_repair_action="ScaleResources",
        runtime_class="rule_failure",
        executability_note=(
            "fault is parameterized by [config] fd_limit (1024) against a "
            "workload needing 4096 descriptors; raising the budget re-runs "
            "to success against the simtool stub"
        ),
        toml_template="""[workflow]
name = "fault_test_nofile"

[config]
fd_limit = 1024

[[rules]]
name = "many_files_step"
output = ["merged.vcf"]
shell = "simtool fdpool --need-fd 4096 --have-fd {config.fd_limit} -o merged.vcf"
""",
        repaired_toml="""[workflow]
name = "fault_test_nofile"

[config]
fd_limit = 4096

[[rules]]
name = "many_files_step"
output = ["merged.vcf"]
shell = "simtool fdpool --need-fd 4096 --have-fd {config.fd_limit} -o merged.vcf"
""",
    ),
    FaultCase(
        fault_id="infra_missing_shared_library",
        category="infrastructure",
        description=(
            "Tool aborts with exit 127 loading a missing shared library; "
            "the required ABI level is a declared [config] key so a "
            "configuration-change repair genuinely fixes the fault"
        ),
        target_rule="library_dependent_step",
        expected_diagnostic_code="shared_library",
        expected_repair_action="InsertBridgeRule",
        runtime_class="rule_failure",
        executability_note=(
            "fault is parameterized by [config] abi_level (3) against a "
            "toolchain requiring ABI level 7; raising the level re-runs to "
            "success against the simtool stub"
        ),
        toml_template="""[workflow]
name = "fault_test_libdep"

[config]
abi_level = 3

[[rules]]
name = "library_dependent_step"
output = ["report.txt"]
shell = "simtool libload --need-abi 7 --have-abi {config.abi_level} -o report.txt"
""",
        repaired_toml="""[workflow]
name = "fault_test_libdep"

[config]
abi_level = 7

[[rules]]
name = "library_dependent_step"
output = ["report.txt"]
shell = "simtool libload --need-abi 7 --have-abi {config.abi_level} -o report.txt"
""",
    ),
]


# ── Stub tool binaries (executable-repair campaign arm) ─────────────────────

# Shared helpers injected into every stub script. Rule shells do NOT inherit
# arbitrary env (issue #1108 allowlist) but DO receive PATH, so a stub bin
# prepended to PATH reaches the rule shell; the engine validates declared
# outputs post-rule, so stubs must CREATE every file the real tool writes.
_STUB_HELPERS = r"""mk() {
    mkdir -p "$(dirname "$1")" 2>/dev/null
    printf 'stub output for %s\n' "$1" > "$1"
}"""

# Generic tool stub: plausible stdout + create -o/--output/-O file arguments.
# `-O` values are treated as files only when they contain a dot (gatk's
# `-O raw.vcf` creates; bcftools' `-O z` format selector does not).
_STUB_GENERIC_TEMPLATE = r"""#!/bin/sh
# oxo-flow fault-injection benchmark stub: {name}
# Succeeds trivially: prints a plausible stdout line and creates the files a
# real invocation would write. Detection is unaffected: the TYPE-*/SCI-*
# checks pattern-match the workflow TOML text and command strings, not the
# binaries.
echo "[{name} (stub)] invoked: $*"
{helpers}
prev=""
for arg in "$@"; do
    case "$prev" in
        -o|--output)
            mk "$arg";;
        -O)
            # `-O` is a file target only when the value carries an extension
            # (gatk's `-O raw.vcf`); bcftools' `-O z` format selector is skipped.
            case "$arg" in
                *.*) mk "$arg";;
            esac;;
    esac
    case "$arg" in
        -o=*|--output=*) mk "${{arg#*=}}";;
    esac
    prev="$arg"
done
exit 0
"""

_STUB_SAMTOOLS = r"""#!/bin/sh
# oxo-flow fault-injection benchmark stub: samtools
# `index` also creates the companion .bai (TYPE-003 repair shape).
echo "[samtools (stub)] invoked: $*"
{helpers}
if [ "$1" = "index" ]; then
    last=""
    for a in "$@"; do last="$a"; done
    mk "$last.bai"
fi
prev=""
for arg in "$@"; do
    case "$prev" in
        -o|--output) mk "$arg";;
    esac
    case "$arg" in
        -o=*|--output=*) mk "${{arg#*=}}";;
    esac
    prev="$arg"
done
exit 0
"""

_STUB_TABIX = r"""#!/bin/sh
# oxo-flow fault-injection benchmark stub: tabix
# Creates the companion .tbi for the last positional input.
echo "[tabix (stub)] invoked: $*"
{helpers}
last=""
for a in "$@"; do
    case "$a" in -*) continue;; esac
    last="$a"
done
[ -n "$last" ] && mk "$last.tbi"
exit 0
"""

# Fault simulator: the INFRA-* faults are parameterized by a declared
# [config] key, so a resource-change repair (raise the allocation past the
# workload's stated requirement) genuinely flips the fault to success —
# while the unrepaired seeded workflow deterministically fails with the
# exact exit code / stderr the diagnostic engine classifies.
_STUB_SIMTOOL = r"""#!/bin/sh
# oxo-flow fault-injection benchmark stub: simtool (fault simulator)
# Modes: memhog (exit 137) / longrun (exit 152) / bigscratch (exit 1) when the
# declared allocation is below the workload's stated requirement; succeeds
# (and creates the -o output) when the allocation covers the requirement.
need=""; have=""; out=""
prev=""
for a in "$@"; do
    case "$prev" in
        --need-*) need="$a";;
        --have-*|--free-*) have="$a";;
        -o) out="$a";;
    esac
    prev="$a"
done
mode="$1"
mk() {
    mkdir -p "$(dirname "$1")" 2>/dev/null
    printf 'simtool %s output\n' "$mode" > "$1"
}
if [ -n "$have" ] && [ -n "$need" ] && [ "$have" -ge "$need" ] 2>/dev/null; then
    echo "simtool: allocation $have covers workload requirement $need — ok"
    [ -n "$out" ] && mk "$out"
    exit 0
fi
case "$mode" in
    memhog)
        echo "Out of memory (OOM): workload requires $need MB, allocated $have MB (SIGKILL)" >&2
        exit 137;;
    longrun)
        echo "Time limit exceeded (timeout): workload requires $need min, allocation $have min (SIGXCPU)" >&2
        exit 152;;
    bigscratch)
        echo "No space left on device (disk): workload requires ${need}G scratch, ${have}G free" >&2
        exit 1;;
    cpushare)
        echo "CPU time limit exceeded: workload needs $need threads, allocation $have (EAGAIN)" >&2
        exit 1;;
    fdpool)
        echo "tool error: too many open files (EMFILE): workload requires $need descriptors, allocated $have" >&2
        exit 1;;
    libload)
        echo "libtool: error while loading shared libraries: libbio.so.2: cannot open shared object file" >&2
        exit 127;;
    *)
        echo "simtool: unknown mode '$mode'" >&2
        exit 1;;
esac
"""


def build_stub_dir(workdir: Path) -> Path:
    """Materialize the stub bin/ dir for one case workdir."""
    stub_dir = workdir / "stubbin"
    stub_dir.mkdir(parents=True, exist_ok=True)
    scripts: Dict[str, str] = {}
    for name in ("gatk", "bcftools", "featureCounts", "bwa", "bwa-mem2", "vep", "samtools"):
        scripts[name] = (
            _STUB_SAMTOOLS if name == "samtools" else _STUB_GENERIC_TEMPLATE
        ).format(name=name, helpers=_STUB_HELPERS)
    scripts["tabix"] = _STUB_TABIX.format(helpers=_STUB_HELPERS)
    scripts["simtool"] = _STUB_SIMTOOL
    for name, body in scripts.items():
        path = stub_dir / name
        path.write_text(body)
        path.chmod(0o755)
    return stub_dir


def seed_input_files(case: FaultCase, workdir: Path) -> None:
    """Touch the external input files the case's shell commands reference."""
    for name in case.seed_inputs:
        path = workdir / name
        path.parent.mkdir(parents=True, exist_ok=True)
        path.touch()


# ── AI provider resolution (env-only; the orchestrator supplies these) ──────

# The repair arm reads the provider from the environment, never hardcodes
# one and never falls back to a saved host config: without these variables
# the harness runs in dry mode (provider pinned to "disabled"), which keeps
# the dry campaign deterministic even on hosts with a stale
# ~/.oxo-flow/ai_config.json. Values are never recorded — only which
# variable NAMES are present.
PROVIDER_ENV_VARS = (
    "OXO_FLOW_AI_PROVIDER",
    "OXO_FLOW_AI_API_URL",
    "OXO_FLOW_AI_MODEL",
    "OXO_FLOW_AI_TIMEOUT_SECS",
    "OXO_FLOW_AI_MAX_TOKENS",
    "OPENAI_BASE_URL",
    "OPENAI_API_KEY",
    "OPENAI_MODEL",
    "ANTHROPIC_BASE_URL",
    "ANTHROPIC_AUTH_TOKEN",
    "ANTHROPIC_MODEL",
    "DEEPSEEK_API_KEY",
)


def provider_from_env(env: Optional[Dict[str, str]] = None) -> Optional[Dict[str, Any]]:
    """Provider descriptor from env vars only, or None (= dry mode).

    Live mode requires an explicit non-disabled `OXO_FLOW_AI_PROVIDER`; the
    credential/base-url variables are passed through to the CLI untouched.
    """
    environ = os.environ if env is None else env
    provider = environ.get("OXO_FLOW_AI_PROVIDER", "").strip()
    if not provider or provider.lower() == "disabled":
        return None
    present = [name for name in PROVIDER_ENV_VARS if environ.get(name, "").strip()]
    return {"provider": provider, "provider_env_vars_present": present}


def cli_env(stub_dir: Path, dry: bool) -> Dict[str, str]:
    """Child env for CLI invocations: stub bin first on PATH; dry mode pins
    the provider to `disabled` (an explicit disabled always wins in the
    engine, so a host's saved ai_config.json cannot resurrect a provider)."""
    env = dict(os.environ)
    env["PATH"] = f"{stub_dir}{os.pathsep}{env.get('PATH', '')}"
    if dry:
        env["OXO_FLOW_AI_PROVIDER"] = "disabled"
    return env


AI_BLOCK = """
[ai]
enabled = true
auto_fix = "always"
"""


def workflow_with_ai(case: FaultCase) -> str:
    """Seeded template + the [ai] block the repair arm requires.

    Stages 1-3 lint the seeded file WITHOUT the block so detection stays
    deterministic (dry-run would otherwise attempt AI analysis when a
    provider resolves); only the repair arm runs the [ai]-enabled copy.
    """
    return case.toml_template.rstrip("\n") + "\n" + AI_BLOCK


# ── Repair narration / checkpoint parsing (issue #1164 contract) ────────────

# Engine contract (parallel #1164 engine work): the executor narrates
# `REPAIR_APPLIED ` (and sibling REPAIR_* prefixes) and records a
# `repair_record` in the checkpoint. Parse BOTH; fall back gracefully to the
# pre-#1164 narration ("Fix applied to" / "AI fix not auto-applied (...)")
# so the harness measures honestly against the current binary too.
REPAIR_NARRATION_RE = re.compile(r"REPAIR_[A-Z_]+")

LEGACY_APPLIED_MARKERS = ("Fix applied to",)
LEGACY_NOT_APPLIED_MARKERS = (
    "AI fix not auto-applied",
    "Failed to apply AI fix",
    "refusing to apply AI fix",
    "AI diagnosis failed",
)


def find_repair_record(checkpoint: Dict[str, Any]) -> Optional[Dict[str, Any]]:
    """Tolerant recursive lookup of a `repair_record` object in the
    checkpoint JSON (top-level or nested; the field does not exist in
    checkpoints written by binaries without the #1164 engine work)."""
    if not isinstance(checkpoint, dict):
        return None
    record = checkpoint.get("repair_record")
    if isinstance(record, dict):
        return record
    for value in checkpoint.values():
        if isinstance(value, dict):
            found = find_repair_record(value)
            if found is not None:
                return found
    return None


def _first_line_with(markers, text: str) -> str:
    for line in text.splitlines():
        for marker in markers:
            if marker in line:
                return line.strip()
    return ""


def assess_repair_stages(
    case: FaultCase,
    run_output: str,
    run_exit_code: Optional[int],
    checkpoint: Optional[Dict[str, Any]],
    rerun_exit_code: Optional[int],
    rerun_output: str,
    provider_configured: bool,
    workflow_sha_before: str,
    workflow_sha_after: str,
    workflow_after_text: str,
) -> Dict[str, Any]:
    """Classify measured stages 4-6 without conflating them with detection.

    All evidence comes from the repair arm's real CLI run: narration
    prefixes, the checkpoint `repair_record`, and rerun exit codes. Every
    false stage carries the exact WHY.
    """
    events = sorted(set(REPAIR_NARRATION_RE.findall(run_output)))

    record = find_repair_record(checkpoint) if checkpoint else None
    engine_self_rerun = (
        any(ev.startswith("REPAIR_RERUN_") for ev in events)
        or bool(record and record.get("rerun_attempted") is not None)
    )

    applied_evidence: List[str] = []
    if "REPAIR_APPLIED" in events:
        applied_evidence.append("narration:REPAIR_APPLIED")
    if record and record.get("applied") is True:
        applied_evidence.append("checkpoint:repair_record.applied")
    if not applied_evidence:
        legacy_line = _first_line_with(LEGACY_APPLIED_MARKERS, run_output)
        if legacy_line:
            applied_evidence.append("narration:fix-applied-legacy")
    repair_applied = bool(applied_evidence)

    # A fault counts as manifested when the seeded run failed OR when the
    # repair machinery engaged: the #1164 engine self-reruns after a verified
    # patch and exits 0 on success, so exit-0 no longer implies "no fault".
    fault_manifested = (
        run_exit_code not in (0, None)
        or repair_applied
        or bool(record and record.get("rerun_attempted") is True)
    )

    verified_evidence: List[str] = []
    if "REPAIR_RERUN_SUCCEEDED" in events:
        verified_evidence.append("narration:REPAIR_RERUN_SUCCEEDED")
    if "REPAIR_VERIFIED" in events:
        verified_evidence.append("narration:REPAIR_VERIFIED")
    if record and record.get("rerun_succeeded") is True:
        verified_evidence.append("checkpoint:repair_record.rerun_succeeded")
    if record and record.get("verified") is True:
        verified_evidence.append("checkpoint:repair_record.verified")
    if repair_applied and not verified_evidence and not engine_self_rerun and workflow_sha_after != workflow_sha_before:
        # Legacy-binary fallback: the engine accepted and wrote the patch but
        # has no self-rerun (pre-#1164). A patch that still parses as workflow
        # TOML counts as verified through the engine's own validation gate —
        # under the self-rerunning engine this fallback is intentionally
        # unavailable: parses-valid says nothing about the rerun outcome.
        try:
            tomllib.loads(workflow_after_text)
            verified_evidence.append("fallback:patched-toml-parses")
        except tomllib.TOMLDecodeError:
            pass
    repair_verified = bool(verified_evidence)

    engine_patched = workflow_sha_after != workflow_sha_before
    rerun_attempted = fault_manifested and (
        engine_self_rerun
        or run_exit_code not in (0, None)
    )
    rerun_succeeded = rerun_attempted and (
        "REPAIR_RERUN_SUCCEEDED" in events
        or bool(record and record.get("rerun_succeeded") is True)
        or (
            # Legacy mode: the harness's own rerun of the patched workflow.
            not engine_self_rerun
            and rerun_exit_code == 0
        )
    )

    # WHY for the first failing stage (application → verification → rerun).
    repair_why: Optional[str] = None
    rerun_failed_line = _first_line_with(("REPAIR_RERUN_FAILED",), run_output)
    if not fault_manifested:
        if case.runtime_class == "advisory_warning":
            repair_why = (
                "seeded run exited 0 — the SCI preflight finding is advisory "
                "(warn, never abort), so no runtime failure exists for AI "
                "recovery to engage"
            )
        else:
            repair_why = f"seeded run exited 0 (exit code {run_exit_code})"
    elif not repair_applied:
        if not provider_configured:
            repair_why = (
                "provider not configured (OXO_FLOW_AI_PROVIDER unset or "
                "disabled) — AI recovery cannot run"
            )
        else:
            narration_line = _first_line_with(LEGACY_NOT_APPLIED_MARKERS, run_output)
            if narration_line:
                repair_why = narration_line
            elif "AI Error Recovery" not in run_output:
                repair_why = (
                    "recovery path never engaged: the engine narrated no "
                    "AI Error Recovery block for this failure"
                    + (
                        f" ({case.runtime_class})"
                        if case.runtime_class != "rule_failure"
                        else ""
                    )
                )
            else:
                repair_why = (
                    "recovery ran but no applicable patch: no REPAIR_APPLIED "
                    "narration and no checkpoint repair_record"
                )
    elif rerun_failed_line:
        repair_why = rerun_failed_line
    elif not repair_verified:
        repair_why = (
            "patch written but not verified: no REPAIR_RERUN_SUCCEEDED "
            "narration, no checkpoint repair_record.rerun_succeeded, and no "
            "legacy patched-TOML fallback applies"
        )
    elif not rerun_succeeded:
        tail = _first_line_with(("Error:", "exit code", "✗"), rerun_output) or ""
        repair_why = (
            f"rerun of the {'patched' if engine_patched else 'unpatched'} "
            f"workflow failed (exit code {rerun_exit_code})"
            + (f": {tail}" if tail else "")
        )

    return {
        "fault_manifested": fault_manifested,
        "fault_run_exit_code": run_exit_code,
        "repair_applied": repair_applied,
        "applied_evidence": applied_evidence,
        "repair_verified": repair_verified,
        "verified_evidence": verified_evidence,
        "narration_repair_events": events,
        "checkpoint_repair_record_present": record is not None,
        "engine_patched_workflow": engine_patched,
        "rerun_attempted": rerun_attempted,
        "rerun_exit_code": rerun_exit_code,
        "rerun_succeeded": rerun_succeeded,
        # assess_case's stage-6 field, overridden with the measured value.
        "workflow_success_on_rerun": rerun_succeeded,
        "repair_stage_why": repair_why,
    }


def _sha256(path: Path) -> str:
    return hashlib.sha256(path.read_bytes()).hexdigest()


def _lint_diagnostics(stdout: str) -> List[Dict[str, Any]]:
    try:
        payload = json.loads(stdout)
    except json.JSONDecodeError:
        return []
    diagnostics = payload.get("diagnostics", [])
    return diagnostics if isinstance(diagnostics, list) else []


def _code_in_text(code: str, text: str) -> bool:
    """Case-insensitive, word-boundary search for a diagnostic code.

    The previous checks were inconsistent: a case-sensitive bare substring
    missed lowercase engine output for "OOM"/"timeout" (false negatives)
    while lowercase "disk" matched inside rule names like `spill_to_disk`
    (false positives). \\b keeps hyphenated codes ("TYPE-002") matchable and
    accepts hyphen-separated tokens ("disk pressure") but refuses embedded
    hits ("oom" in "bloom", "disk" in "spill_to_disk").
    """
    return re.search(rf"\b{re.escape(code)}\b", text, re.IGNORECASE) is not None


def assess_case(
    case: FaultCase,
    lint_stdout: str,
    dry_output: str,
    run_output: str,
    run_exit_code: Optional[int],
    workflow_before_sha256: str,
    workflow_after_sha256: str,
    measured_repair: Optional[Dict[str, Any]] = None,
) -> Dict[str, Any]:
    """Classify evidence without conflating detection, repair, and success.

    `measured_repair` (the repair arm's measurement) overrides the unset
    repair-stage defaults; without it the repair stages stay False with the
    historical "not_implemented" status.
    """
    diagnostics = _lint_diagnostics(lint_stdout)
    matching = [
        diagnostic
        for diagnostic in diagnostics
        if diagnostic.get("code") == case.expected_diagnostic_code
    ]
    combined_static = lint_stdout + "\n" + dry_output
    static_detected = bool(matching) or _code_in_text(case.expected_diagnostic_code, combined_static)

    attribution = any(
        diagnostic.get("rule") == case.target_rule for diagnostic in matching
    )
    if case.category == "infrastructure":
        lower_run = run_output.lower()
        attribution = (
            _code_in_text(case.expected_diagnostic_code, run_output)
            or "root cause:" in lower_run
        ) and case.target_rule.lower() in lower_run

    repair_proposal = any(
        bool(diagnostic.get("suggestion")) for diagnostic in matching
    )
    if case.category == "infrastructure":
        lower_run = run_output.lower()
        repair_proposal = (
            "suggestion:" in lower_run
            or "remediation:" in lower_run
            or "one-key remediation" in lower_run
        )

    # Repair stages default to unset (no repair-arm measurement); a provided
    # measurement replaces them with artifact-traceable values.
    repair_applied = False
    repair_verified = False
    workflow_success_on_rerun = False

    result = {
        "fault_id": case.fault_id,
        "category": case.category,
        "description": case.description,
        "target_rule": case.target_rule,
        "expected_code": case.expected_diagnostic_code,
        "expected_action": case.expected_repair_action,
        "runtime_class": case.runtime_class,
        "static_detected": static_detected,
        "diagnostic_attributed": attribution,
        "detected_codes": sorted(
            {
                diagnostic.get("code")
                for diagnostic in diagnostics
                if diagnostic.get("code")
            }
        ),
        "repair_proposed": repair_proposal,
        "repair_applied": repair_applied,
        "repair_verified": repair_verified,
        "workflow_success_on_rerun": workflow_success_on_rerun,
        "workflow_run_exit_code": run_exit_code,
        "workflow_sha256_before": workflow_before_sha256,
        "workflow_sha256_after": workflow_after_sha256,
        "workflow_unchanged": workflow_before_sha256 == workflow_after_sha256,
        "repair_status": "not_implemented",
        "case_status": "static_detected" if static_detected else "missed",
    }
    if measured_repair is not None:
        result.update(measured_repair)
        result["repair_status"] = "measured"
        if result["repair_applied"] and result["workflow_success_on_rerun"]:
            result["case_status"] = "repaired_and_rerun"
        elif result["static_detected"] or result["fault_manifested"]:
            result["case_status"] = "fault_confirmed"
    return result


def _run_capped(cmd: List[str], **kwargs: Any) -> subprocess.CompletedProcess:
    """subprocess.run under a 600 s cap (same ceiling as capture_workflow.py).

    A hung engine check must not wedge the whole fault campaign: the timeout
    is reported as a failed check (exit 124, the convention run_eval.py's
    gates use) instead of surfacing as an uncaught exception.
    """
    try:
        return subprocess.run(cmd, timeout=600, **kwargs)
    except subprocess.TimeoutExpired as exc:
        # POSIX run() re-raises without the partial streams, so they are
        # usually None; handle captured str output defensively too.
        stdout = exc.stdout if isinstance(exc.stdout, str) else ""
        stderr = exc.stderr if isinstance(exc.stderr, str) else ""
        return subprocess.CompletedProcess(cmd, returncode=124, stdout=stdout, stderr=stderr)


def _read_checkpoint(workdir: Path) -> Optional[Dict[str, Any]]:
    path = workdir / ".oxo-flow" / "checkpoint.json"
    if not path.is_file():
        return None
    try:
        return json.loads(path.read_text())
    except json.JSONDecodeError:
        return None


def _save_logs(logs_dir: Optional[Path], case_id: str, artifacts: Dict[str, str]) -> Dict[str, str]:
    """Persist raw logs under the run's logs dir; return relative paths."""
    if logs_dir is None:
        return {}
    case_dir = logs_dir / case_id
    case_dir.mkdir(parents=True, exist_ok=True)
    saved = {}
    for name, content in artifacts.items():
        (case_dir / name).write_text(content)
        saved[name] = str((case_dir / name).relative_to(logs_dir.parent))
    return saved


def _run_executability_check(
    binary: str, case: FaultCase, workdir: Path, stub_dir: Path
) -> Dict[str, Any]:
    """Prove the reference repair genuinely re-runs to success with stubs."""
    if case.repaired_toml is None:
        return {
            "executability_verified": False,
            "executability_exit_code": None,
            "executability_note": case.executability_note
            or "no reference repair — case cannot be made executable",
        }
    check_dir = workdir / "exec_check"
    check_dir.mkdir(exist_ok=True)
    (check_dir / "workflow.oxoflow").write_text(case.repaired_toml)
    seed_input_files(case, check_dir)
    res = _run_capped(
        [binary, "run", "workflow.oxoflow"],
        input="A\n",
        capture_output=True,
        text=True,
        cwd=check_dir,
        # The repaired workflow carries no [ai] block, but pin the provider
        # to disabled anyway so the check never dials out.
        env=cli_env(stub_dir, dry=True),
    )
    output = (res.stdout or "") + "\n" + (res.stderr or "")
    return {
        "executability_verified": res.returncode == 0,
        "executability_exit_code": res.returncode,
        "executability_note": case.executability_note,
        "executability_run_tail": output.strip()[-500:],
    }


def run_test_case(
    binary: str,
    case: FaultCase,
    provider: Optional[Dict[str, Any]] = None,
    logs_dir: Optional[Path] = None,
    measure_repair: bool = True,
) -> Dict[str, Any]:
    """Evaluate each evidence stage independently for one controlled fault.

    Stages 1-3 use the seeded workflow (static, deterministic). The repair
    arm (stages 4-6) runs the [ai]-enabled copy with the real CLI; without a
    provider the arm degrades to honest false-with-reason records.
    `measure_repair=False` restores the historical static-only measurement
    (stages 1-3 plus the legacy infra runtime probe, no stubs, no [ai]).
    """
    dry = provider is None
    with tempfile.TemporaryDirectory(prefix="oxo_fault_") as tmpdir:
        tmp_path = Path(tmpdir)
        wf_path = tmp_path / "workflow.oxoflow"
        wf_path.write_text(case.toml_template)
        before_hash = _sha256(wf_path)
        seed_input_files(case, tmp_path)
        stub_dir = build_stub_dir(tmp_path)

        # 1-3. Static Pre-Execution Detection (lint --json & dry-run).
        # No stub PATH and no [ai] block: these stages are static text
        # checks and must stay deterministic regardless of host config.
        lint_res = _run_capped(
            [binary, "lint", "--json", str(wf_path)],
            capture_output=True,
            text=True,
            cwd=tmp_path,
        )

        dry_res = _run_capped(
            [binary, "dry-run", str(wf_path)],
            capture_output=True,
            text=True,
            cwd=tmp_path,
        )
        dry_output = dry_res.stdout + "\n" + dry_res.stderr

        run_output = ""
        run_exit_code: Optional[int] = None
        if not measure_repair:
            # Historical static-only mode: infrastructure cases still run so
            # runtime attribution stays measurable, but no repair arm.
            if case.category == "infrastructure":
                run_res = _run_capped(
                    [binary, "run", str(wf_path)],
                    input="A\n",
                    capture_output=True,
                    text=True,
                    cwd=tmp_path,
                )
                run_output = run_res.stdout + "\n" + run_res.stderr
                run_exit_code = run_res.returncode
            return assess_case(
                case=case,
                lint_stdout=lint_res.stdout,
                dry_output=dry_output,
                run_output=run_output,
                run_exit_code=run_exit_code,
                workflow_before_sha256=before_hash,
                workflow_after_sha256=_sha256(wf_path),
            )

        measured: Dict[str, Any] = {}
        log_artifacts: Dict[str, str] = {
            "seeded_workflow.oxoflow": case.toml_template,
            "lint.json": lint_res.stdout + "\n" + lint_res.stderr,
            "dry_run.txt": dry_output,
        }

        # Repair arm (stages 4-6): real CLI run of the [ai]-enabled copy.
        ai_wf = tmp_path / "workflow_ai.oxoflow"
        ai_wf.write_text(workflow_with_ai(case))
        ai_before_hash = _sha256(ai_wf)
        repair_env = cli_env(stub_dir, dry=dry)
        fault_res = _run_capped(
            [binary, "run", ai_wf.name, "--ai-recover", "--ai-max-retries", "1"],
            input="A\n",
            capture_output=True,
            text=True,
            cwd=tmp_path,
            env=repair_env,
        )
        run_output = fault_res.stdout + "\n" + fault_res.stderr
        run_exit_code = fault_res.returncode
        checkpoint = _read_checkpoint(tmp_path)
        log_artifacts["fault_run.txt"] = run_output
        log_artifacts["workflow_ai.oxoflow"] = ai_wf.read_text()
        if checkpoint is not None:
            log_artifacts["checkpoint.json"] = json.dumps(checkpoint, indent=2, sort_keys=True)

        exec_result = _run_executability_check(binary, case, tmp_path, stub_dir)
        log_artifacts["exec_check_run.txt"] = exec_result.get("executability_run_tail", "")

        # Rerun the (possibly patched) workflow whenever the fault run
        # failed: proves fault determinism and measures post-repair success.
        rerun_output = ""
        rerun_exit_code: Optional[int] = None
        if run_exit_code not in (0, None):
            rerun_res = _run_capped(
                [binary, "run", ai_wf.name],
                input="A\n",
                capture_output=True,
                text=True,
                cwd=tmp_path,
                env=repair_env,
            )
            rerun_output = rerun_res.stdout + "\n" + rerun_res.stderr
            rerun_exit_code = rerun_res.returncode
            log_artifacts["rerun.txt"] = rerun_output

        ai_after_hash = _sha256(ai_wf)
        log_artifacts["workflow_ai.oxoflow.post_run"] = ai_wf.read_text()

        measured = assess_repair_stages(
            case=case,
            run_output=run_output,
            run_exit_code=run_exit_code,
            checkpoint=checkpoint,
            rerun_exit_code=rerun_exit_code,
            rerun_output=rerun_output,
            provider_configured=provider is not None,
            workflow_sha_before=ai_before_hash,
            workflow_sha_after=ai_after_hash,
            workflow_after_text=ai_wf.read_text(),
        )
        measured.update(exec_result)
        measured["measurement_mode"] = "dry" if dry else "live"
        measured["repair_run_cmd"] = " ".join(
            [binary, "run", ai_wf.name, "--ai-recover", "--ai-max-retries", "1"]
        )
        if provider is not None:
            # Variable NAMES only — never values.
            measured["provider_env_vars_present"] = provider["provider_env_vars_present"]

        saved = _save_logs(logs_dir, case.fault_id, log_artifacts)
        measured["raw_logs"] = saved

        return assess_case(
            case=case,
            lint_stdout=lint_res.stdout,
            dry_output=dry_output,
            run_output=run_output,
            run_exit_code=run_exit_code,
            workflow_before_sha256=before_hash,
            workflow_after_sha256=_sha256(wf_path),
            measured_repair=measured,
        )


def summarize_results(results: List[Dict[str, Any]]) -> Dict[str, Any]:
    total = len(results)

    def count(field: str) -> int:
        return sum(1 for result in results if result.get(field))

    def rate(field: str) -> float:
        return count(field) / total if total else 0.0

    modes = sorted({result.get("measurement_mode", "unmeasured") for result in results})
    return {
        "total_cases": total,
        "measurement_mode": modes[0] if len(modes) == 1 else "mixed",
        "static_detected": count("static_detected"),
        "diagnostic_attributed": count("diagnostic_attributed"),
        "repair_proposed": count("repair_proposed"),
        "executability_verified": count("executability_verified"),
        "fault_manifested": count("fault_manifested"),
        "repair_applied": count("repair_applied"),
        "repair_verified": count("repair_verified"),
        "rerun_attempted": count("rerun_attempted"),
        "workflow_success_on_rerun": count("workflow_success_on_rerun"),
        "static_detection_rate": rate("static_detected"),
        "diagnostic_attribution_rate": rate("diagnostic_attributed"),
        "repair_proposal_rate": rate("repair_proposed"),
        "executability_rate": rate("executability_verified"),
        "repair_application_rate": rate("repair_applied"),
        "repair_verification_rate": rate("repair_verified"),
        "post_repair_success_rate": rate("workflow_success_on_rerun"),
        "interpretation": (
            "Detection, repair application, verification, and rerun success "
            "are separate measured outcomes. The repair arm drives the real "
            "CLI per case; without a provider the repair stages report "
            "false with reason 'provider not configured'. Repair quality is "
            "judged by rerun success, not biological correctness (stubs, "
            "not real tools). Every count traces to a per-case record with "
            "raw logs."
        ),
    }


def main() -> None:
    parser = argparse.ArgumentParser(
        description="Fault detection and repair-stage benchmark harness"
    )
    parser.add_argument("--binary", default=None, help="Path to oxo-flow binary")
    parser.add_argument("--out-json", default=None, help="Path to export JSON benchmark results")
    parser.add_argument(
        "--logs-dir",
        default=None,
        help="Directory for per-case raw logs (default: results/fault_repair_logs beside this script)",
    )
    parser.add_argument(
        "--skip-repair",
        action="store_true",
        help="Skip the repair arm (stages 4-6); measure static stages only",
    )
    args = parser.parse_args()

    binary = str(Path(args.binary).resolve()) if args.binary else common.find_engine_binary()
    provider = provider_from_env()
    logs_dir = (
        Path(args.logs_dir).resolve()
        if args.logs_dir
        else Path(__file__).resolve().parent / "results" / "fault_repair_logs"
    )
    logs_dir.mkdir(parents=True, exist_ok=True)

    print("=" * 80)
    print("oxo-flow Fault Detection & Repair-Stage Benchmark")
    print(f"Target Binary: {binary}")
    print(f"Total Test Cases: {len(FAULT_CASES)}")
    if args.skip_repair:
        print("Repair arm: SKIPPED (--skip-repair)")
    elif provider:
        print(f"Repair arm: LIVE (provider={provider['provider']}, env-configured)")
    else:
        print("Repair arm: DRY (no provider env — repair stages report 'provider not configured')")
    print(f"Raw logs: {logs_dir}")
    print("=" * 80)

    results = []
    detected_count = 0

    for i, case in enumerate(FAULT_CASES, 1):
        res = run_test_case(
            binary,
            case,
            provider=provider,
            logs_dir=logs_dir,
            measure_repair=not args.skip_repair,
        )
        results.append(res)
        if res["static_detected"]:
            detected_count += 1

        def sym(flag: bool) -> str:
            return "✓" if flag else "✗"

        why = res.get("repair_stage_why") or "-"
        print(
            f"[{i:02d}/{len(FAULT_CASES):02d}] {sym(res['static_detected'])} {case.fault_id:<28} "
            f"| Cat: {case.category:<14} "
            f"| Static: {sym(res['static_detected'])} "
            f"| Attrib: {sym(res['diagnostic_attributed'])} "
            f"| Proposal: {sym(res['repair_proposed'])} "
            f"| Exec: {sym(res.get('executability_verified', False))} "
            f"| Manifest: {sym(res.get('fault_manifested', False))} "
            f"| Applied: {sym(res['repair_applied'])} "
            f"| Rerun: {sym(res['workflow_success_on_rerun'])}"
        )
        if res.get("repair_stage_why"):
            print(f"       why: {why}")

    summary = summarize_results(results)
    summary["results"] = results

    print("-" * 80)
    print(
        f"Static detection: {detected_count}/{len(FAULT_CASES)} "
        f"({summary['static_detection_rate']*100:.1f}%); "
        f"executability: {summary['executability_verified']}/{len(FAULT_CASES)}; "
        f"repair applied: {summary['repair_applied']}/{len(FAULT_CASES)} "
        f"({summary['measurement_mode']} mode); "
        f"rerun success: {summary['workflow_success_on_rerun']}/{len(FAULT_CASES)}"
    )
    print("-" * 80)

    if args.out_json:
        out_p = Path(args.out_json)
        out_p.parent.mkdir(parents=True, exist_ok=True)
        out_p.write_text(json.dumps(summary, indent=2))
        print(f"Detailed JSON results exported to: {out_p}")

    # Campaign gate: infrastructure faults are runtime-only (the static
    # gates cannot see a stderr OOM that has not happened yet), so the gate
    # checks static detection for the static fault classes and RUNTIME
    # attribution for the infrastructure class — plus the executable-repair
    # verification for every case.
    undetected = [
        res["fault_id"]
        for res in results
        if not (
            res["diagnostic_attributed"]
            if res["category"] == "infrastructure"
            else res["static_detected"]
        )
    ]
    if undetected:
        print("FAIL: detection regression for: " + ", ".join(undetected))
        sys.exit(1)
    unverified = [
        res["fault_id"] for res in results if not res.get("executability_verified")
    ]
    if unverified:
        print(
            "FAIL: executable-repair verification failed for: "
            + ", ".join(unverified)
        )
        sys.exit(1)


if __name__ == "__main__":
    main()
