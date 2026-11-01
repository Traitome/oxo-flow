//! Legacy web-boot entry points over the canonical SQLite layer.
//!
//! Schema and pool ownership live in [`crate::infra::db::sqlite`] — one
//! definition, one pool. `init_db` delegates to it and then layers the
//! web-boot-only seed (the system template library) on top. The second
//! schema/pool pair that used to live here let the two boot paths drift
//! until `tests/audit_integration.rs` pinned them together; the delegation
//! makes that pin true by construction.
//!
//! Failure-mode note: schema/pool initialization aborts via the canonical
//! `init_pool` retry-then-panic contract instead of returning `Err` — the
//! server boot terminates either way.

use anyhow::Result;
use chrono::Utc;
use sqlx::SqlitePool;
use uuid::Uuid;

/// Initialize the canonical SQLite backend (schema, global pool, global
/// storage backend) and seed the system templates. Idempotent: a second
/// call only re-runs the (INSERT OR IGNORE) template seed.
pub async fn init_db(database_url: &str) -> Result<()> {
    crate::infra::db::sqlite::init_pool(database_url).await;
    seed_system_templates(crate::infra::db::sqlite::pool()).await
}

/// Obtain a reference to the global pool.
///
/// # Panics
/// Panics if `init_db` has not been called yet.
pub fn pool() -> &'static SqlitePool {
    crate::infra::db::sqlite::pool()
}

/// Obtain a reference to the global pool, or `None` when it was never
/// initialized (PostgreSQL deployments). Background run-lifecycle tasks run
/// under `tokio::spawn`, where a panic here would leave runs stuck in
/// 'running' forever — those callers must degrade gracefully instead.
pub fn try_pool() -> Option<&'static SqlitePool> {
    crate::infra::db::sqlite::try_pool().ok()
}

/// Recover runs left in flight ('running'/'paused') after a server crash
/// (issue #79 P1-02: the old behavior blindly marked every in-flight run
/// failed, so a crash-restart lied about runs whose CLI kept executing).
///
/// With a recorded pid the truth is probed: a still-alive CLI is re-attached
/// to the process registry (cancel/pause/resume work again) and monitored to
/// its natural end; a dead one is attributed from the `.exit-code` record the
/// wrapper shell wrote (0 → completed, other → failed) or marked failed as
/// interrupted when no record exists. Rows without a pid keep the old
/// grace-period blind marking for legacy data.
pub async fn recover_orphaned_runs() -> Result<()> {
    let rows: Vec<(String, Option<i64>, Option<String>)> =
        sqlx::query_as("SELECT id, pid, workdir FROM runs WHERE status IN ('running', 'paused')")
            .fetch_all(pool())
            .await?;

    for (id, pid, workdir) in rows {
        match pid {
            Some(pid) => {
                let pid = pid as i32;
                if crate::process_control::probe_alive(pid) {
                    if !crate::process_control::looks_like_oxo_flow(pid) {
                        // The pid was recycled by an unrelated process — the
                        // CLI is gone and cannot be attributed.
                        tracing::warn!(
                            "Run {id}: pid {pid} is alive but not an oxo-flow process (recycled?) — marking failed"
                        );
                        crate::executor::finalize_run(
                            &id,
                            None,
                            &workdir
                                .as_deref()
                                .map(|wd| std::path::Path::new(wd).join("execution.log"))
                                .unwrap_or_default(),
                        )
                        .await;
                        continue;
                    }
                    // Re-attach: the CLI is the group leader (spawned with
                    // `process_group(0)`), so its pid doubles as the pgid.
                    crate::process_control::register(&id, pid);
                    tracing::info!("Run {id}: re-attached live CLI (pid {pid}) after restart");
                    if let Some(wd) = workdir {
                        crate::executor::resume_monitoring(id, pid, wd.into());
                    }
                } else {
                    // The CLI died while the server was down: attribute from
                    // the exit record instead of guessing. Stat first — a
                    // planted FIFO at .exit-code would hang startup recovery
                    // (issue #735).
                    let workdir = workdir.unwrap_or_default();
                    let exit_path = std::path::Path::new(&workdir).join(".exit-code");
                    let exit_code = std::fs::metadata(&exit_path)
                        .map(|m| m.is_file())
                        .unwrap_or(false)
                        .then(|| std::fs::read_to_string(&exit_path).ok())
                        .flatten()
                        .and_then(|c| c.trim().parse::<i32>().ok());
                    tracing::info!(
                        "Run {id}: CLI already dead after restart (exit record: {exit_code:?}) — attributing"
                    );
                    crate::executor::finalize_run(
                        &id,
                        exit_code,
                        &std::path::Path::new(&workdir).join("execution.log"),
                    )
                    .await;
                }
            }
            None => {
                // Legacy row without a recorded pid: nothing to probe. Keep
                // the old grace period so a run that just started (its pid
                // write raced the crash) is not falsely killed.
                let now = Utc::now();
                let cutoff = now - chrono::Duration::seconds(60);
                let result = sqlx::query(
                    "UPDATE runs SET status = 'failed', finished_at = ? WHERE id = ? AND status IN ('running','paused') AND started_at < ?",
                )
                .bind(now)
                .bind(&id)
                .bind(cutoff)
                .execute(pool())
                .await?;
                if result.rows_affected() > 0 {
                    tracing::warn!(
                        "Run {id}: no pid recorded — marked failed after grace period (legacy)"
                    );
                }
            }
        }
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Seed Data
// ---------------------------------------------------------------------------

async fn seed_system_templates(pool: &SqlitePool) -> Result<()> {
    let now = Utc::now();
    let templates = [
        (
            "hello-world",
            "Basic",
            "Simple starter workflow",
            "demo,simple",
            "[workflow]\nname = \"hello-world\"\nversion = \"1.0.0\"\ndescription = \"My first workflow\"\n\n[[rules]]\nname = \"greet\"\noutput = [\"hello.txt\"]\nshell = \"echo Hello, oxo-flow! > {output[0]}\"\n",
        ),
        (
            "wgs-germline",
            "Genomics",
            "WGS germline variant calling (fastp → bwa-mem2 → bcftools)",
            "wgs,variant-calling,germline",
            "[workflow]\nname = \"wgs-germline\"\nversion = \"1.0.0\"\ndescription = \"Basic germline WGS pipeline\"\n\n[config]\nref = \"/path/to/reference.fa\"\ndata = \"/path/to/fastq\"\nout = \"results\"\nsample = \"SAMPLE01\"\n\n[defaults]\nthreads = 4\nmemory = \"8G\"\n\n[[rules]]\nname = \"fastp_trim\"\ninput = [\"{config.data}/{config.sample}_R1.fastq.gz\", \"{config.data}/{config.sample}_R2.fastq.gz\"]\noutput = [\"{config.out}/trimmed_R1.fq.gz\", \"{config.out}/trimmed_R2.fq.gz\"]\nshell = \"mkdir -p {config.out}\\nfastp -i {input[0]} -I {input[1]} -o {output[0]} -O {output[1]} --thread {threads}\"\nthreads = 4\n[rules.environment]\nconda = \"envs/qc.yaml\"\n\n[[rules]]\nname = \"bwa_align\"\ninput = [\"{config.out}/trimmed_R1.fq.gz\", \"{config.out}/trimmed_R2.fq.gz\"]\noutput = [\"{config.out}/aligned.sam\"]\nshell = \"bwa-mem2 mem -t {threads} {config.ref} {input[0]} {input[1]} > {output[0]}\"\nthreads = 8\nmemory = \"16G\"\ncheckpoint = true\n[rules.environment]\nconda = \"envs/alignment.yaml\"\n\n[[rules]]\nname = \"call_variants\"\ninput = [\"{config.out}/aligned.sam\"]\noutput = [\"{config.out}/variants.vcf.gz\"]\nshell = \"samtools sort -@ 4 {input[0]} | bcftools mpileup -f {config.ref} - | bcftools call -mv -Oz -o {output[0]}\"\nthreads = 4\n[rules.environment]\nconda = \"envs/variant_calling.yaml\"\n",
        ),
        (
            "rnaseq-quantification",
            "Genomics",
            "RNA-seq quantification (fastp + Salmon)",
            "rnaseq,quantification,salmon",
            "[workflow]\nname = \"rnaseq-quantification\"\nversion = \"1.0.0\"\ndescription = \"RNA-seq quantification pipeline\"\n\n[config]\ndata = \"/path/to/fastq\"\nout = \"results\"\nsample = \"SAMPLE01\"\n\n[defaults]\nthreads = 4\nmemory = \"8G\"\n\n[[rules]]\nname = \"fastp_trim\"\ninput = [\"{config.data}/{config.sample}_R1.fastq.gz\", \"{config.data}/{config.sample}_R2.fastq.gz\"]\noutput = [\"{config.out}/trimmed_R1.fq.gz\", \"{config.out}/trimmed_R2.fq.gz\"]\nshell = \"mkdir -p {config.out}\\nfastp -i {input[0]} -I {input[1]} -o {output[0]} -O {output[1]} --thread {threads}\"\nthreads = 4\n[rules.environment]\nconda = \"envs/qc.yaml\"\n\n[[rules]]\nname = \"salmon_quant\"\ninput = [\"{config.out}/trimmed_R1.fq.gz\", \"{config.out}/trimmed_R2.fq.gz\"]\noutput = [\"{config.out}/quant.sf\"]\nshell = \"salmon quant -i /path/to/salmon_index -l A -1 {input[0]} -2 {input[1]} -o {config.out}/salmon -p {threads}\\ncp {config.out}/salmon/quant.sf {output[0]}\"\nthreads = 8\nmemory = \"16G\"\n[rules.environment]\nconda = \"envs/rnaseq.yaml\"\n",
        ),
        (
            "somatic-tumor-normal",
            "Genomics",
            "Somatic variant calling with matched pairs",
            "somatic,paired,tumor-normal",
            "[workflow]\nname = \"somatic-tumor-normal\"\nversion = \"1.0.0\"\ndescription = \"Somatic variant calling with matched tumor-normal pairs\"\n\n[config]\nref = \"/path/to/reference.fa\"\ndata = \"/path/to/fastq\"\nout = \"results\"\n\n[defaults]\nthreads = 4\nmemory = \"8G\"\n\n[[pairs]]\npair_id = \"CASE001\"\nexperiment = \"TUMOR01\"\ncontrol = \"NORMAL01\"\nexperiment_type = \"lung\"\n\n[[rules]]\nname = \"trim_experiment\"\ninput = [\"{config.data}/{experiment}_R1.fastq.gz\", \"{config.data}/{experiment}_R2.fastq.gz\"]\noutput = [\"{config.out}/{pair_id}/trim_T_R1.fq.gz\", \"{config.out}/{pair_id}/trim_T_R2.fq.gz\"]\nshell = \"mkdir -p {config.out}/{pair_id}\\nfastp -i {input[0]} -I {input[1]} -o {output[0]} -O {output[1]} --thread {threads}\"\nthreads = 4\n[rules.environment]\nconda = \"envs/qc.yaml\"\n\n[[rules]]\nname = \"trim_control\"\ninput = [\"{config.data}/{control}_R1.fastq.gz\", \"{config.data}/{control}_R2.fastq.gz\"]\noutput = [\"{config.out}/{pair_id}/trim_N_R1.fq.gz\", \"{config.out}/{pair_id}/trim_N_R2.fq.gz\"]\nshell = \"fastp -i {input[0]} -I {input[1]} -o {output[0]} -O {output[1]} --thread {threads}\"\nthreads = 4\n[rules.environment]\nconda = \"envs/qc.yaml\"\n\n[[rules]]\nname = \"somatic_call\"\ninput = [\"{config.out}/{pair_id}/trim_T_R1.fq.gz\", \"{config.out}/{pair_id}/trim_N_R1.fq.gz\"]\noutput = [\"{config.out}/{pair_id}/somatic.vcf.gz\"]\nshell = \"bcftools mpileup -f {config.ref} {input[0]} {input[1]} | bcftools call -mv -Oz -o {output[0]}\"\nthreads = 4\n[rules.environment]\nconda = \"envs/variant_calling.yaml\"\n",
        ),
        (
            "cohort-analysis",
            "Genomics",
            "Multi-sample cohort joint variant calling",
            "cohort,multi-sample,joint-calling",
            "[workflow]\nname = \"cohort-analysis\"\nversion = \"1.0.0\"\ndescription = \"Joint variant calling across multiple samples\"\n\n[config]\ndata = \"/path/to/fastq\"\nout = \"results\"\n\n[[sample_groups]]\nname = \"control\"\nsamples = [\"CTRL01\", \"CTRL02\", \"CTRL03\"]\n\n[[sample_groups]]\nname = \"case\"\nsamples = [\"CASE01\", \"CASE02\"]\n\n[[rules]]\nname = \"qc_per_sample\"\ninput = [\"{config.data}/{sample}_R1.fastq.gz\"]\noutput = [\"{config.out}/{sample}/qc.html\"]\nshell = \"mkdir -p {config.out}/{sample}\\nfastqc {input[0]} -o {config.out}/{sample}\"\nthreads = 2\n[rules.environment]\nconda = \"envs/qc.yaml\"\n\n[[rules]]\nname = \"cohort_summary\"\ninput = [\"{config.out}/CTRL01/qc.html\", \"{config.out}/CTRL02/qc.html\"]\noutput = [\"{config.out}/summary.txt\"]\nshell = \"echo Cohort QC complete > {output[0]}\"\n",
        ),
        (
            "scatter-gather",
            "Advanced",
            "Parallel chromosome-wise variant calling",
            "parallel,scatter-gather,transform",
            "[workflow]\nname = \"scatter-gather\"\nversion = \"1.0.0\"\ndescription = \"Scatter-gather parallel processing by chromosome\"\n\n[config]\nref = \"/path/to/reference.fa\"\n\n[[rules]]\nname = \"call_by_chromosome\"\ninput = [\"aligned.bam\"]\noutput = [\"variants.vcf.gz\"]\nshell = \"bcftools mpileup -f {config.ref} -r {_chrom} {input[0]} | bcftools call -mv -Oz -o {output[0]}\"\nthreads = 4\n\n[rules.transform]\nmap = \"bcftools mpileup -f {config.ref} -r {_chrom} {input[0]} | bcftools call -mv -Oz -o {output[0]}\"\n\n[rules.transform.split]\nby = \"_chrom\"\nvalues = [\"chr1\",\"chr2\",\"chr3\",\"chr4\",\"chr5\",\"chr6\",\"chr7\",\"chr8\",\"chr9\",\"chr10\",\"chr11\",\"chr12\",\"chr13\",\"chr14\",\"chr15\",\"chr16\",\"chr17\",\"chr18\",\"chr19\",\"chr20\",\"chr21\",\"chr22\",\"chrX\",\"chrY\"]\n\n[rules.transform.combine]\naggregate = true\nmethod = \"concat\"\n\n[rules.environment]\nconda = \"envs/variant_calling.yaml\"\n",
        ),
        (
            "conditional-pipeline",
            "Advanced",
            "Demonstrates conditional rule execution",
            "conditional,when,logic",
            "[workflow]\nname = \"conditional-pipeline\"\nversion = \"1.0.0\"\ndescription = \"Conditional execution based on config flags\"\n\n[config]\nrun_expensive_analysis = false\ndo_qc = true\nmin_quality = 30\n\n[[rules]]\nname = \"basic_qc\"\noutput = [\"qc_report.txt\"]\nshell = \"echo QC Report > {output[0]}\"\nwhen = \"config.do_qc\"\n\n[[rules]]\nname = \"expensive_analysis\"\ninput = [\"qc_report.txt\"]\noutput = [\"deep_analysis.txt\"]\nshell = \"echo Deep analysis > {output[0]}\"\nwhen = \"config.run_expensive_analysis && config.min_quality >= 20\"\n\n[[rules]]\nname = \"always_run\"\noutput = [\"summary.txt\"]\nshell = \"echo Pipeline summary > {output[0]}\"\n",
        ),
    ];

    for (name, category, description, tags, toml) in &templates {
        let id = Uuid::new_v4().to_string();
        sqlx::query(
            "INSERT OR IGNORE INTO templates (id, name, category, description, tags, toml_content, is_system, created_at, updated_at) VALUES (?, ?, ?, ?, ?, ?, 1, ?, ?)"
        )
        .bind(&id)
        .bind(name)
        .bind(category)
        .bind(description)
        .bind(tags)
        .bind(toml)
        .bind(now)
        .bind(now)
        .execute(pool)
        .await?;
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Audit persistence (storage backend for the audit middleware — issue #79
// P1-05). Everything else lives in `infra::db`.
// ---------------------------------------------------------------------------

/// Insert one audit record with explicit result + metadata.
pub async fn insert_audit_row(
    user_id: &str,
    action: &str,
    target: &str,
    result: &str,
    metadata: &str,
) -> Result<()> {
    let id = Uuid::new_v4().to_string();
    let now = Utc::now();
    sqlx::query(
        "INSERT INTO audit_logs (id, user_id, action, target, result, metadata, timestamp) VALUES (?, ?, ?, ?, ?, ?, ?)",
    )
    .bind(id)
    .bind(user_id)
    .bind(action)
    .bind(target)
    .bind(result)
    .bind(metadata)
    .bind(now)
    .execute(pool())
    .await?;
    Ok(())
}
