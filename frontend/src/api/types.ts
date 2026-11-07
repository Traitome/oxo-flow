// ── Health & System ──
export interface EngineBinaryInfo {
  version: string;
  path: string;
  version_mismatch: boolean;
}

export interface HealthResponse {
  status: string;
  version: string;
  mode: string;
  uptime_secs: number;
  components: {
    database: { status: string; latency_ms: number | null };
    filesystem: { status: string; latency_ms: number | null };
    scheduler: { status: string; latency_ms: number | null } | null;
    ai_provider: { status: string; latency_ms: number | null } | null;
    ai_key_storage?: string;
    engine?: EngineBinaryInfo | null;
  };
  resources: { cpu_pct: number; memory_used_pct: number; disk_used_pct: number };
  license: { license_type: string; valid: boolean; commercial_use: string; contact: string; message: string };
}

export interface SystemInfo {
  version: string;
  rust_version: string;
  os: string;
  arch: string;
  pid: number;
  uptime_secs: number;
  engine?: EngineBinaryInfo | null;
}

export interface RuntimeMetrics {
  uptime_secs: number; version: string; pid: number; os: string; arch: string;
  cpu_count: number; total_requests: number; active_workflows: number;
  host: { cpu_usage_percent: number; total_memory_mb: number; used_memory_mb: number; total_swap_mb: number; used_swap_mb: number };
}

// ── Auth ──
export interface LoginResponse { token: string; username: string; role: string; }
export interface UserInfo { id: string; username: string; role: string; auth_type?: string; os_user?: string | null; created_at: string; }

// ── Pipeline ──
export interface Pipeline {
  id: string; user_id: string; name: string; version: string; toml_content: string;
  rules_count: number; forked_from?: string | null; visibility: string; created_at: string; updated_at: string;
}
export interface ValidateResponse {
  valid: boolean;
  errors: Array<{ code: string; message: string; rule: string | null; suggestion: string | null; line?: number | null }>;
  warnings?: Array<{ code: string; message: string; rule: string | null; suggestion: string | null; line?: number | null }>;
  rules: number;
  dependencies: number;
  missing_inputs: string[];
}
export interface ParseResponse {
  pipeline_id: string; name: string; version: string;
  rules: Array<{ name: string; shell: string | null; inputs: string[]; outputs: string[]; environment: string | null; threads: number | null }>;
  dag: DagJson; stats: Record<string, unknown>;
}
export interface DagJsonNode {
  id: string;
  label: string;
  color: string;
  environment: string;
  /** Complete serialized rule — canvas cards and the inspector read from here. */
  rule: Record<string, unknown>;
}

export interface DagJsonEdge {
  from: string;
  to: string;
  /** "declared" = explicit depends_on; "file" = inferred from input/output match. */
  kind: 'declared' | 'file';
}

export interface DagJson {
  nodes: DagJsonNode[];
  edges: DagJsonEdge[];
  parallel_groups?: Array<Array<string>>; critical_path?: Array<string>;
  /** Aggregate shape metrics (Rust `DagMetrics` — always present on the response). */
  metrics: {
    node_count: number;
    edge_count: number;
    max_depth: number;
    max_width: number;
    critical_path_length: number;
    parallel_group_count: number;
  };
}

export interface KnowledgeTool {
  name: string;
  version: string;
  summary: string;
  platforms: string[];
}

export interface KnowledgeToolsResponse {
  total: number;
  tools: KnowledgeTool[];
}

// ── Runs ──
/** Cursor-paginated run list envelope (issue #82 P1-3/P1-13). */
export interface RunList {
  items: RunItem[];
  next_cursor: string | null;
  total: number;
}

export interface RunItem {
  id: string; user_id: string; pipeline_id: string | null;
  /** Workflow name the run executed (backend enriches the list — issue #82 P1-3). */
  workflow_name?: string | null;
  /** The TOML the run executed (get_run only — powers Manual Edit). */
  pipeline_snapshot?: string;
  /** True when the run was a --dry-run plan (persisted since #907 P3-14). */
  dry_run?: boolean;
  status: string; phase: string; workdir: string | null;
  started_at: string | null; finished_at: string | null; created_at: string;
}
/** One expanded sample×rule instance from a run's checkpoint
 *  (issue #82 P1-1): `qc_auto-discovered_S1` → rule `qc`, sample `S1`. */
export interface RunInstance {
  instance: string;
  rule: string;
  sample?: string;
  group?: string;
  status: string;
  duration_ms?: number;
  exit_code?: number;
  /** Bounded stderr tail of a failed rule (issue #758). */
  stderr_tail?: string;
  /** Bounded stdout tail of a failed rule (issue #765) — some tools
   *  print their root cause on stdout. */
  stdout_tail?: string;
}

/** Public payload of a share link's landing page (issue #82 P0-6). */
export interface ShareLanding {
  pipeline: { name: string; version: string; rules_count: number; visibility: string };
  dag: string[];
  toml_content: string;
  owner?: string | null;
  created_at: string;
  expires_at?: string | null;
  recent_run?: { status: string; finished_at?: string } | null;
}

/** Live contract of `GET /api/runs/{id}/status` (Rust `RunStatusResponse`,
 *  still served — server.rs). Pinned by tests/frontend_types_drift.rs:
 *  no page calls it today, but the drift gate is the consumer — remove
 *  only together with its pin. */
export interface RunStatus {
  status: string; phase: string;
  nodes: Array<{ rule: string; status: string; started_at: string | null; duration_ms: number | null; exit_code: number | null; stderr_tail?: string; stdout_tail?: string }>;
  timeline: Array<{ timestamp: string; event: string; node: string | null; message: string | null }>;
  resources: { cpu_pct: number; memory_mb: number; disk_mb: number };
}
export interface DagStatus {
  nodes: Array<{ id: string; label: string; status: string; color: string; duration_ms: number | null; exit_code: number | null; stderr_tail?: string; stdout_tail?: string }>;
  edges: Array<{ source: string; target: string }>;
  parallel_groups: Array<Array<string>>; critical_path: Array<string>;
  metrics: { total_nodes: number; completed_nodes: number; failed_nodes: number; running_nodes: number; pending_nodes: number; skipped_nodes: number; cancelled_nodes: number; eta_ms: number | null };
}
export interface Diagnostics {
  failed_nodes: Array<{ rule: string; error_pattern: string | null; likely_cause: string; suggestions: string[]; relevant_log_lines: string[]; auto_fixable?: boolean; fix_action?: { description: string; command?: string } }>;
  warnings: Array<{ rule: string; pattern: string; suggestion: string }>;
  resource_bottlenecks: Array<{ rule: string; metric: string; actual: number; limit: number }>;
}
export interface RetryPlan { new_run_id: string; will_rerun: string[]; will_skip: string[]; }

// ── AI ──
export interface AiConfig { provider: string; model: string | null; api_url: string | null; is_configured: boolean; }
export interface AiExplainResponse { summary: string; root_cause: { rule: string; error_type: string; evidence: string; confidence: number } | null; fix_suggestion: { action: string; automated: boolean; estimated_impact: string } | null; }

// ── Templates ──
export interface Template { id: string; name: string; category: string; description: string; tags: string[]; toml_content: string; is_system: boolean; created_by: string | null; usage_count: number; created_at: string; updated_at: string; }

// ── Collaboration ──
export interface ForkResponse { forked_id: string; name: string; }
export interface ShareResponse { share_url: string; access_token: string; expires_at: string | null; }
export interface ImportResponse { pipeline_id: string; }

// ── Data ──
// GET /api/data/reference/status — a genome is `installed` only when at
// least one expected component exists on disk; `components` lists what was
// found, `missing_components` what is absent. Guidance lines (no fetchable
// URLs — oxo-flow hosts no reference mirror) land in `download_commands`.
export interface ReferenceStatusEntry { genome: string; components: string[]; found: string[]; missing_components: string[]; }
export interface ReferenceStatus { installed: ReferenceStatusEntry[]; missing_genomes: string[]; missing: string[]; download_commands: string[]; }

// ── Audit ──
export interface AuditLogResponse {
  entries: Array<{ timestamp: string; user: string; action: string; resource: string; result?: string }>;
  days: number;
  page: number;
  per_page: number;
  total: number;
}

// ── Resource Quota ──
export interface QuotaConfig {
  max_concurrent_runs: number;
  max_total_threads: number;
  max_total_memory_mb: number;
  max_runs_per_day: number;
}

export interface QuotaUsage {
  active_runs: number;
  used_threads: number;
  used_memory_mb: number;
  runs_today: number;
}

export interface QuotaInfo {
  enabled: boolean;
  limits: QuotaConfig;
  usage: QuotaUsage;
}

// ── Cluster connections (SSH endpoints) ──
export interface ClusterInfo {
  id: string;
  name: string;
  ssh_host: string;
  ssh_port: number;
  ssh_user?: string | null;
  /** The key itself is never returned (#517) — only whether one is set. */
  ssh_key_set?: boolean;
  scheduler?: string | null;
  remote_dir?: string | null;
  enabled: boolean;
  created_at: string;
}
export interface ClusterUpsert {
  id: string;
  name: string;
  ssh_host: string;
  ssh_port?: number;
  ssh_user?: string;
  ssh_key?: string;
  scheduler?: string;
  remote_dir?: string;
  enabled?: boolean;
}
// ── Dry-run preview (instance-level plan) ──
export interface DryRunPreview {
  checkpoint_preview: {
    summary: { will_run: number; will_skip: number; protected_outside: number };
    plan: Array<{ name: string; status: string; cascaded_from?: string | null }>;
    cascade_chains: string[][];
  };
  execution_order: string[];
}
export interface ClusterProbeResult {
  ok: boolean;
  hostname?: string | null;
  scheduler?: string | null;
  version?: string | null;
  error?: string | null;
  duration_ms: number;
}

// ── Legacy compat ──
export interface RunResponse {
  run_id: string;
  status: string;
  estimated_resources: {
    max_memory_mb: number;
    max_threads: number;
    estimated_duration_secs: number;
  };
  execution_plan: {
    total_rules: number;
    parallel_groups: string[][];
    execution_order: string[];
  };
}

// ── v0.8 AI Companion Types ──

// DAG Edit
export interface DagEditResponse {
  validation_errors?: string[];
  success: boolean;
  toml_content: string;
  dag_json: DagJson;
  validation: Array<{ code: string; message: string; severity: string; rule: string | null }>;
}

// Monitor
export interface MonitorAlert {
  /** Stable identity across polls (e.g. `failed:rule:pattern`, `resource:memory`) —
   *  dedup key so the 5s ai-status poll does not re-emit the same alert. */
  alert_id: string;
  level: 'info' | 'warn' | 'alert' | 'critical';
  rule_name: string | null;
  prediction: string;
  suggestion: string;
  auto_fixable: boolean;
  needs_approval: boolean;
  timestamp: string;
}

export interface MonitorStatus {
  overall: string;
  alerts: MonitorAlert[];
  resource_forecast: {
    cpu_trend: string;
    memory_trend: string;
    disk_trend: string;
    oom_risk: number;
    timeout_risk: number;
  };
  estimated_completion: string | null;
}

// Report
export interface ReportFile {
  path: string;
  name: string;
  size_bytes: number;
  is_dir: boolean;
}

export interface ReportFinding {
  finding: string;
  significance: string;
  evidence: string;
}

export interface ChartConfig {
  chart_type: string;
  title: string;
  spec: Record<string, unknown>;
}

export interface ReportData {
  qc_summary: Record<string, unknown>;
  key_findings: ReportFinding[];
  narrative_md: string;
  caveats: string[];
  suggested_next: string[];
  file_tree: ReportFile[];
  charts: ChartConfig[];
}

// AI Config (three-tier)
export interface UserAiConfig {
  user_config: {
    provider: string;
    api_url: string;
    model: string;
    api_key_set?: boolean;
    is_configured: boolean;
    /** #545: the four advanced fields PUT persists must round-trip. */
    search_enabled?: boolean;
    monitor_enabled?: boolean;
    auto_retry_enabled?: boolean;
    max_correction_rounds?: number;
  } | null;
  configured: boolean;
}

export interface AiConfigUpdate {
  provider?: string;
  api_key?: string;
  api_url?: string;
  model?: string;
  search_enabled?: boolean;
  monitor_enabled?: boolean;
  auto_retry_enabled?: boolean;
  max_correction_rounds?: number;
}

// Lifecycle & Verification extensions
export interface ProtectedDriftResponse {
  drifted_files: string[];
  new_trees: number;
  warnings: string[];
}

export interface ProvenanceVerifyEntry {
  file: string;
  status: 'matched' | 'mismatched' | 'missing' | 'cleaned' | string;
  expected?: string;
  actual?: string;
  note?: string;
}

export interface ProvenanceVerifyResponse {
  checkpoint: string;
  matched: number;
  mismatched: number;
  missing: number;
  cleaned: number;
  entries: ProvenanceVerifyEntry[];
  note?: string;
  ok: boolean;
}

export interface VerifyDataEntry {
  file: string;
  status: 'matched' | 'mismatched' | 'missing' | 'extra' | string;
  expected?: string;
  actual?: string;
  note?: string;
}

export interface VerifyDataResponse {
  tree: string;
  matched: number;
  mismatched: number;
  missing: number;
  extra: number;
  entries: VerifyDataEntry[];
  note?: string;
}

export interface DiffEntry {
  path: string;
  category: string;
  description: string;
  severity: string;
}

export interface DiffResponse {
  diffs: DiffEntry[];
}

export interface UploadedFileEntry {
  name: string;
  path: string;
  size_bytes: number;
}

export interface FilePreviewResponse {
  name: string;
  size_bytes: number;
  mime: string;
  truncated: boolean;
  content: string;
}

export interface SamplesheetParseResponse {
  format: string;
  samples_count: number;
  samples: Array<Record<string, string>>;
  detected_headers: string[];
}
