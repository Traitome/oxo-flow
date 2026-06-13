import type {
  HealthResponse, SystemInfo, RuntimeMetrics, LoginResponse, UserInfo,
  ValidateResponse, ParseResponse, Pipeline, DagJson, DagStatus,
  RunItem, RunStatus, Diagnostics, RetryPlan,
  AiConfig, AiTranslateResponse, AiExplainResponse, AiInterpretResponse, AiOptimizeResponse,
  Template, ForkResponse, ShareResponse, ImportResponse,
  DataAnalysis, ReferenceResult, AuditLogResponse, SearchResponse,
  GenerateResponse, WorkflowDetail, RunResponse,
} from './types';
import type { DagEditCommand, DagEditResponse, DataPerceptionReport, MonitorStatus, ReportData, AiConfigFull, ServerAiConfig, UserAiConfig, AiConfigUpdate } from './types';


class ApiError extends Error {
  code: string; detail?: string; suggestion?: string;
  constructor(code: string, message: string, detail?: string, suggestion?: string) {
    super(message); this.code = code; this.detail = detail; this.suggestion = suggestion; this.name = 'ApiError';
  }
}

async function request<T>(url: string, options?: RequestInit): Promise<T> {
  const token = localStorage.getItem('oxo_token');
  const headers: Record<string, string> = { 'Content-Type': 'application/json', ...(options?.headers as Record<string, string> || {}) };
  if (token) headers['Authorization'] = `Bearer ${token}`;

  const res = await fetch(url, { ...options, headers });
  if (!res.ok) {
    const body = await res.json().catch(() => ({}));
    throw new ApiError(body.code || 'UNKNOWN', body.message || res.statusText, body.detail, body.suggestion);
  }
  return res.json();
}

function get<T>(url: string) { return request<T>(url); }
function post<T>(url: string, body: unknown) { return request<T>(url, { method: 'POST', body: JSON.stringify(body) }); }
function put<T>(url: string, body: unknown) { return request<T>(url, { method: 'PUT', body: JSON.stringify(body) }); }
function del<T>(url: string) { return request<T>(url, { method: 'DELETE' }); }

export const api = {
  // Observability
  health: () => get<HealthResponse>('/api/health'),
  system: () => get<SystemInfo>('/api/system'),
  metrics: () => get<RuntimeMetrics>('/api/metrics'),
  audit: (days = 7) => get<AuditLogResponse>(`/api/audit?days=${days}`),
  events: () => get<{ events: Array<Record<string, unknown>> }>('/api/events'),

  // Auth
  login: (username: string, password: string) => post<LoginResponse>('/api/auth/login', { username, password }),
  authMe: () => get<{ authenticated: boolean; username?: string; role?: string }>('/api/auth/me'),
  listUsers: () => get<UserInfo[]>('/api/users'),
  createUser: (username: string, role: string, password?: string) => post<UserInfo>('/api/users', { username, role, password }),
  deleteUser: (id: string) => del<{ deleted: string }>(`/api/users/${id}`),
  licenseStatus: () => get<{ valid: boolean; license_type: string | null; commercial_use: string }>('/api/license'),
  uploadLicense: (licenseData: string) => post<{ valid: boolean }>('/api/license/upload', { license_data: licenseData }),

  // Pipeline lifecycle
  parse: (toml_content: string) => post<ParseResponse>('/api/pipelines/parse', { toml_content }),
  validate: (toml_content: string) => post<ValidateResponse>('/api/pipelines/validate', { toml_content }),
  prepare: (toml_content: string, resolve = true, apply = true) => post<{ pipeline_id: string; expanded_rules_count: number; wildcard_combinations: number }>('/api/pipelines/prepare', { toml_content, resolve_wildcards: resolve, apply_defaults: apply }),
  buildDag: (toml_content: string) => post<DagJson>('/api/pipelines/dag', { toml_content }),
  format: (toml_content: string) => post<{ formatted: string }>('/api/pipelines/format', { toml_content }),
  lint: (toml_content: string) => post<ValidateResponse>('/api/pipelines/lint', { toml_content }),
  stats: (toml_content: string) => post<Record<string, unknown>>('/api/pipelines/stats', { toml_content }),
  diff: (pipeline_a_id: string, pipeline_b_id: string) => post<{ diffs: Array<Record<string, unknown>> }>('/api/pipelines/diff', { pipeline_a_id, pipeline_b_id }),
  exportPipeline: (pipeline_id: string, format?: string) => post<{ content: string }>('/api/pipelines/export', { pipeline_id, format }),
  search: (query: string) => post<SearchResponse>('/api/pipelines/search', { query }),

  // Pipeline CRUD
  listPipelines: () => get<Pipeline[]>('/api/pipelines'),
  getPipeline: (id: string) => get<Pipeline>(`/api/pipelines/${id}`),
  updatePipeline: (id: string, data: Record<string, unknown>) => put<Pipeline>(`/api/pipelines/${id}`, data),
  deletePipeline: (id: string) => del<{ deleted: string }>(`/api/pipelines/${id}`),

  // Execution
  createRun: (toml_content: string, maxJobs = 4) => post<RunResponse>('/api/runs', { toml_content, max_jobs: maxJobs }),
  listRuns: () => get<RunItem[]>('/api/runs'),
  getRun: (id: string) => get<RunItem>(`/api/runs/${id}`),
  getRunStatus: (id: string) => get<RunStatus>(`/api/runs/${id}/status`),
  getDagStatus: (id: string) => get<DagStatus>(`/api/runs/${id}/dag-status`),
  getDiagnostics: (id: string) => get<Diagnostics>(`/api/runs/${id}/diagnostics`),
  getRunLogs: (id: string) => get<string>(`/api/runs/${id}/logs`),
  getRunResults: (id: string) => get<Array<{ name: string; path: string; size_bytes: number; is_dir: boolean }>>(`/api/runs/${id}/results`),
  retryRun: (id: string, skipSucceeded = true) => post<RetryPlan>(`/api/runs/${id}/retry`, { skip_succeeded: skipSucceeded }),
  cancelRun: (id: string) => post<{ run_id: string; status: string }>(`/api/runs/${id}/cancel`, {}),

  // Data discovery
  analyzeData: (paths: string[], maxDepth = 2) => post<DataAnalysis>('/api/data/analyze', { paths, max_depth: maxDepth }),
  discoverReference: (genome: string, components: string[]) => post<ReferenceResult>('/api/data/reference', { genome, components }),

  // Templates
  listTemplates: () => get<Template[]>('/api/templates'),
  getTemplate: (id: string) => get<Template>(`/api/templates/${id}`),
  saveTemplate: (data: { name: string; category: string; description: string; tags: string[]; toml_content: string }) => post<Template>('/api/templates', data),
  deleteTemplate: (id: string) => del<{ deleted: string }>(`/api/templates/${id}`),

  // AI
  aiConfig: () => get<AiConfig>('/api/ai/config'),
  aiUpdateConfig: (provider: string, apiKey?: string, apiUrl?: string, model?: string) => post<AiConfig>('/api/ai/config', { provider, api_key: apiKey, api_url: apiUrl, model }),
  aiTest: () => post<{ success: boolean; message: string; provider: string }>('/api/ai/test', {}),
  aiTranslate: (intent: string) => post<AiTranslateResponse>('/api/ai/translate', { intent }),
  aiExplain: (run_id: string, language = 'en') => post<AiExplainResponse>('/api/ai/explain', { run_id, language }),
  aiInterpret: (run_id: string, result_type = 'general') => post<AiInterpretResponse>('/api/ai/interpret', { run_id, result_type }),
  aiOptimize: (pipeline_id: string, goal: string) => post<AiOptimizeResponse>('/api/ai/optimize', { pipeline_id, goal }),

  // Collaboration
  forkPipeline: (id: string, userId = 'default') => post<ForkResponse>(`/api/pipelines/${id}/fork`, { user_id: userId }),
  sharePipeline: (id: string, visibility: string, expiresInDays?: number) => post<ShareResponse>(`/api/pipelines/${id}/share`, { visibility, expires_in_days: expiresInDays }),
  importPipeline: (url: string) => post<ImportResponse>('/api/pipelines/import', { url }),

  // Legacy compat (old endpoints still work)
  legacyGenerate: (intent: string) => post<GenerateResponse>('/api/workflows/generate', { intent }),
  legacyValidate: (toml_content: string) => post<ValidateResponse>('/api/workflows/validate', { toml_content }),
  legacyParse: (toml_content: string) => post<WorkflowDetail>('/api/workflows/parse', { toml_content }),
  legacyRun: (toml_content: string) => post<RunResponse>('/api/workflows/run', { toml_content }),
  legacyDryRun: (toml_content: string) => post<{ status: { id: string; status: string; rules_total: number; rules_completed: number }; execution_order: string[] }>('/api/workflows/dry-run', { toml_content }),
  legacyListEnvs: () => get<{ available: string[] }>('/api/environments'),
  legacyOpenApi: () => get<Record<string, unknown>>('/api/openapi.json'),
};

export function createEventSource(): EventSource {
  return new EventSource('/api/events');
}
export { ApiError };

// ── v0.9 AI Companion API endpoints ──

export const apiV2 = {
  // Chat
  chatSessions: () => get<Array<{ id: string; title: string; updated_at: string }>>('/api/chat/sessions'),
  chatSendJson: (message: string, context?: any) => post<any>('/api/chat/send/json', { message, context }),

  // DAG Edit
  dagCommand: (id: string, cmd: DagEditCommand) => post<DagEditResponse>(`/api/pipeline/${id}/command`, { source: cmd.source, operation: cmd.operation, payload: cmd.payload }),
  dagUndo: (id: string) => post<{ toml_content: string }>(`/api/pipeline/${id}/undo`, {}),
  dagRedo: (id: string) => post<{ toml_content: string }>(`/api/pipeline/${id}/redo`, {}),

  // Data Perception
  perceiveData: (paths?: string[], description?: string) => post<DataPerceptionReport>('/api/data/perceive', { paths, description }),
  referenceStatus: () => get<{ installed: any[]; missing: string[]; download_commands: string[] }>('/api/data/reference/status'),
  parseSamplesheet: (content: string) => post<{ format: string; samples_count: number; samples: any[] }>('/api/data/samplesheet/parse', { content }),

  // Monitor
  pauseRun: (id: string, reason?: string) => post<{ run_id: string; status: string }>(`/api/runs/${id}/pause`, { reason: reason || 'user_request' }),
  resumeRun: (id: string, from_rule?: string) => post<{ run_id: string; status: string }>(`/api/runs/${id}/resume`, { from_rule }),
  aiStatus: (id: string) => get<MonitorStatus>(`/api/runs/${id}/ai-status`),

  // Report
  runReport: (id: string) => get<ReportData>(`/api/runs/${id}/report`),
  askReport: (id: string, question: string) => post<string>(`/api/runs/${id}/report/ask`, { question }),
  visualizeReport: (id: string, type: string) => post<any>(`/api/runs/${id}/report/visualize`, { type }),

  // AI Config (three-tier)
  aiConfigEffective: () => get<AiConfigFull>('/api/ai/config/effective'),
  aiConfigServer: () => get<ServerAiConfig>('/api/ai/config/server'),
  aiConfigUser: () => get<UserAiConfig>('/api/ai/config/user'),
  updateAiConfigServer: (cfg: AiConfigUpdate) => put<{ status: string }>('/api/ai/config/server', cfg),
  updateAiConfigUser: (cfg: AiConfigUpdate) => put<{ status: string }>('/api/ai/config/user', cfg),
};

