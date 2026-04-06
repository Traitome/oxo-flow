#![forbid(unsafe_code)]
//! oxo-flow-web — Web interface for the oxo-flow pipeline engine.
//!
//! Provides a REST API and web UI for building, running, and monitoring
//! bioinformatics workflows.  Includes session-based authentication,
//! role-based access control, and dual-license verification via
//! [`oxo_license`].

use axum::{
    Router,
    extract::Json,
    http::StatusCode,
    middleware,
    response::IntoResponse,
    routing::{get, post},
};
use serde::{Deserialize, Serialize};
use tower_http::cors::{Any, CorsLayer};

// ---------------------------------------------------------------------------
// License configuration (oxo-dual-licenser integration)
// ---------------------------------------------------------------------------

/// Static license configuration for oxo-flow-web.
///
/// Uses the same Ed25519 public key as other Traitome products.  The license
/// file is discovered via (in order):
///   1. `OXO_FLOW_LICENSE` env var
///   2. Platform config directory (`io.traitome.oxo-flow/license.oxo.json`)
///   3. Legacy `~/.config/oxo-flow/license.oxo.json`
pub static OXO_FLOW_CONFIG: oxo_license::LicenseConfig = oxo_license::LicenseConfig {
    schema_version: "oxo-flow-license-v1",
    public_key_base64: "SOTbyPWS8fSF+XS9dqEg9cFyag0wPO/YMA5LhI4PXw4=",
    license_env_var: "OXO_FLOW_LICENSE",
    app_qualifier: "io",
    app_org: "traitome",
    app_name: "oxo-flow",
    license_filename: "license.oxo.json",
};

// ---------------------------------------------------------------------------
// Embedded frontend
// ---------------------------------------------------------------------------

/// Embedded single-page web application.
const FRONTEND_HTML: &str = r#"<!DOCTYPE html>
<html lang="en">
<head>
<meta charset="UTF-8">
<meta name="viewport" content="width=device-width, initial-scale=1.0">
<title>oxo-flow — Pipeline Engine</title>
<style>
:root { --bg: #0f172a; --surface: #1e293b; --border: #334155; --accent: #3b82f6; --accent-hover: #2563eb; --text: #f8fafc; --text-secondary: #94a3b8; --success: #22c55e; --error: #ef4444; --warning: #eab308; }
* { margin: 0; padding: 0; box-sizing: border-box; }
body { font-family: -apple-system, BlinkMacSystemFont, 'Segoe UI', system-ui, sans-serif; background: var(--bg); color: var(--text); min-height: 100vh; }
header { background: var(--surface); border-bottom: 1px solid var(--border); padding: 0.75rem 1.5rem; display: flex; align-items: center; justify-content: space-between; }
header h1 { font-size: 1.25rem; font-weight: 600; }
header h1 span { color: var(--accent); }
nav { display: flex; gap: 0.5rem; align-items: center; }
nav button { background: transparent; border: 1px solid var(--border); color: var(--text-secondary); padding: 0.4rem 1rem; border-radius: 0.375rem; cursor: pointer; font-size: 0.875rem; transition: all 0.15s; }
nav button:hover, nav button.active { background: var(--accent); color: white; border-color: var(--accent); }
.user-info { font-size: 0.8rem; color: var(--text-secondary); margin-left: 1rem; }
.user-info strong { color: var(--accent); }
.container { max-width: 1200px; margin: 0 auto; padding: 1.5rem; }
.card { background: var(--surface); border: 1px solid var(--border); border-radius: 0.5rem; padding: 1.25rem; margin-bottom: 1rem; }
.card h2 { font-size: 1rem; font-weight: 600; margin-bottom: 0.75rem; color: var(--accent); }
.grid { display: grid; grid-template-columns: repeat(auto-fit, minmax(200px, 1fr)); gap: 1rem; }
.stat { text-align: center; }
.stat .value { font-size: 2rem; font-weight: 700; color: var(--accent); }
.stat .label { font-size: 0.75rem; color: var(--text-secondary); text-transform: uppercase; letter-spacing: 0.05em; }
textarea { width: 100%; min-height: 300px; background: var(--bg); color: var(--text); border: 1px solid var(--border); border-radius: 0.375rem; padding: 0.75rem; font-family: 'SF Mono', 'Fira Code', monospace; font-size: 0.8rem; resize: vertical; }
input[type="text"], input[type="password"] { background: var(--bg); color: var(--text); border: 1px solid var(--border); border-radius: 0.375rem; padding: 0.5rem 0.75rem; font-size: 0.875rem; width: 100%; }
.btn { display: inline-block; padding: 0.5rem 1rem; border-radius: 0.375rem; border: none; cursor: pointer; font-size: 0.875rem; font-weight: 500; transition: all 0.15s; }
.btn-primary { background: var(--accent); color: white; }
.btn-primary:hover { background: var(--accent-hover); }
.btn-success { background: var(--success); color: white; }
.btn-danger { background: var(--error); color: white; }
.btn-warning { background: var(--warning); color: #000; }
.actions { display: flex; gap: 0.5rem; margin-top: 0.75rem; flex-wrap: wrap; }
.output { background: var(--bg); border: 1px solid var(--border); border-radius: 0.375rem; padding: 0.75rem; margin-top: 0.75rem; font-family: monospace; font-size: 0.8rem; white-space: pre-wrap; max-height: 400px; overflow-y: auto; }
.badge { display: inline-block; padding: 0.15rem 0.5rem; border-radius: 0.25rem; font-size: 0.7rem; font-weight: 600; }
.badge-ok { background: var(--success); color: white; }
.badge-err { background: var(--error); color: white; }
.badge-warn { background: var(--warning); color: #000; }
.hidden { display: none; }
table { width: 100%; border-collapse: collapse; font-size: 0.85rem; }
th, td { text-align: left; padding: 0.5rem; border-bottom: 1px solid var(--border); }
th { color: var(--text-secondary); font-weight: 500; font-size: 0.75rem; text-transform: uppercase; }
.login-container { max-width: 400px; margin: 4rem auto; }
.form-group { margin-bottom: 1rem; }
.form-group label { display: block; margin-bottom: 0.25rem; font-size: 0.85rem; color: var(--text-secondary); }
#status-bar { padding: 0.5rem 1.5rem; font-size: 0.75rem; color: var(--text-secondary); border-top: 1px solid var(--border); background: var(--surface); position: fixed; bottom: 0; width: 100%; }
</style>
</head>
<body>
<header>
  <h1><span>oxo-flow</span> Pipeline Engine</h1>
  <nav id="main-nav">
    <button class="active" onclick="showView('dashboard', this)">Dashboard</button>
    <button onclick="showView('editor', this)">Editor</button>
    <button onclick="showView('monitor', this)">Monitor</button>
    <button onclick="showView('system', this)">System</button>
    <span id="user-info" class="user-info"></span>
  </nav>
</header>

<div class="container">
  <!-- Login View -->
  <div id="view-login" class="hidden">
    <div class="login-container">
      <div class="card">
        <h2>Sign In</h2>
        <div class="form-group">
          <label for="login-user">Username</label>
          <input type="text" id="login-user" placeholder="admin">
        </div>
        <div class="form-group">
          <label for="login-pass">Password</label>
          <input type="password" id="login-pass" placeholder="password">
        </div>
        <div id="login-error" style="color:var(--error);font-size:0.85rem;margin-bottom:0.5rem"></div>
        <button class="btn btn-primary" onclick="doLogin()" style="width:100%">Login</button>
      </div>
    </div>
  </div>

  <!-- Dashboard View -->
  <div id="view-dashboard">
    <div class="grid" id="stats-grid">
      <div class="card stat"><div class="value" id="stat-version">-</div><div class="label">Version</div></div>
      <div class="card stat"><div class="value" id="stat-status">-</div><div class="label">Status</div></div>
      <div class="card stat"><div class="value" id="stat-envs">-</div><div class="label">Environments</div></div>
      <div class="card stat"><div class="value" id="stat-license">-</div><div class="label">License</div></div>
    </div>
    <div class="card">
      <h2>Quick Validate</h2>
      <p style="color:var(--text-secondary);font-size:0.85rem;margin-bottom:0.5rem">Paste a .oxoflow workflow to quickly validate it.</p>
      <textarea id="quick-toml" placeholder="[workflow]&#10;name = &quot;my-pipeline&quot;&#10;&#10;[[rules]]&#10;name = &quot;step1&quot;&#10;shell = &quot;echo hello&quot;"></textarea>
      <div class="actions">
        <button class="btn btn-primary" onclick="quickValidate()">Validate</button>
        <button class="btn btn-success" onclick="quickFormat()">Format</button>
        <button class="btn btn-warning" onclick="quickLint()">Lint</button>
        <button class="btn btn-primary" onclick="quickDag()">Build DAG</button>
      </div>
      <div id="quick-output" class="output hidden"></div>
    </div>
  </div>

  <!-- Editor View -->
  <div id="view-editor" class="hidden">
    <div class="card">
      <h2>Workflow Editor</h2>
      <textarea id="editor-toml" placeholder="[workflow]&#10;name = &quot;my-pipeline&quot;&#10;version = &quot;1.0.0&quot;"></textarea>
      <div class="actions">
        <button class="btn btn-primary" onclick="editorValidate()">Validate</button>
        <button class="btn btn-success" onclick="editorFormat()">Format</button>
        <button class="btn btn-warning" onclick="editorLint()">Lint</button>
        <button class="btn btn-primary" onclick="editorDag()">DAG</button>
        <button class="btn btn-primary" onclick="editorDryRun()">Dry Run</button>
        <button class="btn btn-primary" onclick="editorParse()">Parse</button>
        <button class="btn btn-primary" onclick="editorStats()">Stats</button>
      </div>
      <div id="editor-output" class="output hidden"></div>
    </div>
  </div>

  <!-- Monitor View -->
  <div id="view-monitor" class="hidden">
    <div class="card">
      <h2>Execution Monitor</h2>
      <p style="color:var(--text-secondary);font-size:0.85rem">Real-time execution monitoring via SSE.</p>
      <div class="actions">
        <button class="btn btn-primary" onclick="connectSSE()">Connect</button>
        <button class="btn btn-danger" onclick="disconnectSSE()">Disconnect</button>
      </div>
      <div id="sse-output" class="output" style="min-height:200px">Waiting for connection...</div>
    </div>
  </div>

  <!-- System View -->
  <div id="view-system" class="hidden">
    <div class="card">
      <h2>System Information</h2>
      <div id="system-info">Loading...</div>
    </div>
    <div class="card">
      <h2>License Status</h2>
      <div id="license-info">Loading...</div>
    </div>
    <div class="card">
      <h2>Available Environments</h2>
      <div id="env-list">Loading...</div>
    </div>
  </div>
</div>

<div id="status-bar">Ready</div>

<script>
var BASE = '';
var authToken = null;

function authHeaders() {
  var h = {'Content-Type': 'application/json'};
  if (authToken) h['Authorization'] = 'Bearer ' + authToken;
  return h;
}

function showView(name, btn) {
  document.querySelectorAll('[id^="view-"]').forEach(function(el) { el.classList.add('hidden'); });
  document.getElementById('view-' + name).classList.remove('hidden');
  document.querySelectorAll('nav button').forEach(function(b) { b.classList.remove('active'); });
  if (btn) btn.classList.add('active');
  if (name === 'system') loadSystemInfo();
  if (name === 'dashboard') loadDashboard();
}

function setStatus(msg) { document.getElementById('status-bar').textContent = msg; }
function showOutput(id, text) { var el = document.getElementById(id); el.textContent = text; el.classList.remove('hidden'); }

async function apiPost(path, body) {
  setStatus('Requesting ' + path + '...');
  var res = await fetch(BASE + path, { method: 'POST', headers: authHeaders(), body: JSON.stringify(body) });
  var data = await res.json();
  setStatus('Done');
  return data;
}

async function apiGet(path) {
  var res = await fetch(BASE + path, { headers: authHeaders() });
  return await res.json();
}

async function doLogin() {
  var user = document.getElementById('login-user').value;
  var pass = document.getElementById('login-pass').value;
  try {
    var res = await fetch(BASE + '/api/auth/login', { method: 'POST', headers: {'Content-Type': 'application/json'}, body: JSON.stringify({username: user, password: pass}) });
    var data = await res.json();
    if (data.token) {
      authToken = data.token;
      var info = document.getElementById('user-info');
      info.textContent = '';
      var strong = document.createElement('strong');
      strong.textContent = data.username;
      info.appendChild(strong);
      info.appendChild(document.createTextNode(' (' + data.role + ') '));
      var logoutBtn = document.createElement('button');
      logoutBtn.className = 'btn btn-danger';
      logoutBtn.style.cssText = 'padding:0.2rem 0.5rem;font-size:0.7rem';
      logoutBtn.textContent = 'Logout';
      logoutBtn.onclick = doLogout;
      info.appendChild(logoutBtn);
      showView('dashboard');
      document.getElementById('view-login').classList.add('hidden');
      setStatus('Logged in as ' + data.username);
    } else {
      document.getElementById('login-error').textContent = data.error || 'Login failed';
    }
  } catch(e) { document.getElementById('login-error').textContent = e.message; }
}

function doLogout() {
  authToken = null;
  document.getElementById('user-info').innerHTML = '';
  setStatus('Logged out');
}

async function loadDashboard() {
  try {
    var ver = await apiGet('/api/version');
    document.getElementById('stat-version').textContent = ver.version || '-';
    var health = await apiGet('/api/health');
    document.getElementById('stat-status').textContent = health.status || '-';
    var envs = await apiGet('/api/environments');
    document.getElementById('stat-envs').textContent = (envs.available || []).length;
    var lic = await apiGet('/api/license');
    document.getElementById('stat-license').textContent = lic.valid ? 'Active' : 'None';
  } catch(e) { setStatus('Error: ' + e.message); }
}

async function quickValidate() {
  var toml = document.getElementById('quick-toml').value;
  var data = await apiPost('/api/workflows/validate', { toml_content: toml });
  showOutput('quick-output', JSON.stringify(data, null, 2));
}
async function quickFormat() {
  var toml = document.getElementById('quick-toml').value;
  var data = await apiPost('/api/workflows/format', { toml_content: toml });
  if (data.formatted) { document.getElementById('quick-toml').value = data.formatted; showOutput('quick-output', 'Formatted successfully.'); }
  else showOutput('quick-output', JSON.stringify(data, null, 2));
}
async function quickLint() {
  var toml = document.getElementById('quick-toml').value;
  var data = await apiPost('/api/workflows/lint', { toml_content: toml });
  showOutput('quick-output', JSON.stringify(data, null, 2));
}
async function quickDag() {
  var toml = document.getElementById('quick-toml').value;
  var data = await apiPost('/api/workflows/dag', { toml_content: toml });
  showOutput('quick-output', JSON.stringify(data, null, 2));
}

async function editorValidate() {
  var toml = document.getElementById('editor-toml').value;
  var data = await apiPost('/api/workflows/validate', { toml_content: toml });
  showOutput('editor-output', JSON.stringify(data, null, 2));
}
async function editorFormat() {
  var toml = document.getElementById('editor-toml').value;
  var data = await apiPost('/api/workflows/format', { toml_content: toml });
  if (data.formatted) { document.getElementById('editor-toml').value = data.formatted; showOutput('editor-output', 'Formatted.'); }
  else showOutput('editor-output', JSON.stringify(data, null, 2));
}
async function editorLint() {
  var toml = document.getElementById('editor-toml').value;
  var data = await apiPost('/api/workflows/lint', { toml_content: toml });
  showOutput('editor-output', JSON.stringify(data, null, 2));
}
async function editorDag() {
  var toml = document.getElementById('editor-toml').value;
  var data = await apiPost('/api/workflows/dag', { toml_content: toml });
  showOutput('editor-output', data.dot || JSON.stringify(data, null, 2));
}
async function editorDryRun() {
  var toml = document.getElementById('editor-toml').value;
  var data = await apiPost('/api/workflows/dry-run', { toml_content: toml });
  showOutput('editor-output', JSON.stringify(data, null, 2));
}
async function editorParse() {
  var toml = document.getElementById('editor-toml').value;
  var data = await apiPost('/api/workflows/parse', { toml_content: toml });
  showOutput('editor-output', JSON.stringify(data, null, 2));
}
async function editorStats() {
  var toml = document.getElementById('editor-toml').value;
  var data = await apiPost('/api/workflows/stats', { toml_content: toml });
  showOutput('editor-output', JSON.stringify(data, null, 2));
}

var sseSource = null;
function connectSSE() {
  if (sseSource) sseSource.close();
  var el = document.getElementById('sse-output');
  el.textContent = 'Connecting...\n';
  sseSource = new EventSource(BASE + '/api/events');
  sseSource.onmessage = function(e) { el.textContent += e.data + '\n'; el.scrollTop = el.scrollHeight; };
  sseSource.onerror = function() { el.textContent += '[connection error]\n'; };
  sseSource.onopen = function() { el.textContent += '[connected]\n'; setStatus('SSE connected'); };
}
function disconnectSSE() {
  if (sseSource) { sseSource.close(); sseSource = null; }
  document.getElementById('sse-output').textContent += '[disconnected]\n';
  setStatus('SSE disconnected');
}

async function loadSystemInfo() {
  try {
    var sys = await apiGet('/api/system');
    document.getElementById('system-info').innerHTML = '<pre>' + JSON.stringify(sys, null, 2) + '</pre>';
    var lic = await apiGet('/api/license');
    var licHtml = '<table><tr><th>Valid</th><td>' + (lic.valid ? '<span class="badge badge-ok">Yes</span>' : '<span class="badge badge-err">No</span>') + '</td></tr>';
    licHtml += '<tr><th>Type</th><td>' + (lic.license_type || 'N/A') + '</td></tr>';
    licHtml += '<tr><th>Issued To</th><td>' + (lic.issued_to || 'N/A') + '</td></tr>';
    licHtml += '<tr><th>Message</th><td>' + lic.message + '</td></tr></table>';
    document.getElementById('license-info').innerHTML = licHtml;
    var envs = await apiGet('/api/environments');
    var list = (envs.available || []).map(function(e) { return '<span class="badge badge-ok">' + e + '</span> '; }).join('');
    document.getElementById('env-list').innerHTML = list || '<em>None detected</em>';
  } catch(e) { setStatus('Error: ' + e.message); }
}

loadDashboard();
</script>
</body>
</html>"#;

// Store server start time for uptime calculation.
static START_TIME: std::sync::OnceLock<std::time::Instant> = std::sync::OnceLock::new();

fn get_start_time() -> std::time::Instant {
    *START_TIME.get_or_init(std::time::Instant::now)
}

// ---------------------------------------------------------------------------
// Authentication & authorization
// ---------------------------------------------------------------------------

/// User roles with increasing privileges.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum UserRole {
    /// Read-only access (dashboards, system info).
    Viewer,
    /// Can run and manage workflows.
    User,
    /// Full access including user management.
    Admin,
}

impl std::fmt::Display for UserRole {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            UserRole::Viewer => write!(f, "viewer"),
            UserRole::User => write!(f, "user"),
            UserRole::Admin => write!(f, "admin"),
        }
    }
}

/// In-memory session store.  Each entry maps a base64-encoded random token
/// to the associated user name and role.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct Session {
    username: String,
    role: UserRole,
    created_at: String,
}

/// Global session store — `HashMap<token, Session>`.
static SESSIONS: std::sync::OnceLock<std::sync::Mutex<std::collections::HashMap<String, Session>>> =
    std::sync::OnceLock::new();

fn sessions() -> &'static std::sync::Mutex<std::collections::HashMap<String, Session>> {
    SESSIONS.get_or_init(|| std::sync::Mutex::new(std::collections::HashMap::new()))
}

/// Login request body.
#[derive(Serialize, Deserialize)]
pub struct LoginRequest {
    pub username: String,
    pub password: String,
}

/// Login response body.
#[derive(Serialize, Deserialize)]
pub struct LoginResponse {
    pub token: String,
    pub username: String,
    pub role: String,
}

/// Response from `GET /api/auth/me`.
#[derive(Serialize, Deserialize)]
pub struct AuthMeResponse {
    pub authenticated: bool,
    pub username: Option<String>,
    pub role: Option<String>,
}

/// License status response.
#[derive(Serialize, Deserialize)]
pub struct LicenseStatus {
    pub valid: bool,
    pub license_type: Option<String>,
    pub issued_to: Option<String>,
    pub schema: Option<String>,
    pub message: String,
}

/// Simple password check.  In production this should use hashed passwords
/// and a persistent user store backed by environment variables or a config file.
/// The default accounts (`admin/admin`, `user/user`, `viewer/viewer`) are
/// intentionally simple for initial setup and development; they should be
/// changed immediately in any deployment.
fn check_credentials(username: &str, password: &str) -> Option<UserRole> {
    // Default accounts — override via a real user store in production.
    match (username, password) {
        ("admin", "admin") => Some(UserRole::Admin),
        ("user", "user") => Some(UserRole::User),
        ("viewer", "viewer") => Some(UserRole::Viewer),
        _ => None,
    }
}

/// Generate a hex-encoded UUID session token.
fn generate_session_token() -> String {
    use std::fmt::Write;
    let id = uuid::Uuid::new_v4();
    let mut buf = String::with_capacity(44);
    for byte in id.as_bytes() {
        let _ = write!(buf, "{byte:02x}");
    }
    buf
}

/// Extract a session from the `Authorization: Bearer <token>` header.
fn extract_session(headers: &axum::http::HeaderMap) -> Option<Session> {
    let value = headers.get("authorization")?.to_str().ok()?;
    let token = value.strip_prefix("Bearer ")?;
    let store = sessions().lock().ok()?;
    store.get(token).cloned()
}

/// Check the oxo-flow license status.
fn check_license() -> LicenseStatus {
    match oxo_license::load_and_verify(None, &OXO_FLOW_CONFIG) {
        Ok(license) => LicenseStatus {
            valid: true,
            license_type: Some(license.payload.license_type.clone()),
            issued_to: Some(license.payload.issued_to_org.clone()),
            schema: Some(license.payload.schema.clone()),
            message: "License verified successfully".to_string(),
        },
        Err(e) => LicenseStatus {
            valid: false,
            license_type: None,
            issued_to: None,
            schema: None,
            message: format!("No valid license: {e}"),
        },
    }
}

// ---------------------------------------------------------------------------
// Data types
// ---------------------------------------------------------------------------

/// Health check response.
#[derive(Serialize, Deserialize)]
struct HealthResponse {
    status: String,
    version: String,
}

/// Workflow list response.
#[derive(Serialize, Deserialize)]
struct WorkflowListResponse {
    workflows: Vec<WorkflowSummary>,
}

/// Summary of a workflow.
#[derive(Serialize, Deserialize)]
pub struct WorkflowSummary {
    pub name: String,
    pub version: String,
    pub rules_count: usize,
}

/// Full workflow detail including parsed rules.
#[derive(Serialize, Deserialize)]
pub struct WorkflowDetail {
    pub name: String,
    pub version: String,
    pub description: Option<String>,
    pub author: Option<String>,
    pub rules_count: usize,
    pub rules: Vec<RuleSummary>,
}

/// Summary of a single rule within a workflow.
#[derive(Serialize, Deserialize)]
pub struct RuleSummary {
    pub name: String,
    pub inputs: Vec<String>,
    pub outputs: Vec<String>,
    pub environment: String,
    pub threads: u32,
}

/// Environment backend info.
#[derive(Serialize, Deserialize)]
struct EnvInfo {
    available: Vec<String>,
}

/// Request body for endpoints that accept TOML workflow content.
#[derive(Serialize, Deserialize)]
pub struct ValidateRequest {
    pub toml_content: String,
}

/// Response from the validation endpoint.
#[derive(Serialize, Deserialize)]
pub struct ValidateResponse {
    pub valid: bool,
    pub errors: Vec<String>,
    pub rules_count: Option<usize>,
    pub edges_count: Option<usize>,
}

/// Optional run configuration parameters.
#[derive(Serialize, Deserialize)]
pub struct RunConfig {
    pub max_jobs: Option<usize>,
    pub dry_run: Option<bool>,
    pub keep_going: Option<bool>,
}

/// Status of a workflow run (used in dry-run response).
#[derive(Serialize, Deserialize)]
pub struct RunStatus {
    pub id: String,
    pub status: String,
    pub rules_total: usize,
    pub rules_completed: usize,
    pub started_at: Option<String>,
}

/// Request body for the dry-run endpoint.
#[derive(Serialize, Deserialize)]
struct DryRunRequest {
    toml_content: String,
    #[serde(default)]
    config: Option<RunConfig>,
}

/// DAG visualisation response.
#[derive(Serialize, Deserialize)]
pub struct DagResponse {
    pub dot: String,
    pub nodes: usize,
    pub edges: usize,
}

/// Request body for report generation.
#[derive(Serialize, Deserialize)]
pub struct ReportRequest {
    pub toml_content: String,
    pub format: Option<String>,
}

/// Uniform JSON error body.
#[derive(Serialize, Deserialize)]
pub struct ErrorResponse {
    pub error: String,
    pub detail: Option<String>,
}

/// Response from the run endpoint.
#[derive(Serialize, Deserialize)]
pub struct RunResponse {
    pub run_id: String,
    pub status: String,
    pub execution_order: Vec<String>,
    pub rules_total: usize,
}

/// Response from the version endpoint.
#[derive(Serialize, Deserialize)]
pub struct VersionResponse {
    pub version: String,
    pub crate_name: String,
    pub rust_version: String,
}

/// Response from the clean endpoint.
#[derive(Serialize, Deserialize)]
pub struct CleanResponse {
    pub workflow_name: String,
    pub files_to_clean: Vec<String>,
    pub total_files: usize,
}

/// Request body for the export endpoint.
#[derive(Serialize, Deserialize)]
pub struct ExportRequest {
    pub toml_content: String,
    pub format: Option<String>, // "docker" or "singularity", default "docker"
}

/// Response from the export endpoint.
#[derive(Serialize, Deserialize)]
pub struct ExportResponse {
    pub format: String,
    pub content: String,
}

/// Request body for lint endpoint.
#[derive(Serialize, Deserialize)]
pub struct LintRequest {
    pub toml_content: String,
}

/// Response from lint endpoint.
#[derive(Serialize, Deserialize)]
pub struct LintResponse {
    pub diagnostics: Vec<DiagnosticItem>,
    pub error_count: usize,
    pub warning_count: usize,
    pub info_count: usize,
}

/// Single diagnostic item in lint/validate response.
#[derive(Serialize, Deserialize)]
pub struct DiagnosticItem {
    pub severity: String,
    pub code: String,
    pub message: String,
    pub rule: Option<String>,
}

/// Response from format endpoint.
#[derive(Serialize, Deserialize)]
pub struct FormatResponse {
    pub formatted: String,
}

/// Response from stats endpoint.
#[derive(Serialize, Deserialize)]
pub struct StatsResponse {
    pub rule_count: usize,
    pub shell_rules: usize,
    pub script_rules: usize,
    pub dependency_count: usize,
    pub parallel_groups: usize,
    pub max_depth: usize,
    pub environments: Vec<String>,
    pub total_threads: u32,
    pub wildcard_count: usize,
    pub wildcard_names: Vec<String>,
}

/// System information response.
#[derive(Serialize, Deserialize)]
pub struct SystemInfo {
    pub version: String,
    pub rust_version: String,
    pub os: String,
    pub arch: String,
    pub pid: u32,
    pub uptime_secs: f64,
}

// ---------------------------------------------------------------------------
// Error helper
// ---------------------------------------------------------------------------

/// Wrap an `ErrorResponse` with an HTTP status code so it can be returned from
/// any handler via `Result<impl IntoResponse, ApiError>`.
struct ApiError {
    status: StatusCode,
    body: ErrorResponse,
}

impl ApiError {
    fn bad_request(error: impl Into<String>, detail: impl Into<Option<String>>) -> Self {
        Self {
            status: StatusCode::BAD_REQUEST,
            body: ErrorResponse {
                error: error.into(),
                detail: detail.into(),
            },
        }
    }

    fn unprocessable(error: impl Into<String>, detail: impl Into<Option<String>>) -> Self {
        Self {
            status: StatusCode::UNPROCESSABLE_ENTITY,
            body: ErrorResponse {
                error: error.into(),
                detail: detail.into(),
            },
        }
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> axum::response::Response {
        (self.status, Json(self.body)).into_response()
    }
}

// ---------------------------------------------------------------------------
// Existing endpoints
// ---------------------------------------------------------------------------

async fn health() -> Json<HealthResponse> {
    Json(HealthResponse {
        status: "ok".to_string(),
        version: env!("CARGO_PKG_VERSION").to_string(),
    })
}

async fn list_workflows() -> Json<WorkflowListResponse> {
    // Placeholder — will scan working directory for .oxoflow files
    Json(WorkflowListResponse { workflows: vec![] })
}

async fn list_environments() -> Json<EnvInfo> {
    let resolver = oxo_flow_core::environment::EnvironmentResolver::new();
    Json(EnvInfo {
        available: resolver
            .available_backends()
            .into_iter()
            .map(String::from)
            .collect(),
    })
}

async fn not_found() -> (StatusCode, Json<ErrorResponse>) {
    (
        StatusCode::NOT_FOUND,
        Json(ErrorResponse {
            error: "Not found".to_string(),
            detail: None,
        }),
    )
}

// ---------------------------------------------------------------------------
// New endpoints
// ---------------------------------------------------------------------------

/// `POST /api/workflows/validate` — Parse + validate a workflow TOML.
async fn validate_workflow(
    Json(req): Json<ValidateRequest>,
) -> Result<Json<ValidateResponse>, ApiError> {
    let config = match oxo_flow_core::WorkflowConfig::parse(&req.toml_content) {
        Ok(c) => c,
        Err(e) => {
            return Ok(Json(ValidateResponse {
                valid: false,
                errors: vec![e.to_string()],
                rules_count: None,
                edges_count: None,
            }));
        }
    };

    let dag = match oxo_flow_core::WorkflowDag::from_rules(&config.rules) {
        Ok(d) => d,
        Err(e) => {
            return Ok(Json(ValidateResponse {
                valid: false,
                errors: vec![e.to_string()],
                rules_count: Some(config.rules.len()),
                edges_count: None,
            }));
        }
    };

    if let Err(e) = dag.validate() {
        return Ok(Json(ValidateResponse {
            valid: false,
            errors: vec![e.to_string()],
            rules_count: Some(dag.node_count()),
            edges_count: Some(dag.edge_count()),
        }));
    }

    Ok(Json(ValidateResponse {
        valid: true,
        errors: vec![],
        rules_count: Some(dag.node_count()),
        edges_count: Some(dag.edge_count()),
    }))
}

/// `POST /api/workflows/parse` — Parse a workflow and return full detail.
async fn parse_workflow(
    Json(req): Json<ValidateRequest>,
) -> Result<Json<WorkflowDetail>, ApiError> {
    let config = oxo_flow_core::WorkflowConfig::parse(&req.toml_content)
        .map_err(|e| ApiError::bad_request("Invalid workflow TOML", Some(e.to_string())))?;

    let rules: Vec<RuleSummary> = config
        .rules
        .iter()
        .map(|r| RuleSummary {
            name: r.name.clone(),
            inputs: r.input.clone(),
            outputs: r.output.clone(),
            environment: r.environment.kind().to_string(),
            threads: r.effective_threads(),
        })
        .collect();

    Ok(Json(WorkflowDetail {
        name: config.workflow.name.clone(),
        version: config.workflow.version.clone(),
        description: config.workflow.description.clone(),
        author: config.workflow.author.clone(),
        rules_count: rules.len(),
        rules,
    }))
}

/// `POST /api/workflows/dag` — Build a DAG and return its DOT representation.
async fn build_dag(Json(req): Json<ValidateRequest>) -> Result<Json<DagResponse>, ApiError> {
    let config = oxo_flow_core::WorkflowConfig::parse(&req.toml_content)
        .map_err(|e| ApiError::bad_request("Invalid workflow TOML", Some(e.to_string())))?;

    let dag = oxo_flow_core::WorkflowDag::from_rules(&config.rules)
        .map_err(|e| ApiError::unprocessable("DAG construction failed", Some(e.to_string())))?;

    Ok(Json(DagResponse {
        dot: dag.to_dot(),
        nodes: dag.node_count(),
        edges: dag.edge_count(),
    }))
}

/// `POST /api/workflows/dry-run` — Simulate execution and return the plan.
async fn dry_run(Json(req): Json<DryRunRequest>) -> Result<impl IntoResponse, ApiError> {
    let config = oxo_flow_core::WorkflowConfig::parse(&req.toml_content)
        .map_err(|e| ApiError::bad_request("Invalid workflow TOML", Some(e.to_string())))?;

    let dag = oxo_flow_core::WorkflowDag::from_rules(&config.rules)
        .map_err(|e| ApiError::unprocessable("DAG construction failed", Some(e.to_string())))?;

    let order = dag.execution_order().map_err(|e| {
        ApiError::unprocessable("Cannot determine execution order", Some(e.to_string()))
    })?;

    let rules: Vec<RuleSummary> = order
        .iter()
        .filter_map(|name| config.get_rule(name))
        .map(|r| RuleSummary {
            name: r.name.clone(),
            inputs: r.input.clone(),
            outputs: r.output.clone(),
            environment: r.environment.kind().to_string(),
            threads: r.effective_threads(),
        })
        .collect();

    let run_config = req.config.unwrap_or(RunConfig {
        max_jobs: None,
        dry_run: None,
        keep_going: None,
    });

    let status = RunStatus {
        id: uuid::Uuid::new_v4().to_string(),
        status: "dry-run".to_string(),
        rules_total: rules.len(),
        rules_completed: 0,
        started_at: Some(chrono::Utc::now().to_rfc3339()),
    };

    #[derive(Serialize)]
    struct DryRunResponse {
        status: RunStatus,
        execution_order: Vec<String>,
        rules: Vec<RuleSummary>,
        config: RunConfig,
    }

    Ok(Json(DryRunResponse {
        status,
        execution_order: order,
        rules,
        config: run_config,
    }))
}

/// `POST /api/reports/generate` — Generate a report from a workflow.
async fn generate_report(Json(req): Json<ReportRequest>) -> Result<impl IntoResponse, ApiError> {
    let config = oxo_flow_core::WorkflowConfig::parse(&req.toml_content)
        .map_err(|e| ApiError::bad_request("Invalid workflow TOML", Some(e.to_string())))?;

    let dag = oxo_flow_core::WorkflowDag::from_rules(&config.rules)
        .map_err(|e| ApiError::unprocessable("DAG construction failed", Some(e.to_string())))?;

    let order = dag.execution_order().map_err(|e| {
        ApiError::unprocessable("Cannot determine execution order", Some(e.to_string()))
    })?;

    // Build a report with workflow overview and rule details.
    let mut report = oxo_flow_core::report::Report::new(
        &format!("{} — Workflow Report", config.workflow.name),
        &config.workflow.name,
        &config.workflow.version,
    );

    report.add_metadata("rules_count", &config.rules.len().to_string());
    report.add_metadata("edges_count", &dag.edge_count().to_string());

    // Overview section
    let overview = oxo_flow_core::report::ReportSection {
        title: "Workflow Overview".to_string(),
        id: "overview".to_string(),
        content: oxo_flow_core::report::ReportContent::KeyValue {
            pairs: vec![
                ("Name".to_string(), config.workflow.name.clone()),
                ("Version".to_string(), config.workflow.version.clone()),
                (
                    "Description".to_string(),
                    config
                        .workflow
                        .description
                        .clone()
                        .unwrap_or_else(|| "N/A".to_string()),
                ),
                ("Rules".to_string(), config.rules.len().to_string()),
                ("DAG edges".to_string(), dag.edge_count().to_string()),
            ],
        },
        subsections: vec![],
    };
    report.add_section(overview);

    // Execution order section
    let exec_section = oxo_flow_core::report::ReportSection {
        title: "Execution Order".to_string(),
        id: "execution-order".to_string(),
        content: oxo_flow_core::report::ReportContent::Table {
            headers: vec![
                "Step".to_string(),
                "Rule".to_string(),
                "Threads".to_string(),
                "Environment".to_string(),
            ],
            rows: order
                .iter()
                .enumerate()
                .filter_map(|(i, name)| {
                    config.get_rule(name).map(|r| {
                        vec![
                            (i + 1).to_string(),
                            r.name.clone(),
                            r.effective_threads().to_string(),
                            r.environment.kind().to_string(),
                        ]
                    })
                })
                .collect(),
        },
        subsections: vec![],
    };
    report.add_section(exec_section);

    let format = req.format.unwrap_or_else(|| "html".to_string());

    match format.as_str() {
        "json" => {
            let json = report.to_json().map_err(|e| {
                ApiError::unprocessable("Report generation failed", Some(e.to_string()))
            })?;
            Ok((StatusCode::OK, [("content-type", "application/json")], json))
        }
        _ => {
            let html = report.to_html();
            Ok((StatusCode::OK, [("content-type", "text/html")], html))
        }
    }
}

/// `POST /api/workflows/run` — Validate and return an execution plan as if starting a run.
async fn run_workflow(Json(req): Json<DryRunRequest>) -> Result<impl IntoResponse, ApiError> {
    let config = oxo_flow_core::WorkflowConfig::parse(&req.toml_content)
        .map_err(|e| ApiError::bad_request("Invalid workflow TOML", Some(e.to_string())))?;

    let dag = oxo_flow_core::WorkflowDag::from_rules(&config.rules)
        .map_err(|e| ApiError::unprocessable("DAG construction failed", Some(e.to_string())))?;

    let order = dag.execution_order().map_err(|e| {
        ApiError::unprocessable("Cannot determine execution order", Some(e.to_string()))
    })?;

    Ok(Json(RunResponse {
        run_id: uuid::Uuid::new_v4().to_string(),
        status: "started".to_string(),
        execution_order: order.clone(),
        rules_total: order.len(),
    }))
}

/// `GET /api/version` — Return crate version and build info.
async fn version() -> Json<VersionResponse> {
    Json(VersionResponse {
        version: env!("CARGO_PKG_VERSION").to_string(),
        crate_name: env!("CARGO_PKG_NAME").to_string(),
        rust_version: option_env!("CARGO_PKG_RUST_VERSION")
            .unwrap_or("unknown")
            .to_string(),
    })
}

/// `POST /api/workflows/clean` — List output files that would be cleaned.
async fn clean_workflow(Json(req): Json<ValidateRequest>) -> Result<Json<CleanResponse>, ApiError> {
    let config = oxo_flow_core::WorkflowConfig::parse(&req.toml_content)
        .map_err(|e| ApiError::bad_request("Invalid workflow TOML", Some(e.to_string())))?;

    let files_to_clean: Vec<String> = config.rules.iter().flat_map(|r| r.output.clone()).collect();

    Ok(Json(CleanResponse {
        workflow_name: config.workflow.name.clone(),
        total_files: files_to_clean.len(),
        files_to_clean,
    }))
}

// ---------------------------------------------------------------------------
// Request ID middleware
// ---------------------------------------------------------------------------

/// Middleware that attaches a unique `x-request-id` header to every response.
async fn add_request_id(
    request: axum::http::Request<axum::body::Body>,
    next: axum::middleware::Next,
) -> axum::response::Response {
    let request_id = uuid::Uuid::new_v4().to_string();
    let mut response = next.run(request).await;
    response.headers_mut().insert(
        axum::http::HeaderName::from_static("x-request-id"),
        axum::http::HeaderValue::from_str(&request_id).unwrap(),
    );
    response
}

// ---------------------------------------------------------------------------
// Export endpoint
// ---------------------------------------------------------------------------

/// `POST /api/workflows/export` — Generate a Dockerfile or Singularity def.
async fn export_workflow(Json(req): Json<ExportRequest>) -> Result<Json<ExportResponse>, ApiError> {
    let config = oxo_flow_core::WorkflowConfig::parse(&req.toml_content)
        .map_err(|e| ApiError::bad_request("Invalid workflow TOML", Some(e.to_string())))?;

    let format = req.format.unwrap_or_else(|| "docker".to_string());
    let pkg_config = oxo_flow_core::container::PackageConfig::default();

    let content = match format.as_str() {
        "singularity" => oxo_flow_core::container::generate_singularity_def(&config, &pkg_config)
            .map_err(|e| {
            ApiError::unprocessable("Singularity def generation failed", Some(e.to_string()))
        })?,
        _ => oxo_flow_core::container::generate_dockerfile(&config, &pkg_config).map_err(|e| {
            ApiError::unprocessable("Dockerfile generation failed", Some(e.to_string()))
        })?,
    };

    let actual_format = match format.as_str() {
        "singularity" => "singularity".to_string(),
        _ => "docker".to_string(),
    };

    Ok(Json(ExportResponse {
        format: actual_format,
        content,
    }))
}

// ---------------------------------------------------------------------------
// Frontend & new endpoints
// ---------------------------------------------------------------------------

/// Serve the embedded frontend HTML.
async fn frontend() -> impl IntoResponse {
    (
        StatusCode::OK,
        [("content-type", "text/html; charset=utf-8")],
        FRONTEND_HTML,
    )
}

/// `POST /api/workflows/format` — Format a workflow TOML into canonical form.
async fn format_workflow_endpoint(
    Json(req): Json<ValidateRequest>,
) -> Result<Json<FormatResponse>, ApiError> {
    let config = oxo_flow_core::WorkflowConfig::parse(&req.toml_content)
        .map_err(|e| ApiError::bad_request("Invalid workflow TOML", Some(e.to_string())))?;

    let formatted = oxo_flow_core::format::format_workflow(&config);

    Ok(Json(FormatResponse { formatted }))
}

/// `POST /api/workflows/lint` — Lint a workflow for best practices.
async fn lint_workflow(Json(req): Json<LintRequest>) -> Result<Json<LintResponse>, ApiError> {
    let config = oxo_flow_core::WorkflowConfig::parse(&req.toml_content)
        .map_err(|e| ApiError::bad_request("Invalid workflow TOML", Some(e.to_string())))?;

    let validation = oxo_flow_core::format::validate_format(&config);
    let lint_diags = oxo_flow_core::format::lint_format(&config);

    let mut diagnostics = Vec::new();
    let mut error_count = 0;
    let mut warning_count = 0;
    let mut info_count = 0;

    for d in validation.diagnostics.iter().chain(lint_diags.iter()) {
        let severity = match d.severity {
            oxo_flow_core::format::Severity::Error => {
                error_count += 1;
                "error"
            }
            oxo_flow_core::format::Severity::Warning => {
                warning_count += 1;
                "warning"
            }
            oxo_flow_core::format::Severity::Info => {
                info_count += 1;
                "info"
            }
        };
        diagnostics.push(DiagnosticItem {
            severity: severity.to_string(),
            code: d.code.clone(),
            message: d.message.clone(),
            rule: d.rule.clone(),
        });
    }

    Ok(Json(LintResponse {
        diagnostics,
        error_count,
        warning_count,
        info_count,
    }))
}

/// `POST /api/workflows/stats` — Return workflow statistics.
async fn workflow_stats_endpoint(
    Json(req): Json<ValidateRequest>,
) -> Result<Json<StatsResponse>, ApiError> {
    let config = oxo_flow_core::WorkflowConfig::parse(&req.toml_content)
        .map_err(|e| ApiError::bad_request("Invalid workflow TOML", Some(e.to_string())))?;

    let stats = oxo_flow_core::format::workflow_stats(&config);

    Ok(Json(StatsResponse {
        rule_count: stats.rule_count,
        shell_rules: stats.shell_rules,
        script_rules: stats.script_rules,
        dependency_count: stats.dependency_count,
        parallel_groups: stats.parallel_groups,
        max_depth: stats.max_depth,
        environments: stats.environments,
        total_threads: stats.total_threads,
        wildcard_count: stats.wildcard_count,
        wildcard_names: stats.wildcard_names,
    }))
}

/// `GET /api/system` — Return system information.
async fn system_info() -> Json<SystemInfo> {
    let uptime = get_start_time().elapsed().as_secs_f64();
    Json(SystemInfo {
        version: env!("CARGO_PKG_VERSION").to_string(),
        rust_version: option_env!("CARGO_PKG_RUST_VERSION")
            .unwrap_or("unknown")
            .to_string(),
        os: std::env::consts::OS.to_string(),
        arch: std::env::consts::ARCH.to_string(),
        pid: std::process::id(),
        uptime_secs: uptime,
    })
}

/// `GET /api/events` — SSE endpoint for real-time execution events.
async fn sse_events() -> impl IntoResponse {
    use axum::response::sse::{Event, Sse};
    use tokio_stream::StreamExt as _;

    let stream = tokio_stream::wrappers::IntervalStream::new(tokio::time::interval(
        std::time::Duration::from_secs(5),
    ))
    .map(|_| {
        let msg = format!(
            r#"{{"type":"heartbeat","time":"{}"}}"#,
            chrono::Utc::now().to_rfc3339()
        );
        Ok::<_, std::convert::Infallible>(Event::default().data(msg))
    });

    Sse::new(stream).keep_alive(
        axum::response::sse::KeepAlive::new()
            .interval(std::time::Duration::from_secs(15))
            .text("ping"),
    )
}

// ---------------------------------------------------------------------------
// Authentication & license endpoints
// ---------------------------------------------------------------------------

/// `POST /api/auth/login` — Authenticate and obtain a session token.
async fn login(Json(req): Json<LoginRequest>) -> Result<Json<LoginResponse>, ApiError> {
    let role = check_credentials(&req.username, &req.password).ok_or_else(|| ApiError {
        status: StatusCode::UNAUTHORIZED,
        body: ErrorResponse {
            error: "Invalid credentials".to_string(),
            detail: None,
        },
    })?;

    let token = generate_session_token();
    let session = Session {
        username: req.username.clone(),
        role,
        created_at: chrono::Utc::now().to_rfc3339(),
    };

    if let Ok(mut store) = sessions().lock() {
        store.insert(token.clone(), session);
    }

    Ok(Json(LoginResponse {
        token,
        username: req.username,
        role: role.to_string(),
    }))
}

/// `GET /api/auth/me` — Return the identity of the current session.
async fn auth_me(headers: axum::http::HeaderMap) -> Json<AuthMeResponse> {
    match extract_session(&headers) {
        Some(session) => Json(AuthMeResponse {
            authenticated: true,
            username: Some(session.username),
            role: Some(session.role.to_string()),
        }),
        None => Json(AuthMeResponse {
            authenticated: false,
            username: None,
            role: None,
        }),
    }
}

/// `GET /api/license` — Return current license status.
async fn license_status() -> Json<LicenseStatus> {
    Json(check_license())
}

// ---------------------------------------------------------------------------
// Router
// ---------------------------------------------------------------------------

/// Build the web application router.
pub fn build_router() -> Router {
    // Initialize start time
    get_start_time();

    let cors = CorsLayer::new()
        .allow_origin(Any)
        .allow_methods(Any)
        .allow_headers(Any);

    Router::new()
        // Frontend
        .route("/", get(frontend))
        // API endpoints
        .route("/api/health", get(health))
        .route("/api/version", get(version))
        .route("/api/system", get(system_info))
        .route("/api/workflows", get(list_workflows))
        .route("/api/workflows/validate", post(validate_workflow))
        .route("/api/workflows/parse", post(parse_workflow))
        .route("/api/workflows/dag", post(build_dag))
        .route("/api/workflows/dry-run", post(dry_run))
        .route("/api/workflows/run", post(run_workflow))
        .route("/api/workflows/clean", post(clean_workflow))
        .route("/api/workflows/export", post(export_workflow))
        .route("/api/workflows/format", post(format_workflow_endpoint))
        .route("/api/workflows/lint", post(lint_workflow))
        .route("/api/workflows/stats", post(workflow_stats_endpoint))
        .route("/api/environments", get(list_environments))
        .route("/api/reports/generate", post(generate_report))
        .route("/api/events", get(sse_events))
        // Authentication & license
        .route("/api/auth/login", post(login))
        .route("/api/auth/me", get(auth_me))
        .route("/api/license", get(license_status))
        .fallback(not_found)
        .layer(middleware::from_fn(add_request_id))
        .layer(cors)
}

/// Build a router mounted under a configurable base path.
pub fn build_router_with_base(base_path: &str) -> Router {
    let app = build_router();
    if base_path.is_empty() || base_path == "/" {
        app
    } else {
        Router::new().nest(base_path, app)
    }
}

/// Start the web server.
pub async fn start_server(host: &str, port: u16) -> anyhow::Result<()> {
    let app = build_router();
    let addr = format!("{host}:{port}");
    tracing::info!("Starting oxo-flow web server on {}", addr);

    let listener = tokio::net::TcpListener::bind(&addr).await?;
    axum::serve(listener, app).await?;

    Ok(())
}

/// Start the web server with an optional base path.
pub async fn start_server_with_base(host: &str, port: u16, base_path: &str) -> anyhow::Result<()> {
    let app = build_router_with_base(base_path);
    let addr = format!("{host}:{port}");
    tracing::info!(
        "Starting oxo-flow web server on {} (base: {})",
        addr,
        if base_path.is_empty() { "/" } else { base_path }
    );

    let listener = tokio::net::TcpListener::bind(&addr).await?;
    axum::serve(listener, app).await?;

    Ok(())
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::http::Request;
    use tower::ServiceExt;

    /// A minimal valid workflow TOML used across tests.
    const VALID_TOML: &str = r#"
[workflow]
name = "test-pipeline"
version = "1.0.0"
description = "A test workflow"
author = "Test Author"

[[rules]]
name = "step_a"
input = ["raw/{sample}.fastq"]
output = ["trimmed/{sample}.fastq"]
shell = "trim {input} > {output}"
threads = 4

[[rules]]
name = "step_b"
input = ["trimmed/{sample}.fastq"]
output = ["aligned/{sample}.bam"]
shell = "align {input} > {output}"
threads = 8
[rules.environment]
docker = "biocontainers/bwa:0.7.17"
"#;

    /// Helper: send a POST request with a JSON body and return the response.
    async fn post_json(uri: &str, body: impl Serialize) -> axum::http::Response<Body> {
        let app = build_router();
        app.oneshot(
            Request::builder()
                .method("POST")
                .uri(uri)
                .header("content-type", "application/json")
                .body(Body::from(serde_json::to_vec(&body).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap()
    }

    // -- Existing endpoint tests -------------------------------------------------

    #[tokio::test]
    async fn health_endpoint() {
        let app = build_router();
        let response = app
            .oneshot(
                Request::builder()
                    .uri("/api/health")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let parsed: HealthResponse = serde_json::from_slice(&body).unwrap();
        assert_eq!(parsed.status, "ok");
    }

    #[tokio::test]
    async fn workflows_endpoint() {
        let app = build_router();
        let response = app
            .oneshot(
                Request::builder()
                    .uri("/api/workflows")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn environments_endpoint() {
        let app = build_router();
        let response = app
            .oneshot(
                Request::builder()
                    .uri("/api/environments")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn not_found_fallback() {
        let app = build_router();
        let response = app
            .oneshot(
                Request::builder()
                    .uri("/nonexistent")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::NOT_FOUND);
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let parsed: ErrorResponse = serde_json::from_slice(&body).unwrap();
        assert_eq!(parsed.error, "Not found");
    }

    // -- Validate endpoint -------------------------------------------------------

    #[tokio::test]
    async fn validate_valid_toml() {
        let resp = post_json(
            "/api/workflows/validate",
            &ValidateRequest {
                toml_content: VALID_TOML.to_string(),
            },
        )
        .await;

        assert_eq!(resp.status(), StatusCode::OK);
        let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let parsed: ValidateResponse = serde_json::from_slice(&body).unwrap();
        assert!(parsed.valid);
        assert!(parsed.errors.is_empty());
        assert_eq!(parsed.rules_count, Some(2));
        assert!(parsed.edges_count.is_some());
    }

    #[tokio::test]
    async fn validate_invalid_toml() {
        let resp = post_json(
            "/api/workflows/validate",
            &ValidateRequest {
                toml_content: "this is not valid toml {{{{".to_string(),
            },
        )
        .await;

        assert_eq!(resp.status(), StatusCode::OK);
        let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let parsed: ValidateResponse = serde_json::from_slice(&body).unwrap();
        assert!(!parsed.valid);
        assert!(!parsed.errors.is_empty());
    }

    // -- Parse endpoint ----------------------------------------------------------

    #[tokio::test]
    async fn parse_valid_workflow() {
        let resp = post_json(
            "/api/workflows/parse",
            &ValidateRequest {
                toml_content: VALID_TOML.to_string(),
            },
        )
        .await;

        assert_eq!(resp.status(), StatusCode::OK);
        let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let detail: WorkflowDetail = serde_json::from_slice(&body).unwrap();
        assert_eq!(detail.name, "test-pipeline");
        assert_eq!(detail.version, "1.0.0");
        assert_eq!(detail.description.as_deref(), Some("A test workflow"));
        assert_eq!(detail.author.as_deref(), Some("Test Author"));
        assert_eq!(detail.rules_count, 2);
        assert_eq!(detail.rules[0].name, "step_a");
        assert_eq!(detail.rules[0].threads, 4);
        assert_eq!(detail.rules[1].environment, "docker");
    }

    #[tokio::test]
    async fn parse_invalid_toml_returns_400() {
        let resp = post_json(
            "/api/workflows/parse",
            &ValidateRequest {
                toml_content: "not valid".to_string(),
            },
        )
        .await;

        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
        let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let err: ErrorResponse = serde_json::from_slice(&body).unwrap();
        assert!(!err.error.is_empty());
    }

    // -- DAG endpoint ------------------------------------------------------------

    #[tokio::test]
    async fn dag_generation() {
        let resp = post_json(
            "/api/workflows/dag",
            &ValidateRequest {
                toml_content: VALID_TOML.to_string(),
            },
        )
        .await;

        assert_eq!(resp.status(), StatusCode::OK);
        let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let dag: DagResponse = serde_json::from_slice(&body).unwrap();
        assert!(dag.dot.contains("digraph"));
        assert_eq!(dag.nodes, 2);
        assert!(dag.edges >= 1);
    }

    // -- Dry-run endpoint --------------------------------------------------------

    #[tokio::test]
    async fn dry_run_endpoint() {
        let resp = post_json(
            "/api/workflows/dry-run",
            &DryRunRequest {
                toml_content: VALID_TOML.to_string(),
                config: Some(RunConfig {
                    max_jobs: Some(4),
                    dry_run: Some(true),
                    keep_going: Some(false),
                }),
            },
        )
        .await;

        assert_eq!(resp.status(), StatusCode::OK);
        let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let value: serde_json::Value = serde_json::from_slice(&body).unwrap();

        let status = &value["status"];
        assert_eq!(status["status"], "dry-run");
        assert_eq!(status["rules_total"], 2);
        assert_eq!(status["rules_completed"], 0);

        let order = value["execution_order"].as_array().unwrap();
        assert_eq!(order.len(), 2);
        // step_a must come before step_b (dependency)
        assert_eq!(order[0], "step_a");
        assert_eq!(order[1], "step_b");

        let rules = value["rules"].as_array().unwrap();
        assert_eq!(rules.len(), 2);
    }

    // -- Report generation endpoint ----------------------------------------------

    #[tokio::test]
    async fn report_generate_html() {
        let resp = post_json(
            "/api/reports/generate",
            &ReportRequest {
                toml_content: VALID_TOML.to_string(),
                format: None, // default → html
            },
        )
        .await;

        assert_eq!(resp.status(), StatusCode::OK);
        let ct = resp
            .headers()
            .get("content-type")
            .unwrap()
            .to_str()
            .unwrap();
        assert_eq!(ct, "text/html");

        let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let html = String::from_utf8(body.to_vec()).unwrap();
        assert!(html.contains("<!DOCTYPE html>"));
        assert!(html.contains("test-pipeline"));
    }

    #[tokio::test]
    async fn report_generate_json() {
        let resp = post_json(
            "/api/reports/generate",
            &ReportRequest {
                toml_content: VALID_TOML.to_string(),
                format: Some("json".to_string()),
            },
        )
        .await;

        assert_eq!(resp.status(), StatusCode::OK);
        let ct = resp
            .headers()
            .get("content-type")
            .unwrap()
            .to_str()
            .unwrap();
        assert_eq!(ct, "application/json");

        let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let value: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(value["workflow_name"], "test-pipeline");
        assert!(value["sections"].as_array().unwrap().len() >= 2);
    }

    // -- Run endpoint ------------------------------------------------------------

    #[tokio::test]
    async fn run_workflow_endpoint() {
        let resp = post_json(
            "/api/workflows/run",
            &DryRunRequest {
                toml_content: VALID_TOML.to_string(),
                config: None,
            },
        )
        .await;

        assert_eq!(resp.status(), StatusCode::OK);
        let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let parsed: RunResponse = serde_json::from_slice(&body).unwrap();
        assert_eq!(parsed.status, "started");
        assert_eq!(parsed.rules_total, 2);
        assert!(!parsed.run_id.is_empty());
        assert_eq!(parsed.execution_order.len(), 2);
        assert_eq!(parsed.execution_order[0], "step_a");
        assert_eq!(parsed.execution_order[1], "step_b");
    }

    // -- Version endpoint --------------------------------------------------------

    #[tokio::test]
    async fn version_endpoint() {
        let app = build_router();
        let response = app
            .oneshot(
                Request::builder()
                    .uri("/api/version")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let parsed: VersionResponse = serde_json::from_slice(&body).unwrap();
        assert_eq!(parsed.version, env!("CARGO_PKG_VERSION"));
        assert_eq!(parsed.crate_name, "oxo-flow-web");
    }

    // -- Clean endpoint ----------------------------------------------------------

    #[tokio::test]
    async fn clean_workflow_endpoint() {
        let resp = post_json(
            "/api/workflows/clean",
            &ValidateRequest {
                toml_content: VALID_TOML.to_string(),
            },
        )
        .await;

        assert_eq!(resp.status(), StatusCode::OK);
        let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let parsed: CleanResponse = serde_json::from_slice(&body).unwrap();
        assert_eq!(parsed.workflow_name, "test-pipeline");
        assert_eq!(parsed.total_files, 2);
        assert!(
            parsed
                .files_to_clean
                .contains(&"trimmed/{sample}.fastq".to_string())
        );
        assert!(
            parsed
                .files_to_clean
                .contains(&"aligned/{sample}.bam".to_string())
        );
    }

    // -- Additional error-path & edge-case tests --------------------------------

    #[tokio::test]
    async fn run_invalid_toml_returns_400() {
        let resp = post_json(
            "/api/workflows/run",
            &DryRunRequest {
                toml_content: "not valid toml {{{{".to_string(),
                config: None,
            },
        )
        .await;

        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
        let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let err: ErrorResponse = serde_json::from_slice(&body).unwrap();
        assert!(!err.error.is_empty());
    }

    #[tokio::test]
    async fn clean_invalid_toml_returns_400() {
        let resp = post_json(
            "/api/workflows/clean",
            &ValidateRequest {
                toml_content: "not valid toml {{{{".to_string(),
            },
        )
        .await;

        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
        let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let err: ErrorResponse = serde_json::from_slice(&body).unwrap();
        assert!(!err.error.is_empty());
    }

    #[tokio::test]
    async fn validate_workflow_with_cycle() {
        let cycle_toml = r#"
[workflow]
name = "cycle-test"
version = "1.0.0"

[[rules]]
name = "step_a"
input = ["b_output.txt"]
output = ["a_output.txt"]
shell = "echo a"

[[rules]]
name = "step_b"
input = ["a_output.txt"]
output = ["b_output.txt"]
shell = "echo b"
"#;

        let resp = post_json(
            "/api/workflows/validate",
            &ValidateRequest {
                toml_content: cycle_toml.to_string(),
            },
        )
        .await;

        assert_eq!(resp.status(), StatusCode::OK);
        let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let parsed: ValidateResponse = serde_json::from_slice(&body).unwrap();
        assert!(!parsed.valid);
        assert!(!parsed.errors.is_empty());
        let joined = parsed.errors.join(" ");
        assert!(
            joined.to_lowercase().contains("cycle"),
            "expected cycle error, got: {joined}"
        );
    }

    #[tokio::test]
    async fn dag_invalid_toml_returns_400() {
        let resp = post_json(
            "/api/workflows/dag",
            &ValidateRequest {
                toml_content: "not valid toml {{{{".to_string(),
            },
        )
        .await;

        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
        let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let err: ErrorResponse = serde_json::from_slice(&body).unwrap();
        assert!(!err.error.is_empty());
    }

    #[tokio::test]
    async fn dry_run_without_config() {
        let resp = post_json(
            "/api/workflows/dry-run",
            &DryRunRequest {
                toml_content: VALID_TOML.to_string(),
                config: None,
            },
        )
        .await;

        assert_eq!(resp.status(), StatusCode::OK);
        let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let value: serde_json::Value = serde_json::from_slice(&body).unwrap();

        let status = &value["status"];
        assert_eq!(status["status"], "dry-run");
        assert_eq!(status["rules_total"], 2);

        let order = value["execution_order"].as_array().unwrap();
        assert_eq!(order.len(), 2);
        assert_eq!(order[0], "step_a");
        assert_eq!(order[1], "step_b");
    }

    // -- Export endpoint ---------------------------------------------------------

    #[tokio::test]
    async fn export_workflow_docker() {
        let resp = post_json(
            "/api/workflows/export",
            &ExportRequest {
                toml_content: VALID_TOML.to_string(),
                format: None, // default → docker
            },
        )
        .await;

        assert_eq!(resp.status(), StatusCode::OK);
        let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let parsed: ExportResponse = serde_json::from_slice(&body).unwrap();
        assert_eq!(parsed.format, "docker");
        assert!(parsed.content.contains("FROM"));
        assert!(parsed.content.contains("test-pipeline"));
    }

    // -- Frontend endpoint -------------------------------------------------------

    #[tokio::test]
    async fn frontend_endpoint() {
        let app = build_router();
        let response = app
            .oneshot(Request::builder().uri("/").body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let html = String::from_utf8_lossy(&body);
        assert!(html.contains("oxo-flow"));
        assert!(html.contains("Pipeline Engine"));
    }

    // -- System info endpoint ----------------------------------------------------

    #[tokio::test]
    async fn system_info_endpoint() {
        let app = build_router();
        let response = app
            .oneshot(
                Request::builder()
                    .uri("/api/system")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let info: SystemInfo = serde_json::from_slice(&body).unwrap();
        assert!(!info.version.is_empty());
        assert!(!info.os.is_empty());
    }

    // -- Format endpoint ---------------------------------------------------------

    #[tokio::test]
    async fn format_endpoint() {
        let body = ValidateRequest {
            toml_content: VALID_TOML.to_string(),
        };
        let response = post_json("/api/workflows/format", &body).await;
        assert_eq!(response.status(), StatusCode::OK);
        let body_bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let resp: FormatResponse = serde_json::from_slice(&body_bytes).unwrap();
        assert!(resp.formatted.contains("[workflow]"));
    }

    #[tokio::test]
    async fn format_endpoint_invalid_toml() {
        let body = ValidateRequest {
            toml_content: "broken!!!".to_string(),
        };
        let response = post_json("/api/workflows/format", &body).await;
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    }

    // -- Lint endpoint -----------------------------------------------------------

    #[tokio::test]
    async fn lint_endpoint() {
        let body = LintRequest {
            toml_content: VALID_TOML.to_string(),
        };
        let response = post_json("/api/workflows/lint", &body).await;
        assert_eq!(response.status(), StatusCode::OK);
        let body_bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let resp: LintResponse = serde_json::from_slice(&body_bytes).unwrap();
        assert_eq!(resp.error_count, 0);
    }

    #[tokio::test]
    async fn lint_endpoint_invalid_toml() {
        let body = LintRequest {
            toml_content: "not valid toml [[[".to_string(),
        };
        let response = post_json("/api/workflows/lint", &body).await;
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    }

    // -- Stats endpoint ----------------------------------------------------------

    #[tokio::test]
    async fn stats_endpoint() {
        let body = ValidateRequest {
            toml_content: VALID_TOML.to_string(),
        };
        let response = post_json("/api/workflows/stats", &body).await;
        assert_eq!(response.status(), StatusCode::OK);
        let body_bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let resp: StatsResponse = serde_json::from_slice(&body_bytes).unwrap();
        assert_eq!(resp.rule_count, 2);
    }

    // -- SSE events endpoint -----------------------------------------------------

    #[tokio::test]
    async fn events_endpoint_returns_sse() {
        let app = build_router();
        let response = app
            .oneshot(
                Request::builder()
                    .uri("/api/events")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
    }

    // -- Auth endpoints ----------------------------------------------------------

    #[tokio::test]
    async fn login_valid_admin() {
        let resp = post_json(
            "/api/auth/login",
            &LoginRequest {
                username: "admin".to_string(),
                password: "admin".to_string(),
            },
        )
        .await;

        assert_eq!(resp.status(), StatusCode::OK);
        let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let parsed: LoginResponse = serde_json::from_slice(&body).unwrap();
        assert_eq!(parsed.username, "admin");
        assert_eq!(parsed.role, "admin");
        assert!(!parsed.token.is_empty());
    }

    #[tokio::test]
    async fn login_valid_user() {
        let resp = post_json(
            "/api/auth/login",
            &LoginRequest {
                username: "user".to_string(),
                password: "user".to_string(),
            },
        )
        .await;

        assert_eq!(resp.status(), StatusCode::OK);
        let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let parsed: LoginResponse = serde_json::from_slice(&body).unwrap();
        assert_eq!(parsed.role, "user");
    }

    #[tokio::test]
    async fn login_invalid_credentials() {
        let resp = post_json(
            "/api/auth/login",
            &LoginRequest {
                username: "admin".to_string(),
                password: "wrong".to_string(),
            },
        )
        .await;

        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
        let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let err: ErrorResponse = serde_json::from_slice(&body).unwrap();
        assert!(err.error.contains("Invalid"));
    }

    #[tokio::test]
    async fn auth_me_without_token() {
        let app = build_router();
        let response = app
            .oneshot(
                Request::builder()
                    .uri("/api/auth/me")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let parsed: AuthMeResponse = serde_json::from_slice(&body).unwrap();
        assert!(!parsed.authenticated);
        assert!(parsed.username.is_none());
    }

    // -- License endpoint --------------------------------------------------------

    #[tokio::test]
    async fn license_endpoint() {
        let app = build_router();
        let response = app
            .oneshot(
                Request::builder()
                    .uri("/api/license")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let status: LicenseStatus = serde_json::from_slice(&body).unwrap();
        // No license file installed in test environment
        assert!(!status.valid);
        assert!(!status.message.is_empty());
    }
}
