use anyhow::Result;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sqlx::migrate::MigrateDatabase;
use sqlx::{SqlitePool, sqlite::SqlitePoolOptions};
use std::sync::OnceLock;
use tokio::sync::Mutex;
use uuid::Uuid;

/// Global SQLite connection pool.
static DB_POOL: OnceLock<SqlitePool> = OnceLock::new();

/// Guard to ensure initialization happens atomically across threads.
static INIT_GUARD: Mutex<()> = Mutex::const_new(());

/// Initialize the database, run migrations, and seed the default admin user.
///
/// When called a second time (e.g., from a later `#[tokio::test]`) this
/// function is a no-op: the existing pool is reused.  For in-memory SQLite
/// the pool is configured with a single persistent connection so all
/// concurrent queries share the same schema and data.
pub async fn init_db(database_url: &str) -> Result<()> {
    // Pool already initialized — no-op.
    if DB_POOL.get().is_some() {
        return Ok(());
    }

    // Acquire lock to ensure only one thread initializes at a time.
    let _guard = INIT_GUARD.lock().await;

    // Double-check after acquiring lock (another thread may have initialized).
    if DB_POOL.get().is_some() {
        return Ok(());
    }

    if !sqlx::Sqlite::database_exists(database_url)
        .await
        .unwrap_or(false)
    {
        sqlx::Sqlite::create_database(database_url).await?;
    }

    // In-memory SQLite databases are per-connection: use a single persistent
    // connection so all queries share the same schema and data.
    let is_memory = database_url.contains(":memory:");
    let mut opts = SqlitePoolOptions::new();
    if is_memory {
        opts = opts
            .max_connections(1)
            .min_connections(1)
            .idle_timeout(None)
            .max_lifetime(None);
    } else {
        opts = opts.max_connections(5);
    }
    let pool = opts.connect(database_url).await?;

    sqlx::query(
        r#"
        CREATE TABLE IF NOT EXISTS users (
            id TEXT PRIMARY KEY,
            username TEXT UNIQUE NOT NULL,
            role TEXT NOT NULL,
            auth_type TEXT NOT NULL,
            os_user TEXT NOT NULL,
            created_at DATETIME NOT NULL
        );

        CREATE TABLE IF NOT EXISTS runs (
            id TEXT PRIMARY KEY,
            user_id TEXT NOT NULL,
            workflow_name TEXT NOT NULL,
            status TEXT NOT NULL,
            pid INTEGER,
            started_at DATETIME,
            finished_at DATETIME,
            FOREIGN KEY(user_id) REFERENCES users(id) ON DELETE CASCADE
        );

        CREATE TABLE IF NOT EXISTS audit_logs (
            id TEXT PRIMARY KEY,
            user_id TEXT NOT NULL,
            action TEXT NOT NULL,
            target TEXT NOT NULL,
            result TEXT NOT NULL DEFAULT 'success',
            timestamp DATETIME NOT NULL,
            FOREIGN KEY(user_id) REFERENCES users(id) ON DELETE CASCADE
        );

        CREATE TABLE IF NOT EXISTS chat_sessions (
            id TEXT PRIMARY KEY,
            user_id TEXT NOT NULL DEFAULT 'default',
            title TEXT NOT NULL DEFAULT 'New Chat',
            created_at DATETIME NOT NULL DEFAULT (datetime('now')),
            updated_at DATETIME NOT NULL DEFAULT (datetime('now'))
        );

        CREATE INDEX IF NOT EXISTS idx_runs_user_id ON runs(user_id);
        CREATE INDEX IF NOT EXISTS idx_runs_status ON runs(status);
        CREATE INDEX IF NOT EXISTS idx_runs_started_at ON runs(started_at DESC);

        CREATE TABLE IF NOT EXISTS workflows (
            id TEXT PRIMARY KEY,
            user_id TEXT NOT NULL,
            name TEXT NOT NULL,
            version TEXT NOT NULL,
            toml_content TEXT NOT NULL,
            created_at DATETIME NOT NULL,
            updated_at DATETIME NOT NULL,
            FOREIGN KEY(user_id) REFERENCES users(id) ON DELETE CASCADE
        );

        CREATE TABLE IF NOT EXISTS sessions (
            token TEXT PRIMARY KEY,
            user_id TEXT NOT NULL,
            created_at DATETIME NOT NULL,
            expires_at DATETIME NOT NULL,
            FOREIGN KEY(user_id) REFERENCES users(id) ON DELETE CASCADE
        );

        CREATE TABLE IF NOT EXISTS scheduled_runs (
            id TEXT PRIMARY KEY,
            user_id TEXT NOT NULL,
            workflow_id TEXT NOT NULL,
            workflow_name TEXT NOT NULL,
            cron_expression TEXT NOT NULL,
            next_run_at DATETIME NOT NULL,
            last_run_at DATETIME,
            status TEXT NOT NULL DEFAULT 'active',
            created_at DATETIME NOT NULL,
            FOREIGN KEY(user_id) REFERENCES users(id) ON DELETE CASCADE,
            FOREIGN KEY(workflow_id) REFERENCES workflows(id) ON DELETE CASCADE
        );

        CREATE INDEX IF NOT EXISTS idx_scheduled_runs_next_run ON scheduled_runs(next_run_at);
        CREATE INDEX IF NOT EXISTS idx_scheduled_runs_status ON scheduled_runs(status);

        CREATE TABLE IF NOT EXISTS templates (
            id TEXT PRIMARY KEY,
            name TEXT NOT NULL,
            category TEXT NOT NULL DEFAULT 'general',
            description TEXT NOT NULL DEFAULT '',
            tags TEXT NOT NULL DEFAULT '',
            toml_content TEXT NOT NULL,
            is_system INTEGER NOT NULL DEFAULT 0,
            created_by TEXT,
            usage_count INTEGER NOT NULL DEFAULT 0,
            created_at DATETIME NOT NULL,
            updated_at DATETIME NOT NULL
        );

        CREATE TABLE IF NOT EXISTS hpc_jobs (
            id TEXT PRIMARY KEY,
            run_id TEXT NOT NULL,
            user_id TEXT NOT NULL,
            scheduler TEXT NOT NULL,
            job_id TEXT,
            partition_name TEXT,
            status TEXT NOT NULL DEFAULT 'pending',
            submitted_at DATETIME,
            completed_at DATETIME,
            FOREIGN KEY(run_id) REFERENCES runs(id) ON DELETE CASCADE,
            FOREIGN KEY(user_id) REFERENCES users(id) ON DELETE CASCADE
        );

        CREATE INDEX IF NOT EXISTS idx_hpc_jobs_run_id ON hpc_jobs(run_id);
        "#,
    )
    .execute(&pool)
    .await?;

    let count: (i64,) = sqlx::query_as("SELECT COUNT(*) FROM users")
        .fetch_one(&pool)
        .await?;

    if count.0 == 0 {
        let admin_id = Uuid::new_v4().to_string();
        let now = Utc::now();
        sqlx::query(
            "INSERT OR IGNORE INTO users (id, username, role, auth_type, os_user, created_at) VALUES (?, ?, ?, ?, ?, ?)"
        )
        .bind(admin_id)
        .bind("admin")
        .bind("admin")
        .bind("sudo")
        .bind("oxo-flow")
        .bind(now)
        .execute(&pool)
        .await?;
    }

    // Seed system templates if empty
    let template_count: (i64,) = sqlx::query_as("SELECT COUNT(*) FROM templates")
        .fetch_one(&pool)
        .await?;
    if template_count.0 == 0 {
        seed_system_templates(&pool).await?;
    }

    // `set` will fail if another thread beat us here; that is fine since we
    // already checked `is_some()` above and both pools point to the same DB.
    let _ = DB_POOL.set(pool);
    Ok(())
}

/// Obtain a reference to the global pool.
///
/// # Panics
/// Panics if `init_db` has not been called yet.
pub fn pool() -> &'static SqlitePool {
    DB_POOL
        .get()
        .expect("Database pool not initialized — call init_db() first")
}

/// Recover runs left in the 'running' state after a server crash.
pub async fn recover_orphaned_runs() -> Result<()> {
    let now = Utc::now();
    // Only mark runs as orphaned if they were started more than 60 seconds ago.
    // This avoids killing runs that are still initialising after a quick restart.
    let cutoff = now - chrono::Duration::seconds(60);
    let result = sqlx::query(
        "UPDATE runs SET status = 'failed', finished_at = ? WHERE status = 'running' AND started_at < ?",
    )
    .bind(now)
    .bind(cutoff)
    .execute(pool())
    .await?;

    if result.rows_affected() > 0 {
        tracing::warn!(
            "Recovered {} orphaned run(s) (started before {}). Runs started within the last 60s were left untouched.",
            result.rows_affected(),
            cutoff.format("%Y-%m-%d %H:%M:%S")
        );
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Models
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize, sqlx::FromRow)]
pub struct User {
    pub id: String,
    pub username: String,
    pub role: String,
    pub auth_type: String,
    pub os_user: String,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize, sqlx::FromRow)]
pub struct Run {
    pub id: String,
    pub user_id: String,
    pub workflow_name: String,
    pub status: String,
    pub pid: Option<i64>,
    pub started_at: Option<DateTime<Utc>>,
    pub finished_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, Serialize, Deserialize, sqlx::FromRow)]
pub struct AuditLog {
    pub id: String,
    pub user_id: String,
    pub action: String,
    pub target: String,
    pub timestamp: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize, sqlx::FromRow)]
pub struct ScheduledRun {
    pub id: String,
    pub user_id: String,
    pub workflow_id: String,
    pub workflow_name: String,
    pub cron_expression: String,
    pub next_run_at: DateTime<Utc>,
    pub last_run_at: Option<DateTime<Utc>>,
    pub status: String,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize, sqlx::FromRow)]
pub struct Session {
    pub token: String,
    pub user_id: String,
    pub created_at: DateTime<Utc>,
    pub expires_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize, sqlx::FromRow)]
pub struct Template {
    pub id: String,
    pub name: String,
    pub category: String,
    pub description: String,
    pub tags: String,
    pub toml_content: String,
    pub is_system: i64,
    pub created_by: Option<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize, sqlx::FromRow)]
pub struct HpcJob {
    pub id: String,
    pub run_id: String,
    pub user_id: String,
    pub scheduler: String,
    pub job_id: Option<String>,
    pub partition_name: Option<String>,
    pub status: String,
    pub submitted_at: Option<DateTime<Utc>>,
    pub completed_at: Option<DateTime<Utc>>,
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
// Repository Functions
// ---------------------------------------------------------------------------

pub async fn get_user_by_username(username: &str) -> Result<Option<User>> {
    let user = sqlx::query_as::<_, User>("SELECT * FROM users WHERE username = ?")
        .bind(username)
        .fetch_optional(pool())
        .await?;
    Ok(user)
}

pub async fn get_user_by_id(id: &str) -> Result<Option<User>> {
    let user = sqlx::query_as::<_, User>("SELECT * FROM users WHERE id = ?")
        .bind(id)
        .fetch_optional(pool())
        .await?;
    Ok(user)
}

pub async fn create_session(session: &Session) -> Result<()> {
    sqlx::query(
        "INSERT INTO sessions (token, user_id, created_at, expires_at) VALUES (?, ?, ?, ?)",
    )
    .bind(&session.token)
    .bind(&session.user_id)
    .bind(session.created_at)
    .bind(session.expires_at)
    .execute(pool())
    .await?;
    Ok(())
}

pub async fn get_session(token: &str) -> Result<Option<Session>> {
    let session =
        sqlx::query_as::<_, Session>("SELECT * FROM sessions WHERE token = ? AND expires_at > ?")
            .bind(token)
            .bind(Utc::now())
            .fetch_optional(pool())
            .await?;
    Ok(session)
}

pub async fn delete_session(token: &str) -> Result<()> {
    sqlx::query("DELETE FROM sessions WHERE token = ?")
        .bind(token)
        .execute(pool())
        .await?;
    Ok(())
}

pub async fn insert_run(run: &Run) -> Result<()> {
    sqlx::query(
        "INSERT INTO runs (id, user_id, workflow_name, status, pid, started_at, finished_at) VALUES (?, ?, ?, ?, ?, ?, ?)"
    )
    .bind(&run.id)
    .bind(&run.user_id)
    .bind(&run.workflow_name)
    .bind(&run.status)
    .bind(run.pid)
    .bind(run.started_at)
    .bind(run.finished_at)
    .execute(pool())
    .await?;
    Ok(())
}

pub async fn log_action(user_id: &str, action: &str, target: &str) -> Result<()> {
    let id = Uuid::new_v4().to_string();
    let now = Utc::now();
    sqlx::query(
        "INSERT INTO audit_logs (id, user_id, action, target, result, timestamp) VALUES (?, ?, ?, ?, 'success', ?)",
    )
    .bind(id)
    .bind(user_id)
    .bind(action)
    .bind(target)
    .bind(now)
    .execute(pool())
    .await?;
    Ok(())
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

// ---------------------------------------------------------------------------
// OutputRecord CRUD
// ---------------------------------------------------------------------------

/// An output record row stored in SQLite.
#[derive(Debug, sqlx::FromRow, Serialize, Deserialize)]
pub struct OutputRecordRow {
    pub id: String,
    pub run_id: String,
    pub rule: String,
    pub sample: Option<String>,
    pub file_path: String,
    pub file_size: i64,
    pub checksum: Option<String>,
    pub metrics: String,
    pub created_at: String,
}

impl OutputRecordRow {
    /// Convert from a core OutputRecord to a database row.
    pub fn from_core(record: &oxo_flow_core::result::OutputRecord, id: &str) -> Self {
        Self {
            id: id.to_string(),
            run_id: record.run_id.clone(),
            rule: record.rule.clone(),
            sample: record.sample.clone(),
            file_path: record.file_path.clone(),
            file_size: record.file_size as i64,
            checksum: record.checksum.clone(),
            metrics: serde_json::to_string(&record.metrics).unwrap_or_else(|_| "{}".to_string()),
            created_at: record.created_at.clone(),
        }
    }

    /// Convert to a core OutputRecord.
    pub fn to_core(&self) -> oxo_flow_core::result::OutputRecord {
        let metrics: std::collections::HashMap<String, serde_json::Value> =
            serde_json::from_str(&self.metrics).unwrap_or_default();
        oxo_flow_core::result::OutputRecord {
            rule: self.rule.clone(),
            run_id: self.run_id.clone(),
            sample: self.sample.clone(),
            file_path: self.file_path.clone(),
            file_size: self.file_size as u64,
            checksum: self.checksum.clone(),
            metrics,
            created_at: self.created_at.clone(),
        }
    }
}

/// Insert a batch of output records into the database.
pub async fn insert_output_records(records: &[OutputRecordRow]) -> anyhow::Result<()> {
    for record in records {
        sqlx::query(
            "INSERT OR IGNORE INTO output_records (id, run_id, rule, sample, file_path, file_size, checksum, metrics, created_at) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)"
        )
        .bind(&record.id)
        .bind(&record.run_id)
        .bind(&record.rule)
        .bind(&record.sample)
        .bind(&record.file_path)
        .bind(record.file_size)
        .bind(&record.checksum)
        .bind(&record.metrics)
        .bind(&record.created_at)
        .execute(pool())
        .await?;
    }
    Ok(())
}

/// Get all output records for a given run.
pub async fn get_output_records(run_id: &str) -> anyhow::Result<Vec<OutputRecordRow>> {
    let rows = sqlx::query_as::<_, OutputRecordRow>(
        "SELECT * FROM output_records WHERE run_id = ? ORDER BY created_at ASC",
    )
    .bind(run_id)
    .fetch_all(pool())
    .await?;
    Ok(rows)
}

/// List all templates from the database.
pub async fn list_templates() -> anyhow::Result<Vec<Template>> {
    let rows = sqlx::query_as::<_, Template>("SELECT * FROM templates ORDER BY category, name ASC")
        .fetch_all(pool())
        .await?;
    Ok(rows)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Test that the model structs can be constructed correctly.
    #[test]
    fn run_model_fields() {
        let run = Run {
            id: "test-id".to_string(),
            user_id: "user-1".to_string(),
            workflow_name: "test".to_string(),
            status: "pending".to_string(),
            pid: None,
            started_at: None,
            finished_at: None,
        };
        assert_eq!(run.id, "test-id");
        assert_eq!(run.status, "pending");
        assert!(run.pid.is_none());
    }

    #[test]
    fn audit_log_model_fields() {
        let log = AuditLog {
            id: "log-1".to_string(),
            user_id: "admin".to_string(),
            action: "run".to_string(),
            target: "test-workflow".to_string(),
            timestamp: Utc::now(),
        };
        assert_eq!(log.action, "run");
        assert_eq!(log.user_id, "admin");
    }

    #[test]
    fn session_model_fields() {
        let now = Utc::now();
        let session = Session {
            token: "abc123".to_string(),
            user_id: "admin".to_string(),
            created_at: now,
            expires_at: now,
        };
        assert_eq!(session.token, "abc123");
    }

    #[test]
    fn run_model_default_status() {
        let run = Run {
            id: "r1".into(),
            user_id: "u1".into(),
            workflow_name: "wf".into(),
            status: "pending".into(),
            pid: None,
            started_at: None,
            finished_at: None,
        };
        assert_eq!(run.status, "pending");
        assert!(run.started_at.is_none());
        assert!(run.finished_at.is_none());
    }

    #[test]
    fn run_model_with_pid() {
        let run = Run {
            id: "r2".into(),
            user_id: "u2".into(),
            workflow_name: "wf2".into(),
            status: "running".into(),
            pid: Some(12345),
            started_at: Some(Utc::now()),
            finished_at: None,
        };
        assert_eq!(run.pid, Some(12345));
        assert_eq!(run.status, "running");
    }

    #[test]
    fn session_model_expiry() {
        let created = Utc::now();
        let expires = created + chrono::Duration::hours(24);
        let session = Session {
            token: "tok".into(),
            user_id: "admin".into(),
            created_at: created,
            expires_at: expires,
        };
        assert!(session.expires_at > session.created_at);
    }

    #[test]
    fn user_model_fields() {
        let user = User {
            id: "user-1".to_string(),
            username: "testuser".to_string(),
            role: "user".to_string(),
            auth_type: "sudo".to_string(),
            os_user: "testuser".to_string(),
            created_at: Utc::now(),
        };
        assert_eq!(user.username, "testuser");
        assert_eq!(user.role, "user");
        assert_eq!(user.auth_type, "sudo");
        assert_eq!(user.os_user, "testuser");
    }
}
