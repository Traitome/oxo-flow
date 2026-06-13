// ── Health & System ──
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
export interface ValidateResponse { valid: boolean; errors: Array<{ code: string; message: string; rule: string | null; suggestion: string | null }>; }
export interface ParseResponse {
  pipeline_id: string; name: string; version: string;
  rules: Array<{ name: string; inputs: string[]; outputs: string[]; environment: string; threads: number }>;
  dag: DagJson; stats: Record<string, unknown>;
}
export interface DagJson {
  nodes: Array<{ id: string; label: string; color: string }>;
  edges: Array<{ source: string; target: string }>;
  parallel_groups?: Array<Array<string>>; critical_path?: Array<string>;
}

// ── Runs ──
export interface RunItem { id: string; user_id: string; pipeline_id: string; status: string; phase: string; pid: number | null; workdir: string | null; started_at: string | null; finished_at: string | null; created_at: string; }
export interface RunStatus {
  status: string; phase: string;
  nodes: Array<{ rule: string; status: string; started_at: string | null; duration_ms: number | null; exit_code: number | null }>;
  resources: { cpu_pct: number; memory_mb: number; disk_mb: number };
}
export interface DagStatus {
  nodes: Array<{ id: string; label: string; status: string; color: string; duration_ms: number | null; exit_code: number | null }>;
  edges: Array<{ source: string; target: string }>;
  parallel_groups: Array<Array<string>>; critical_path: Array<string>;
  metrics: { total_nodes: number; completed_nodes: number; failed_nodes: number; running_nodes: number; pending_nodes: number; eta_ms: number | null };
}
export interface Diagnostics {
  failed_nodes: Array<{ rule: string; error_pattern: string | null; likely_cause: string; suggestions: string[]; relevant_log_lines: string[] }>;
  warnings: Array<{ rule: string; pattern: string; suggestion: string }>;
  resource_bottlenecks: Array<{ rule: string; metric: string; actual: number; limit: number }>;
}
export interface RetryPlan { new_run_id: string; will_rerun: string[]; will_skip: string[]; }

// ── AI ──
export interface AiConfig { provider: string; model: string | null; api_url: string | null; is_configured: boolean; }
export interface AiTranslateResponse { pipeline_id: string; toml_content: string; explanation: { steps: Array<{ rule: string; purpose: string; tool: string; key_params: string; why_chosen: string }> }; alternatives: Array<{ description: string; diff_summary: string; tradeoffs: string }>; confidence: number; }
export interface AiExplainResponse { summary: string; root_cause: { rule: string; error_type: string; evidence: string; confidence: number } | null; fix_suggestion: { action: string; automated: boolean; estimated_impact: string } | null; }
export interface AiInterpretResponse { narrative: string; highlights: Array<{ finding: string; significance: string; supporting_evidence: string }>; caveats: string[]; suggested_next: string[]; }
export interface AiOptimizeResponse { optimized_toml: string; changes: Array<{ rule: string; before: string; after: string; rationale: string; expected_impact: string }>; estimated: { time_saved: string; memory_reduction: string }; }

// ── Templates ──
export interface Template { id: string; name: string; category: string; description: string; tags: string[]; toml_content: string; is_system: boolean; created_by: string | null; usage_count: number; created_at: string; updated_at: string; }

// ── Collaboration ──
export interface ForkResponse { forked_id: string; name: string; }
export interface ShareResponse { share_url: string; access_token: string; expires_at: string | null; }
export interface ImportResponse { pipeline_id: string; }

// ── Data ──
export interface DataAnalysis {
  files: Array<{ path: string; size: number; format: string; format_confidence: number; paired_with: string | null; sample_name: string | null }>;
  summary: { total_size: number; formats_detected: string[]; paired_end_detected: boolean; strand_specific: boolean | null };
  suggested_workflow: { template: string; confidence: number; reason: string } | null;
}
export interface ReferenceResult { found: string[]; missing: string[]; download_commands: string[]; }

// ── Audit ──
export interface AuditLogResponse { entries: Array<{ timestamp: string; user: string; action: string; resource: string }>; days: number; }

// ── Search ──
export interface SearchResponse { query: string; total: number; results: Array<{ id: string; name: string; source: string; category: string; description: string; rules_count: number }>; }

// ── Legacy compat ──
export interface GenerateResponse { toml_content: string; workflow_name: string; rules_count: number; execution_order: string[]; description: string; valid: boolean; }
export interface WorkflowDetail extends ParseResponse {}
export interface RunResponse { run_id: string; status: string; execution_order: string[]; rules_total: number; }
export interface RunDetail extends RunItem { workflow_name?: string; log_tail?: string; output_files?: string[]; }
export interface TemplateSummary { id: string; name: string; category: string; description: string; tags: string; is_system: boolean; created_at: string; }
export interface DagData { dot: string; nodes: number; edges: number; }

// ── v0.9 AI Companion Types ──

// Chat Events (SSE)
export interface ChatEventV2 {
  type: 'text' | 'agent' | 'action' | 'error' | 'done';
  chunk?: string;
  agent?: string;
  status?: string;
  progress?: number;
  action_type?: string;
  data?: any;
  code?: string;
  message?: string;
  session_id?: string;
  pipeline_id?: string;
}

export interface ChatRequestV2 {
  session_id?: string;
  message: string;
  context?: {
    data_paths?: string[];
    samplesheet?: string;
    intent?: string;
  };
}

// Data Perception
export interface DataFindings {
  field: string;
  value: any;
  confidence: number;
  source: string;
  evidence: string;
}

export interface DataPerceptionReport {
  data_level: number;
  findings: DataFindings[];
  warnings: string[];
  suggestions: string[];
}

// DAG Edit
export interface DagEditCommand {
  source: 'dag_editor' | 'chat' | 'proposal';
  operation: 'add_rule' | 'remove_rule' | 'connect' | 'disconnect' | 'update_params' | 'replace_tool' | 'reorder';
  payload: any;
}

export interface DagEditResponse {
  success: boolean;
  toml_content: string;
  dag_json: DagJson;
  validation: Array<{ code: string; message: string; severity: string; rule: string | null }>;
}

// Monitor
export interface MonitorAlert {
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

export interface PauseRequest {
  reason: string;
}

export interface ResumeRequest {
  from_rule?: string;
  memory_adjust?: string;
  thread_adjust?: number;
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
  spec: any;
}

export interface ReportData {
  qc_summary: any;
  key_findings: ReportFinding[];
  narrative_md: string;
  caveats: string[];
  suggested_next: string[];
  file_tree: ReportFile[];
  charts: ChartConfig[];
}

// AI Config (three-tier)
export interface AiConfigFull {
  effective: {
    provider: string;
    model: string | null;
    api_url: string | null;
    is_configured: boolean;
  };
  tiers: {
    env_provider: string | null;
    env_model: string | null;
    env_url: string | null;
    server_provider: string | null;
    server_model: string | null;
    user_provider: string | null;
  };
  resolution_order: string[];
}

export interface ServerAiConfig {
  server_config: {
    provider: string;
    api_url: string;
    model: string;
    search_enabled: boolean;
  } | null;
  configured: boolean;
}

export interface UserAiConfig {
  user_config: {
    provider: string;
    api_url: string;
    model: string;
    is_configured: boolean;
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
