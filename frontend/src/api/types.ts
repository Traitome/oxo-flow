export interface WorkflowDetail {
  name: string;
  version: string;
  description?: string;
  author?: string;
  rules_count: number;
  rules: RuleSummary[];
}

export interface RuleSummary {
  name: string;
  inputs: string[];
  outputs: string[];
  environment: string;
  threads: number;
}

export interface DagData {
  dot: string;
  nodes: number;
  edges: number;
}

export interface DagJsonNode {
  name: string;
  inputs: string[];
  outputs: string[];
  environment: string;
}

export interface DagJsonEdge {
  from: string;
  to: string;
}

export interface DagJson {
  nodes: DagJsonNode[];
  edges: DagJsonEdge[];
}

export interface ValidateResponse {
  valid: boolean;
  errors: string[];
  rules_count: number | null;
  edges_count: number | null;
}

export interface RunStatus {
  id: string;
  status: string;
  rules_total: number;
  rules_completed: number;
  started_at?: string;
}

export interface RunResponse {
  run_id: string;
  status: string;
  execution_order: string[];
  rules_total: number;
}

export interface RunDetail {
  id: string;
  user_id: string;
  workflow_name: string;
  status: string;
  started_at?: string;
  finished_at?: string;
  log_tail?: string;
  output_files: string[];
}

export interface LoginResponse {
  token: string;
  username: string;
  role: string;
}

export interface TemplateSummary {
  id: string;
  name: string;
  category: string;
  description: string;
  tags: string;
  is_system: boolean;
  created_at: string;
}

export interface SseEvent {
  type: string;
  time: string;
  data: Record<string, unknown>;
}

export interface GenerateResponse {
  toml_content: string;
  workflow_name: string;
  rules_count: number;
  execution_order: string[];
  description: string;
  valid: boolean;
}

export interface HealthResponse {
  status: string;
  version: string;
}

export interface SystemInfo {
  version: string;
  os: string;
  arch: string;
  pid: number;
  uptime_secs: number;
}
