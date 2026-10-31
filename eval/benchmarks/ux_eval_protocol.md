# Empirical Human-Factors & User Experience Evaluation Protocol

## 1. Executive Summary & Scientific Purpose

Bioinformatic workflow managers are software systems whose primary users are biological researchers, computational biologists, and clinical assay operators. Existing systems (Nextflow and Snakemake) force users into mental fragmentation: users must write in general-purpose languages (Groovy, Python) embedded in custom DSLs, manually diagnose obscure terminal tracebacks, and parse cryptic stack traces.

This protocol defines a rigorous, randomized within-subject experimental trial to quantify the cognitive debugging load, time-to-first-working-pipeline, and System Usability Scale (SUS) across three platforms:

- **Platform A**: Nextflow DSL2 (Groovy-based scripting runtime).
- **Platform B**: Snakemake (Python-based rule specification).
- **Platform C**: `oxo-flow` (Type-directed TOML + Metro-DAG Studio + 1-Click Remediation).

---

## 2. Experimental Design & Methodology

### 2.1 Participant Cohort Stratification

The study evaluates $N = 30$ independent bioinformaticians recruited from research universities, cancer genome centers, and biotechnology enterprises, stratified into three proficiency tiers:

- **Novice Cohort ($n=10$)**: Researchers with $<1$ year of bioinformatics experience, familiar with bash and high-level tools, but limited DSL experience.
- **Intermediate Cohort ($n=10$)**: Computational biologists with 1–3 years experience managing production pipelines on HPC clusters.
- **Expert Cohort ($n=10$)**: Senior bioinformaticians and pipeline engineers with $>3$ years experience maintaining enterprise or nf-core pipelines.

### 2.2 Counterbalanced Latin Square Crossover

To prevent learning and carryover effects, the presentation order of platforms is balanced across participants using a $3 \times 3$ Latin square design:

1. Sequence 1: Nextflow $\to$ Snakemake $\to$ `oxo-flow`
2. Sequence 2: Snakemake $\to$ `oxo-flow` $\to$ Nextflow
3. Sequence 3: `oxo-flow` $\to$ Nextflow $\to$ Snakemake

---

## 3. Standardized Task Battery

Each participant completes three distinct, timed experimental tasks on each platform:

### Task 1: Pipeline Construction from Intent (Authoring)

- **Objective**: Author a paired-end RNA-seq pipeline with quality trimming (`fastp`), spliced read alignment (`STAR`), and transcript quantification (`Salmon`).
- **Success Criteria**: Pipeline passes static validation and successfully produces transcript abundance matrices.
- **Metrics**: Time-to-First-Pass (TTFP, seconds) and total manual syntax edits.

### Task 2: Biological Contract Mismatch Triage (Biological Debugging)

- **Scenario**: A variant-calling rule (`gatk HaplotypeCaller`) consumes an alignment file produced by an upstream tool sorted by queryname (`bwa mem | samtools view -b`) without coordinate sorting or index generation.
- **Success Criteria**: Identify the root cause and insert the appropriate coordinate sort and indexing steps.
- **Metrics**: Time to diagnose (seconds), number of trial-and-error executions before resolution.

### Task 3: Infrastructure OOM & Scratch Pressure Remediation (Runtime Self-Healing)

- **Scenario**: An alignment rule fails with SIGKILL exit code 137 (OOM Killer) due to an under-provisioned container memory ceiling, and scratch disk space is exhausted.
- **Success Criteria**: Recover execution state without re-running completed upstream rules and successfully complete the pipeline.
- **Metrics**: Recovery latency (seconds), cognitive load score, number of manual configuration file edits.

---

## 4. Quantitative Instrumentation & Psychometric Instruments

### 4.1 System Usability Scale (SUS)

The System Usability Scale (Brooke 1996) consists of 10 standardized 5-point Likert items (1 = Strongly Disagree, 5 = Strongly Agree):

1. I think that I would like to use this system frequently.
2. I found the system unnecessarily complex.
3. I thought the system was easy to use.
4. I think that I would need the support of a technical person to be able to use this system.
5. I found the various functions in this system were well integrated.
6. I thought there was too much inconsistency in this system.
7. I would imagine that most people would learn to use this system very quickly.
8. I found the system very cumbersome to use.
9. I felt very confident using the system.
10. I needed to learn a lot of things before I could get going with this system.

### 4.2 NASA Task Load Index (NASA-TLX)

Cognitive burden is measured across 6 subscales (0–100 scale):

- **Mental Demand**: How much mental and perceptual activity was required?
- **Temporal Demand**: How much time pressure did you feel due to the pace?
- **Performance**: How successful do you think you were in accomplishing the goals?
- **Effort**: How hard did you have to work to accomplish your level of performance?
- **Frustration**: How insecure, discouraged, irritated, and annoyed did you feel?

---

## 5. Statistical Hypothesis & Power Analysis

- **Hypothesis 1 ($H_1$)**: `oxo-flow` exhibits significantly higher SUS scores compared to Nextflow and Snakemake ($\mu_{\text{oxo}} > 80$, Grade A; $\mu_{\text{incumbents}} < 60$, Grade D/F; $p < 0.001$, paired Student's $t$-test / Wilcoxon signed-rank test).
- **Hypothesis 2 ($H_2$)**: `oxo-flow` reduces Time-to-First-Working-Pipeline (TTFP) by $>60\%$ compared to manual DSL scripting.
- **Hypothesis 3 ($H_3$)**: 1-Click / 1-Key remediation reduces infrastructure failure recovery latency from $>15$ minutes to $<30$ seconds.
