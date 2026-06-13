import type {
  HealthResponse, SystemInfo, LoginResponse, ValidateResponse,
  RunResponse, RunDetail, GenerateResponse, WorkflowDetail,
  DagJson, TemplateSummary,
} from './types';

class ApiError extends Error {
  code: string;
  detail?: string;
  suggestion?: string;

  constructor(code: string, message: string, detail?: string, suggestion?: string) {
    super(message);
    this.code = code;
    this.detail = detail;
    this.suggestion = suggestion;
    this.name = 'ApiError';
  }
}

async function request<T>(url: string, options?: RequestInit): Promise<T> {
  const res = await fetch(url, {
    credentials: 'include',
    headers: { 'Content-Type': 'application/json', ...options?.headers },
    ...options,
  });
  if (!res.ok) {
    const body = await res.json().catch(() => ({}));
    throw new ApiError(
      body.code || 'UNKNOWN',
      body.message || res.statusText,
      body.detail,
      body.suggestion
    );
  }
  return res.json();
}

export const api = {
  health: () => request<HealthResponse>('/api/health'),
  system: () => request<SystemInfo>('/api/system'),

  login: (username: string, password: string) =>
    request<LoginResponse>('/api/auth/login', {
      method: 'POST',
      body: JSON.stringify({ username, password }),
    }),

  authMe: () => request<{ authenticated: boolean; username?: string; role?: string }>('/api/auth/me'),

  validate: (toml_content: string) =>
    request<ValidateResponse>('/api/workflows/validate', {
      method: 'POST',
      body: JSON.stringify({ toml_content }),
    }),

  parse: (toml_content: string) =>
    request<WorkflowDetail>('/api/workflows/parse', {
      method: 'POST',
      body: JSON.stringify({ toml_content }),
    }),

  buildDagJson: (toml_content: string) =>
    request<DagJson>('/api/workflows/dag-json', {
      method: 'POST',
      body: JSON.stringify({ toml_content }),
    }),

  dryRun: (toml_content: string) =>
    request<{ status: { id: string; status: string; rules_total: number; rules_completed: number; started_at?: string }; execution_order: string[] }>('/api/workflows/dry-run', {
      method: 'POST',
      body: JSON.stringify({ toml_content }),
    }),

  run: (toml_content: string) =>
    request<RunResponse>('/api/workflows/run', {
      method: 'POST',
      body: JSON.stringify({ toml_content }),
    }),

  generate: (intent: string, organism?: string, tools?: string) =>
    request<GenerateResponse>('/api/workflows/generate', {
      method: 'POST',
      body: JSON.stringify({ intent, organism, tools }),
    }),

  listRuns: () => request<RunDetail[]>('/api/runs'),

  getRun: (id: string) => request<RunDetail>(`/api/runs/${id}`),

  listTemplates: () => request<TemplateSummary[]>('/api/templates'),

  listEnvironments: () => request<{ available: string[] }>('/api/environments'),

  openApiSchema: () => request<Record<string, unknown>>('/api/openapi.json'),
};

export function createEventSource(): EventSource {
  return new EventSource('/api/events');
}
