const API = '';
let token = '';
let user = null;
let currentPreviewTemplate = '';
let currentEditingWfId = null; // For upsert support
let logAutoRefreshTimer = null;
let currentLogRunId = null;
let currentHpcRunId = null;

document.querySelectorAll('.sidebar-nav button').forEach(btn => {
  btn.addEventListener('click', () => {
    document.querySelectorAll('.sidebar-nav button').forEach(b => b.classList.remove('active'));
    btn.classList.add('active');
    document.querySelectorAll('.view').forEach(v => v.classList.remove('active'));
    document.getElementById('view-' + btn.dataset.view).classList.add('active');
    document.getElementById('view-title').textContent = btn.textContent.trim().replace(/^\S+\s*/, '');
    if (btn.dataset.view === 'dashboard') refreshDashboard();
    if (btn.dataset.view === 'runs') refreshRuns();
    if (btn.dataset.view === 'workflows') loadSavedWorkflows();
    if (btn.dataset.view === 'templates') loadTemplates();
    if (btn.dataset.view === 'scheduled') refreshScheduledRuns();
    if (btn.dataset.view === 'users') refreshUsers();
    if (btn.dataset.view === 'system') refreshSystem();
  });
});

async function api(method, path, body) {
  var headers = { 'Content-Type': 'application/json' };
  if (token) headers['Authorization'] = 'Bearer ' + token;
  var opts = { method: method, headers: headers };
  if (body) opts.body = JSON.stringify(body);
  var resp = await fetch(API + path, opts);
  var text = await resp.text();
  try { return { status: resp.status, data: JSON.parse(text) }; } catch(e) { return { status: resp.status, data: text }; }
}

function showLogin() {
  document.getElementById('login-modal').classList.remove('hidden');
  document.getElementById('login-username').focus();
}
function closeLoginModal() {
  document.getElementById('login-modal').classList.add('hidden');
  document.getElementById('login-error').style.display = 'none';
}
async function doLogin() {
  var u = document.getElementById('login-username').value.trim();
  var p = document.getElementById('login-password').value;
  if (!u || !p) return;
  var r = await api('POST', '/api/auth/login', { username: u, password: p });
  if (r.status === 200) {
    token = r.data.token;
    user = { name: r.data.username, role: r.data.role };
    document.getElementById('user-name').textContent = user.name;
    document.getElementById('avatar').textContent = user.name[0].toUpperCase();
    document.getElementById('conn-dot').className = 'status-dot ok';
    closeLoginModal();
  } else {
    var el = document.getElementById('login-error');
    el.textContent = r.data.error || 'Login failed';
    el.style.display = 'block';
  }
}
// Support Enter key in login form
document.addEventListener('DOMContentLoaded', function() {
  document.getElementById('login-password').addEventListener('keydown', function(e) {
    if (e.key === 'Enter') doLogin();
  });
});

async function checkSession() {
  var r = await api('GET', '/api/auth/me');
  if (r.status === 200 && r.data.authenticated) {
    user = { name: r.data.username, role: r.data.role };
    document.getElementById('user-name').textContent = user.name;
    document.getElementById('avatar').textContent = user.name[0].toUpperCase();
    document.getElementById('conn-dot').className = 'status-dot ok';
  }
}

async function updateTopbar() {
  try {
    var r = await api('GET', '/api/metrics');
    if (!r.data || !r.data.host) return;
    document.getElementById('live-cpu').textContent = (r.data.host.cpu_usage_percent || 0).toFixed(1) + '%';
    document.getElementById('live-mem').textContent = ((r.data.host.used_memory_mb||0)/1024).toFixed(0) + '/' + ((r.data.host.total_memory_mb||1)/1024).toFixed(0) + 'GB';
    document.getElementById('live-runs').textContent = r.data.active_workflows || 0;
    document.getElementById('live-uptime').textContent = fmtDur(r.data.uptime_secs || 0);
  } catch(e) {}
}
setInterval(updateTopbar, 5000);

async function refreshDashboard() {
  try {
    var r = await api('GET', '/api/metrics');
    if (r.data && r.data.host) {
      document.getElementById('d-cpu').textContent = (r.data.host.cpu_usage_percent||0).toFixed(1)+'%';
      document.getElementById('d-cpu-bar').style.width = (r.data.host.cpu_usage_percent||0).toFixed(0)+'%';
      var u=(r.data.host.used_memory_mb||0)/1024, t=(r.data.host.total_memory_mb||1)/1024;
      document.getElementById('d-mem').textContent = u.toFixed(0)+' / '+t.toFixed(0)+' GB';
      document.getElementById('d-mem-bar').style.width = ((u/t)*100).toFixed(0)+'%';
      document.getElementById('d-runs').textContent = r.data.active_workflows || 0;
      document.getElementById('d-uptime').textContent = fmtDur(r.data.uptime_secs || 0);
      document.getElementById('d-requests').textContent = 'Requests: ' + (r.data.total_requests || 0);
    }
  } catch(e) {}
  try {
    var runs = await api('GET', '/api/runs');
    if (Array.isArray(runs.data)) {
      document.getElementById('d-total-runs').textContent = 'Total: ' + runs.data.length;
      document.getElementById('recent-runs-tbody').innerHTML = runs.data.slice(0,10).map(function(r) {
        return '<tr><td style="font-family:var(--mono);font-size:0.72rem">'+r.id.substring(0,8)+'...</td>'+
        '<td>'+esc(r.workflow_name)+'</td>'+
        '<td><span class="badge badge-'+r.status+'">'+r.status+'</span></td>'+
        '<td style="font-size:0.78rem;color:var(--text2)">'+(r.started_at?fmtTime(r.started_at):'--')+'</td>'+
        '<td><button class="btn btn-sm btn-outline" onclick="viewRunDetail(\''+r.id+'\')">Detail</button> '+
        '<button class="btn btn-sm btn-outline" onclick="viewRunLogs(\''+r.id+'\')">Logs</button></td></tr>';
      }).join('') || '<tr><td colspan="5" style="color:var(--text3)">No runs yet</td></tr>';
    }
  } catch(e) {}
}

async function refreshRuns() {
  try {
    var r = await api('GET', '/api/runs');
    var tbody = document.getElementById('all-runs-tbody');
    if (Array.isArray(r.data)) {
      tbody.innerHTML = r.data.map(function(run) {
        return '<tr><td style="font-family:var(--mono);font-size:0.72rem">'+run.id.substring(0,12)+'...</td>'+
        '<td>'+esc(run.workflow_name)+'</td>'+
        '<td><span class="badge badge-'+run.status+'">'+run.status+'</span></td>'+
        '<td style="font-size:0.78rem;color:var(--text2)">'+(run.started_at?fmtTime(run.started_at):'--')+'</td>'+
        '<td style="font-size:0.78rem;color:var(--text2)">'+(run.started_at&&run.finished_at?fmtDur((new Date(run.finished_at)-new Date(run.started_at))/1000):'--')+'</td>'+
        '<td><button class="btn btn-sm btn-outline" onclick="viewRunDetail(\''+run.id+'\')">Detail</button> '+
        '<button class="btn btn-sm btn-outline" onclick="viewRunLogs(\''+run.id+'\')">Logs</button> '+
        (run.status==='running'?'<button class="btn btn-sm btn-danger" onclick="cancelRun(\''+run.id+'\')">Cancel</button>':'')+'</td></tr>';
      }).join('') || '<tr><td colspan="6" style="color:var(--text3)">No runs yet</td></tr>';
    }
  } catch(e) {}
}

function getToml() { return document.getElementById('editor-text').value; }
function showOutput(text, isError) {
  var el = document.getElementById('editor-output');
  el.classList.remove('hidden','error','success');
  el.textContent = text;
  if (isError === true) el.classList.add('error');
  else if (isError === false) el.classList.add('success');
}

async function validateEditor() {
  var r = await api('POST', '/api/workflows/validate', { toml_content: getToml() });
  if (r.data.valid) { showOutput('Valid - ' + r.data.rules_count + ' rules, ' + (r.data.edges_count||0) + ' dependencies', false); updateEditorStats(); }
  else { showOutput('Validation failed:\n' + (r.data.errors||[]).join('\n'), true); }
}

async function dryRunEditor() {
  var r = await api('POST', '/api/workflows/dry-run', { toml_content: getToml() });
  if (r.status === 200) showOutput('Dry-run: ' + r.data.status.rules_total + ' rules\nExecution order:\n' + (r.data.execution_order||[]).map(function(n,i){return '  '+(i+1)+'. '+n;}).join('\n'), false);
  else showOutput('Dry-run failed', true);
}

async function formatEditor() {
  var r = await api('POST', '/api/workflows/format', { toml_content: getToml() });
  if (r.data.formatted) { document.getElementById('editor-text').value = r.data.formatted; showOutput('Formatted successfully', false); }
}

async function lintEditor() {
  var r = await api('POST', '/api/workflows/lint', { toml_content: getToml() });
  showOutput('Lint: ' + r.data.error_count + ' errors, ' + r.data.warning_count + ' warnings\n' + (r.data.diagnostics||[]).map(function(d){return '  ['+d.severity+'] '+d.code+': '+d.message;}).join('\n'), r.data.error_count > 0);
}

async function showDag() {
  document.getElementById('dag-modal').classList.remove('hidden');
  document.getElementById('dag-viz').style.display = 'block';
  document.getElementById('dag-content').style.display = 'none';
  // Fetch JSON DAG data for visualization
  var r = await api('POST', '/api/workflows/dag-json', { toml_content: getToml() });
  // Also fetch DOT text as fallback
  var dotR = await api('POST', '/api/workflows/dag', { toml_content: getToml() });
  document.getElementById('dag-content').textContent = dotR.data.dot || 'No DAG generated';

  if (r.data && r.data.nodes && r.data.nodes.length > 0) {
    // Color map for environment types
    var nodes = (r.data.nodes||[]).map(function(n) {
      return { id: n.id, label: n.label, color: { background: n.color, border: '#1e293b' }, font: { color: '#e2e8f0', size: 13 } };
    });
    var edges = (r.data.edges||[]).map(function(e) { return { from: e.from, to: e.to, arrows: 'to', color: { color: '#475569' } }; });
    var container = document.getElementById('dag-viz');
    // Clear previous network
    container.innerHTML = '';
    var data = { nodes: new vis.DataSet(nodes), edges: new vis.DataSet(edges) };
    var options = {
      layout: { hierarchical: { direction: 'LR', sortMethod: 'directed', nodeSpacing: 150, levelSeparation: 200 } },
      physics: { hierarchicalRepulsion: { nodeDistance: 150 } },
      edges: { smooth: { type: 'cubicBezier', forceDirection: 'horizontal' } }
    };
    if (typeof vis !== 'undefined') { new vis.Network(container, data, options); }
    else { container.innerHTML = '<div style="color:var(--text2);padding:2rem;text-align:center">vis-network CDN loading...</div>'; }
  } else {
    document.getElementById('dag-viz').innerHTML = '<div style="color:var(--text2);padding:2rem;text-align:center">No DAG structure found</div>';
  }
}

function toggleDagView() {
  var viz = document.getElementById('dag-viz');
  var dot = document.getElementById('dag-content');
  var btn = document.querySelector('#dag-modal .btn-sm');
  if (viz.style.display === 'none') {
    viz.style.display = 'block'; dot.style.display = 'none'; btn.textContent = 'Show DOT';
  } else {
    viz.style.display = 'none'; dot.style.display = 'block'; btn.textContent = 'Show Graph';
  }
}

async function runWorkflow() {
  if (!token) { showLogin(); return; }
  var r = await api('POST', '/api/workflows/run', { toml_content: getToml() });
  if (r.status === 200) { showOutput('Launched! Run ID: ' + r.data.run_id + '\nStatus: ' + r.data.status + '\nRules: ' + r.data.rules_total, false); refreshRuns(); }
  else { showOutput('Launch failed: ' + (r.data.error || JSON.stringify(r.data)), true); }
}

async function saveWorkflow() {
  var name = document.getElementById('save-name').value.trim() || 'untitled';
  var version = document.getElementById('save-version') ? document.getElementById('save-version').value.trim() || '1.0.0' : '1.0.0';
  var body = { name: name, version: version, toml_content: getToml() };
  // Upsert: if we loaded an existing workflow, update it
  if (currentEditingWfId) { body.id = currentEditingWfId; }
  var r = await api('POST', '/api/workflows/save', body);
  if (r.status === 201 || r.status === 200) {
    showOutput((currentEditingWfId ? 'Updated' : 'Saved') + ' "' + name + '" (ID: ' + (r.data.id||'').substring(0,8) + '...)', false);
    document.getElementById('save-name').value = '';
    currentEditingWfId = null;
  } else if (r.status === 200 && r.data.status === 'updated') {
    showOutput('Updated "' + name + '"', false);
    document.getElementById('save-name').value = '';
    currentEditingWfId = null;
  } else { showOutput('Save failed: ' + (r.data.error || ''), true); }
}

async function updateEditorStats() {
  try {
    var r = await api('POST', '/api/workflows/stats', { toml_content: getToml() });
    document.getElementById('editor-stats').innerHTML =
      'Rules: '+(r.data.rule_count||0)+'<br>Shell: '+(r.data.shell_rules||0)+' | Script: '+(r.data.script_rules||0)+
      '<br>Deps: '+(r.data.dependency_count||0)+'<br>Parallel: '+(r.data.parallel_groups||0)+
      '<br>Threads: '+(r.data.total_threads||0)+'<br>Envs: '+(r.data.environments||[]).join(', ')||'none';
  } catch(e) {}
}

async function viewRunDetail(runId) {
  var r = await api('GET', '/api/runs/' + runId);
  showOutput(
    'Run: ' + r.data.id + '\nWorkflow: ' + r.data.workflow_name + '\nStatus: ' + r.data.status +
    '\nStarted: ' + (r.data.started_at||'--') + '\nFinished: ' + (r.data.finished_at||'--') +
    '\nPID: ' + (r.data.pid||'--') +
    (r.data.output_files ? '\n\nOutputs:\n' + r.data.output_files.map(function(f){return '  '+f;}).join('\n') : '') +
    (r.data.log_tail ? '\n\n--- Log Tail ---\n' + r.data.log_tail : '')
  , false);
}

async function viewRunLogs(runId) {
  document.getElementById('log-run-id').textContent = runId;
  document.getElementById('log-content').textContent = 'Loading...';
  document.getElementById('log-modal').classList.remove('hidden');
  var r = await api('GET', '/api/runs/' + runId + '/logs');
  document.getElementById('log-content').textContent = typeof r.data === 'string' ? (r.data || '(empty)') : 'Log not available';
}

async function exportWorkflow() {
  var r = await api('POST', '/api/workflows/export', { toml_content: getToml(), format: 'docker' });
  if (r.status === 200 && r.data.content) {
    document.getElementById('export-content').textContent = r.data.content;
    document.getElementById('export-modal').classList.remove('hidden');
  } else { showOutput('Export failed: ' + (r.data.error || ''), true); }
}
function closeLogModal() { document.getElementById('log-modal').classList.add('hidden'); }
function closeDagModal() { document.getElementById('dag-modal').classList.add('hidden'); }
function closeExportModal() { document.getElementById('export-modal').classList.add('hidden'); }

async function cancelRun(runId) {
  if (!confirm('Cancel run ' + runId + '?')) return;
  await fetch(API + '/api/runs/' + runId, { method: 'DELETE', headers: token ? {'Authorization':'Bearer '+token} : {} });
  refreshRuns();
}

async function loadSavedWorkflows() {
  try {
    var r = await api('GET', '/api/workflows/saved');
    document.getElementById('saved-wf-tbody').innerHTML = Array.isArray(r.data) ? r.data.map(function(w) {
      return '<tr><td>'+esc(w.name)+'</td><td>'+esc(w.version)+'</td><td>'+w.rules_count+'</td>'+
      '<td style="font-size:0.78rem;color:var(--text2)">'+fmtTime(w.updated_at)+'</td>'+
      '<td><button class="btn btn-sm btn-outline" onclick="loadWorkflowToEditor(\''+w.id+'\')">Load</button> '+
      '<button class="btn btn-sm btn-danger" onclick="deleteWorkflow(\''+w.id+'\',\''+jsStr(w.name)+'\')" style="margin-left:0.25rem">Del</button></td></tr>';
    }).join('') : '<tr><td colspan="5" style="color:var(--text3)">No saved workflows</td></tr>';
  } catch(e) {}
}

async function deleteWorkflow(wfId, name) {
  if (!confirm('Delete workflow "' + name + '"?')) return;
  var resp = await fetch(API + '/api/workflows/saved/' + wfId, { method: 'DELETE', headers: token ? {'Authorization':'Bearer '+token} : {} });
  if (resp.status === 200) loadSavedWorkflows();
  else alert('Delete failed');
}

var templates = {
  hello: '[workflow]\nname = "hello-world"\nversion = "1.0.0"\ndescription = "My first workflow"\n\n[[rules]]\nname = "greet"\noutput = ["hello.txt"]\nshell = "echo Hello, oxo-flow! > {output[0]}"\n',
  wgs: '[workflow]\nname = "wgs-germline"\nversion = "1.0.0"\ndescription = "Basic germline WGS pipeline"\n\n[config]\nref = "/path/to/reference.fa"\ndata = "/path/to/fastq"\nout = "results"\nsample = "SAMPLE01"\n\n[defaults]\nthreads = 4\nmemory = "8G"\n\n[[rules]]\nname = "fastp_trim"\ninput = ["{config.data}/{config.sample}_R1.fastq.gz", "{config.data}/{config.sample}_R2.fastq.gz"]\noutput = ["{config.out}/trimmed_R1.fq.gz", "{config.out}/trimmed_R2.fq.gz"]\nshell = "mkdir -p {config.out}\\nfastp -i {input[0]} -I {input[1]} -o {output[0]} -O {output[1]} --thread {threads}"\nthreads = 4\n[rules.environment]\nconda = "envs/qc.yaml"\n\n[[rules]]\nname = "bwa_align"\ninput = ["{config.out}/trimmed_R1.fq.gz", "{config.out}/trimmed_R2.fq.gz"]\noutput = ["{config.out}/aligned.sam"]\nshell = "bwa-mem2 mem -t {threads} {config.ref} {input[0]} {input[1]} > {output[0]}"\nthreads = 8\nmemory = "16G"\ncheckpoint = true\n[rules.environment]\nconda = "envs/alignment.yaml"\n\n[[rules]]\nname = "call_variants"\ninput = ["{config.out}/aligned.sam"]\noutput = ["{config.out}/variants.vcf.gz"]\nshell = "samtools sort -@ 4 {input[0]} | bcftools mpileup -f {config.ref} - | bcftools call -mv -Oz -o {output[0]}"\nthreads = 4\n[rules.environment]\nconda = "envs/variant_calling.yaml"\n',
  rnaseq: '[workflow]\nname = "rnaseq-quantification"\nversion = "1.0.0"\ndescription = "RNA-seq quantification pipeline"\n\n[config]\ndata = "/path/to/fastq"\nout = "results"\nsample = "SAMPLE01"\n\n[defaults]\nthreads = 4\nmemory = "8G"\n\n[[rules]]\nname = "fastp_trim"\ninput = ["{config.data}/{config.sample}_R1.fastq.gz", "{config.data}/{config.sample}_R2.fastq.gz"]\noutput = ["{config.out}/trimmed_R1.fq.gz", "{config.out}/trimmed_R2.fq.gz"]\nshell = "mkdir -p {config.out}\\nfastp -i {input[0]} -I {input[1]} -o {output[0]} -O {output[1]} --thread {threads}"\nthreads = 4\n[rules.environment]\nconda = "envs/qc.yaml"\n\n[[rules]]\nname = "salmon_quant"\ninput = ["{config.out}/trimmed_R1.fq.gz", "{config.out}/trimmed_R2.fq.gz"]\noutput = ["{config.out}/quant.sf"]\nshell = "salmon quant -i /path/to/salmon_index -l A -1 {input[0]} -2 {input[1]} -o {config.out}/salmon -p {threads}\\ncp {config.out}/salmon/quant.sf {output[0]}"\nthreads = 8\nmemory = "16G"\n[rules.environment]\nconda = "envs/rnaseq.yaml"\n',
  paired: '[workflow]\nname = "somatic-tumor-normal"\nversion = "1.0.0"\ndescription = "Somatic variant calling with matched tumor-normal pairs"\n\n[config]\nref = "/path/to/reference.fa"\ndata = "/path/to/fastq"\nout = "results"\n\n[defaults]\nthreads = 4\nmemory = "8G"\n\n[[pairs]]\npair_id = "CASE001"\nexperiment = "TUMOR01"\ncontrol = "NORMAL01"\nexperiment_type = "lung"\n\n[[rules]]\nname = "trim_experiment"\ninput = ["{config.data}/{experiment}_R1.fastq.gz", "{config.data}/{experiment}_R2.fastq.gz"]\noutput = ["{config.out}/{pair_id}/trim_T_R1.fq.gz", "{config.out}/{pair_id}/trim_T_R2.fq.gz"]\nshell = "mkdir -p {config.out}/{pair_id}\\nfastp -i {input[0]} -I {input[1]} -o {output[0]} -O {output[1]} --thread {threads}"\nthreads = 4\n[rules.environment]\nconda = "envs/qc.yaml"\n\n[[rules]]\nname = "trim_control"\ninput = ["{config.data}/{control}_R1.fastq.gz", "{config.data}/{control}_R2.fastq.gz"]\noutput = ["{config.out}/{pair_id}/trim_N_R1.fq.gz", "{config.out}/{pair_id}/trim_N_R2.fq.gz"]\nshell = "fastp -i {input[0]} -I {input[1]} -o {output[0]} -O {output[1]} --thread {threads}"\nthreads = 4\n[rules.environment]\nconda = "envs/qc.yaml"\n\n[[rules]]\nname = "somatic_call"\ninput = ["{config.out}/{pair_id}/trim_T_R1.fq.gz", "{config.out}/{pair_id}/trim_N_R1.fq.gz"]\noutput = ["{config.out}/{pair_id}/somatic.vcf.gz"]\nshell = "bcftools mpileup -f {config.ref} {input[0]} {input[1]} | bcftools call -mv -Oz -o {output[0]}"\nthreads = 4\n[rules.environment]\nconda = "envs/variant_calling.yaml"\n',
  cohort: '[workflow]\nname = "cohort-analysis"\nversion = "1.0.0"\ndescription = "Joint variant calling across multiple samples"\n\n[config]\ndata = "/path/to/fastq"\nout = "results"\n\n[[sample_groups]]\nname = "control"\nsamples = ["CTRL01", "CTRL02", "CTRL03"]\n\n[[sample_groups]]\nname = "case"\nsamples = ["CASE01", "CASE02"]\n\n[[rules]]\nname = "qc_per_sample"\ninput = ["{config.data}/{sample}_R1.fastq.gz"]\noutput = ["{config.out}/{sample}/qc.html"]\nshell = "mkdir -p {config.out}/{sample}\\nfastqc {input[0]} -o {config.out}/{sample}"\nthreads = 2\n[rules.environment]\nconda = "envs/qc.yaml"\n\n[[rules]]\nname = "cohort_summary"\ninput = ["{config.out}/CTRL01/qc.html", "{config.out}/CTRL02/qc.html"]\noutput = ["{config.out}/summary.txt"]\nshell = "echo Cohort QC complete > {output[0]}"\n',
  scatter: '[workflow]\nname = "scatter-gather"\nversion = "1.0.0"\ndescription = "Scatter-gather parallel processing by chromosome"\n\n[config]\nref = "/path/to/reference.fa"\n\n[[rules]]\nname = "call_by_chromosome"\ninput = ["aligned.bam"]\noutput = ["variants.vcf.gz"]\nshell = "bcftools mpileup -f {config.ref} -r {_chrom} {input[0]} | bcftools call -mv -Oz -o {output[0]}"\nthreads = 4\n\n[rules.transform]\nmap = "bcftools mpileup -f {config.ref} -r {_chrom} {input[0]} | bcftools call -mv -Oz -o {output[0]}"\n\n[rules.transform.split]\nby = "_chrom"\nvalues = ["chr1","chr2","chr3","chr4","chr5","chr6","chr7","chr8","chr9","chr10","chr11","chr12","chr13","chr14","chr15","chr16","chr17","chr18","chr19","chr20","chr21","chr22","chrX","chrY"]\n\n[rules.transform.combine]\naggregate = true\nmethod = "concat"\n\n[rules.environment]\nconda = "envs/variant_calling.yaml"\n',
  conditional: '[workflow]\nname = "conditional-pipeline"\nversion = "1.0.0"\ndescription = "Conditional execution based on config flags"\n\n[config]\nrun_expensive_analysis = false\ndo_qc = true\nmin_quality = 30\n\n[[rules]]\nname = "basic_qc"\noutput = ["qc_report.txt"]\nshell = "echo QC Report > {output[0]}"\nwhen = "config.do_qc"\n\n[[rules]]\nname = "expensive_analysis"\ninput = ["qc_report.txt"]\noutput = ["deep_analysis.txt"]\nshell = "echo Deep analysis > {output[0]}"\nwhen = "config.run_expensive_analysis && config.min_quality >= 20"\n\n[[rules]]\nname = "always_run"\noutput = ["summary.txt"]\nshell = "echo Pipeline summary > {output[0]}"\n'
};

// Template metadata for the template library
var templateMeta = {
  hello: { name: 'Hello World', category: 'Basic', tags: ['demo', 'simple'], description: 'Simple starter workflow demonstrating basic rule syntax' },
  wgs: { name: 'WGS Germline', category: 'Genomics', tags: ['wgs', 'variant-calling', 'germline'], description: 'Whole-genome sequencing germline variant calling pipeline (fastp → bwa-mem2 → bcftools)' },
  rnaseq: { name: 'RNA-seq', category: 'Genomics', tags: ['rnaseq', 'quantification', 'salmon'], description: 'RNA-seq quantification pipeline with fastp trimming and Salmon alignment-free quantification' },
  paired: { name: 'Tumor-Normal', category: 'Genomics', tags: ['somatic', 'paired', 'tumor-normal'], description: 'Somatic variant calling for tumor-normal paired analysis with paired samples feature' },
  cohort: { name: 'Multi-Sample Cohort', category: 'Genomics', tags: ['cohort', 'multi-sample', 'joint-calling'], description: 'Multi-sample cohort analysis with sample groups and per-sample QC aggregation' },
  scatter: { name: 'Scatter-Gather', category: 'Advanced', tags: ['parallel', 'scatter-gather', 'transform'], description: 'Parallel processing example using transform.split for chromosome-wise variant calling' },
  conditional: { name: 'Conditional Rules', category: 'Advanced', tags: ['conditional', 'when', 'logic'], description: 'Demonstrates conditional rule execution using when expressions' }
};

function newWorkflow() {
  document.getElementById('editor-text').value = '';
  document.getElementById('save-name').value = '';
  document.getElementById('editor-output').classList.add('hidden');
  document.querySelectorAll('.sidebar-nav button').forEach(function(b) { b.classList.remove('active'); });
  document.querySelector('[data-view="editor"]').classList.add('active');
  document.querySelectorAll('.view').forEach(function(v) { v.classList.remove('active'); });
  document.getElementById('view-editor').classList.add('active');
  document.getElementById('view-title').textContent = 'Workflow Editor';
  document.getElementById('editor-text').focus();
}

function loadTemplate() {
  var sel = document.getElementById('template-select').value;
  if (sel && templates[sel]) {
    document.getElementById('editor-text').value = templates[sel];
    document.getElementById('save-name').value = sel;
    document.getElementById('template-select').value = '';
  }
}

function loadTemplateById(templateId) {
  if (templates[templateId]) {
    document.getElementById('editor-text').value = templates[templateId];
    document.getElementById('save-name').value = templateMeta[templateId].name;
    // Navigate to editor
    document.querySelectorAll('.sidebar-nav button').forEach(function(b) { b.classList.remove('active'); });
    document.querySelector('[data-view="editor"]').classList.add('active');
    document.querySelectorAll('.view').forEach(function(v) { v.classList.remove('active'); });
    document.getElementById('view-editor').classList.add('active');
    document.getElementById('view-title').textContent = 'Workflow Editor';
  }
}

function showTemplatePreview(templateId) {
  currentPreviewTemplate = templateId;
  var meta = templateMeta[templateId];
  var content = templates[templateId];
  document.getElementById('template-preview-title').textContent = meta.name;
  document.getElementById('template-preview-desc').textContent = meta.description;
  document.getElementById('template-preview-category').textContent = meta.category;
  document.getElementById('template-preview-tags').textContent = meta.tags.join(', ');
  document.getElementById('template-preview-content').textContent = content;
  document.getElementById('template-preview-modal').classList.remove('hidden');
}

function closeTemplatePreview() {
  document.getElementById('template-preview-modal').classList.add('hidden');
}

function renderTemplatesGrid() {
  var grid = document.getElementById('templates-grid');
  grid.innerHTML = '';
  Object.keys(templateMeta).forEach(function(id) {
    var meta = templateMeta[id];
    var card = document.createElement('div');
    card.className = 'template-card';
    card.style.cssText = 'background:var(--surface);border:1px solid var(--border);border-radius:var(--radius);padding:1rem;';
    card.innerHTML = '<div style="font-weight:600;font-size:0.9rem;margin-bottom:0.3rem">' + esc(meta.name) + '</div>' +
      '<div style="font-size:0.75rem;color:var(--accent);margin-bottom:0.5rem">' + esc(meta.category) + '</div>' +
      '<div style="font-size:0.78rem;color:var(--text2);margin-bottom:0.75rem">' + esc(meta.description) + '</div>' +
      '<div style="font-size:0.7rem;color:var(--text3);margin-bottom:0.75rem">Tags: ' + meta.tags.map(function(t) { return esc(t); }).join(', ') + '</div>' +
      '<button class="btn btn-sm btn-outline" onclick="showTemplatePreview(\'' + id + '\')" style="margin-right:0.25rem">Preview</button>' +
      '<button class="btn btn-sm btn-primary" onclick="loadTemplateById(\'' + id + '\')">Use</button>';
    grid.appendChild(card);
  });
}

function filterTemplates() {
  var search = document.getElementById('template-search').value.toLowerCase();
  var grid = document.getElementById('templates-grid');
  grid.innerHTML = '';
  Object.keys(templateMeta).forEach(function(id) {
    var meta = templateMeta[id];
    var matches = !search ||
      meta.name.toLowerCase().includes(search) ||
      meta.category.toLowerCase().includes(search) ||
      meta.description.toLowerCase().includes(search) ||
      meta.tags.some(function(t) { return t.toLowerCase().includes(search); });
    if (matches) {
      var card = document.createElement('div');
      card.className = 'template-card';
      card.style.cssText = 'background:var(--surface);border:1px solid var(--border);border-radius:var(--radius);padding:1rem;';
      card.innerHTML = '<div style="font-weight:600;font-size:0.9rem;margin-bottom:0.3rem">' + esc(meta.name) + '</div>' +
        '<div style="font-size:0.75rem;color:var(--accent);margin-bottom:0.5rem">' + esc(meta.category) + '</div>' +
        '<div style="font-size:0.78rem;color:var(--text2);margin-bottom:0.75rem">' + esc(meta.description) + '</div>' +
        '<div style="font-size:0.7rem;color:var(--text3);margin-bottom:0.75rem">Tags: ' + meta.tags.map(function(t) { return esc(t); }).join(', ') + '</div>' +
        '<button class="btn btn-sm btn-outline" onclick="showTemplatePreview(\'' + id + '\')" style="margin-right:0.25rem">Preview</button>' +
        '<button class="btn btn-sm btn-primary" onclick="loadTemplateById(\'' + id + '\')">Use</button>';
      grid.appendChild(card);
    }
  });
}

// Initialize template grid when templates view is shown
document.querySelector('[data-view="templates"]').addEventListener('click', function() {
  renderTemplatesGrid();
});

async function loadWorkflowToEditor(wfId) {
  try {
    var r = await api('GET', '/api/workflows/saved/' + wfId);
    if (r.status === 200 && r.data.toml_content) {
      document.getElementById('editor-text').value = r.data.toml_content;
      // Navigate to editor
      document.querySelectorAll('.sidebar-nav button').forEach(function(b) { b.classList.remove('active'); });
      document.querySelector('[data-view="editor"]').classList.add('active');
      document.querySelectorAll('.view').forEach(function(v) { v.classList.remove('active'); });
      document.getElementById('view-editor').classList.add('active');
      document.getElementById('view-title').textContent = 'Workflow Editor';
      document.getElementById('save-name').value = r.data.name;
      document.getElementById('save-version').value = r.data.version;
      currentEditingWfId = wfId; // Enable upsert on save
      showOutput('Loaded: ' + r.data.name + ' (v' + r.data.version + ', ' + r.data.rules_count + ' rules)', false);
    } else {
      alert('Failed to load workflow: ' + (r.data.error || 'Not found'));
    }
  } catch(e) { alert('Error loading workflow'); }
}

async function refreshSystem() {
  try {
    var r = await api('GET', '/api/metrics');
    if (r.data) {
      document.getElementById('s-cpu').textContent = (r.data.cpu_count||'?') + ' cores';
      document.getElementById('s-mem').textContent = ((r.data.host?.total_memory_mb||0)/1024).toFixed(0) + ' GB';
      document.getElementById('s-swap').textContent = ((r.data.host?.used_swap_mb||0)/1024).toFixed(1) + ' / ' + ((r.data.host?.total_swap_mb||0)/1024).toFixed(0) + ' GB';
      document.getElementById('s-requests').textContent = r.data.total_requests || 0;
    }
    var sys = await api('GET', '/api/system');
    document.getElementById('sys-info-json').textContent = JSON.stringify(sys.data, null, 2);
    var lic = await api('GET', '/api/license');
    var licJson = lic.data || {};
    var licHtml = '<div style="margin-bottom:0.5rem">';
    if (licJson.valid) {
      licHtml += '<span style="color:var(--success);font-weight:600">Valid</span> ';
      licHtml += '<span style="color:var(--text)">' + esc(licJson.license_type || 'unknown') + '</span>';
      if (licJson.issued_to) licHtml += ' <span style="font-size:0.75rem;color:var(--text3)"> - ' + esc(licJson.issued_to) + '</span>';
      licHtml += '</div>';
      if (licJson.license_type === 'academic') {
        licHtml += '<div style="background:rgba(251,191,36,0.1);border:1px solid rgba(251,191,36,0.3);border-radius:var(--radius);padding:0.5rem;font-size:0.75rem;color:var(--warning)">Commercial use requires a paid license. Contact sales for pricing.</div>';
      }
    } else {
      licHtml += '<span style="color:var(--error)">Invalid</span></div>';
      licHtml += '<div style="font-size:0.75rem;color:var(--text2)">' + esc(licJson.message || '') + '</div>';
    }
    document.getElementById('license-json').innerHTML = licHtml;
    refreshAuditLogs();
    refreshHpcStatus();
  } catch(e) {}
}

async function refreshAuditLogs() {
  try {
    var days = document.getElementById('audit-days').value;
    var r = await api('GET', '/api/audit?days=' + days);
    var tbody = document.getElementById('audit-tbody');
    tbody.innerHTML = '';
    if (r.data && r.data.entries) {
      r.data.entries.forEach(function(e) {
        var tr = document.createElement('tr');
        tr.innerHTML = '<td>' + fmtTime(e.timestamp) + '</td><td>' + esc(e.user) + '</td><td>' + esc(e.action) + '</td><td>' + esc(e.resource) + '</td>';
        tbody.appendChild(tr);
      });
      if (r.data.entries.length === 0) {
        tbody.innerHTML = '<tr><td colspan="4" style="text-align:center;color:var(--text2)">No audit logs found</td></tr>';
      }
    }
  } catch(e) {}
}

async function refreshHpcStatus() {
  try {
    var r = await api('GET', '/api/hpc');
    var el = document.getElementById('hpc-status-content');
    if (r.data && r.data.available) {
      var h = '<div style="margin-bottom:0.5rem"><span style="color:var(--success);font-weight:600">' + esc(r.data.scheduler) + '</span>' +
        (r.data.version ? ' <span style="font-size:0.75rem;color:var(--text3)">' + esc(r.data.version) + '</span>' : '') +
        ' | <span style="color:var(--text)">' + r.data.total_jobs + ' jobs</span></div>';
      if (r.data.queues && r.data.queues.length > 0) {
        h += '<div style="margin-bottom:0.5rem"><strong>Queues:</strong></div>';
        r.data.queues.forEach(function(q) {
          h += '<div style="font-size:0.75rem;margin-bottom:0.2rem">' + esc(q.queue_name) + ': ' +
            q.total_jobs + ' total / ' + q.running + ' running / ' + q.pending + ' pending</div>';
        });
      }
      if (r.data.nodes && r.data.nodes.length > 0) {
        h += '<div style="margin-top:0.5rem"><strong>Nodes:</strong> ' + r.data.nodes.length + ' total</div>';
        r.data.nodes.slice(0, 5).forEach(function(n) {
          h += '<div style="font-size:0.75rem;margin-bottom:0.2rem">' + esc(n.name) + ': <span class="badge badge-' +
            (n.state === 'idle' ? 'success' : n.state === 'allocated' ? 'running' : 'pending') + '">' + esc(n.state) + '</span> ' +
            n.cpus_free + '/' + n.cpus_total + ' CPUs</div>';
        });
        if (r.data.nodes.length > 5) {
          h += '<div style="font-size:0.75rem;color:var(--text3)">... and ' + (r.data.nodes.length - 5) + ' more</div>';
        }
      }
      el.innerHTML = h;
    } else {
      var msg = r.data && r.data.error ? r.data.error : 'No HPC scheduler detected';
      el.innerHTML = '<div style="color:var(--text3)">' + esc(msg) + '</div>';
    }
  } catch(e) { document.getElementById('hpc-status-content').innerHTML = '<div style="color:var(--text3)">HPC status unavailable</div>'; }
}

async function refreshScheduledRuns() {
  try {
    // Load saved workflows into schedule select
    var wfR = await api('GET', '/api/workflows/saved');
    var sel = document.getElementById('schedule-wf-select');
    sel.innerHTML = '<option value="">-- Select workflow --</option>';
    if (Array.isArray(wfR.data)) {
      wfR.data.forEach(function(w) {
        sel.innerHTML += '<option value="' + w.id + '">' + esc(w.name) + ' (v' + esc(w.version) + ')</option>';
      });
    }

    // Load scheduled runs
    var r = await api('GET', '/api/scheduled');
    var tbody = document.getElementById('scheduled-tbody');
    tbody.innerHTML = '';
    if (Array.isArray(r.data)) {
      r.data.forEach(function(s) {
        var badge = s.status === 'active' ? '<span class="badge badge-success">Active</span>' : '<span class="badge badge-cancelled">Cancelled</span>';
        var tr = document.createElement('tr');
        tr.innerHTML = '<td>' + esc(s.workflow_name) + '</td><td>' + esc(s.cron_expression) + '</td>' +
          '<td style="font-size:0.78rem;color:var(--text2)">' + fmtTime(s.next_run_at) + '</td>' +
          '<td style="font-size:0.78rem;color:var(--text2)">' + (s.last_run_at ? fmtTime(s.last_run_at) : '--') + '</td>' +
          '<td>' + badge + '</td>' +
          '<td><button class="btn btn-sm btn-danger" onclick="cancelSchedule(\'' + s.id + '\')">Cancel</button></td>';
        tbody.appendChild(tr);
      });
      if (r.data.length === 0) {
        tbody.innerHTML = '<tr><td colspan="6" style="text-align:center;color:var(--text2)">No scheduled runs</td></tr>';
      }
    }
  } catch(e) {}
}

async function createSchedule() {
  var wfId = document.getElementById('schedule-wf-select').value;
  var cron = document.getElementById('schedule-cron').value;
  if (!wfId) { alert('Select a workflow first'); return; }
  try {
    var r = await api('POST', '/api/scheduled', { workflow_id: wfId, cron_expression: cron });
    if (r.status === 200) {
      refreshScheduledRuns();
    } else {
      alert('Schedule failed: ' + (r.data.error || r.status));
    }
  } catch(e) { alert('Error scheduling workflow'); }
}

async function cancelSchedule(scheduleId) {
  if (!confirm('Cancel this scheduled run?')) return;
  try {
    var r = await api('DELETE', '/api/scheduled/' + scheduleId);
    if (r.status === 200) {
      refreshScheduledRuns();
    } else {
      alert('Cancel failed: ' + (r.data.error || r.status));
    }
  } catch(e) { alert('Error cancelling schedule'); }
}

function esc(s) { var d = document.createElement('div'); d.textContent = s; return d.innerHTML; }
function jsStr(s) { return (s||'').replace(/\\\\/g,'\\\\\\\\').replace(/'/g,"\\\\'").replace(/"/g,'\\\\"').replace(/\\n/g,'\\\\n').replace(/\\r/g,'\\\\r'); }
function fmtTime(t) { try { return new Date(t).toLocaleString(); } catch(e) { return t; } }
function fmtDur(s) {
  if (!s || s < 0) return '--';
  var h = Math.floor(s/3600), m = Math.floor((s%3600)/60), sec = Math.floor(s%60);
  return h>0 ? h+'h '+m+'m' : m>0 ? m+'m '+sec+'s' : sec+'s';
}

// --- New: Parse Workflow ---
async function parseWorkflow() {
  var r = await api('POST', '/api/workflows/parse', { toml_content: getToml() });
  var e = document.getElementById('editor-output');
  e.classList.remove('hidden','error','success');
  if (r.status === 200 && r.data) {
    e.textContent = 'Name: ' + (r.data.name||'') + '\nVersion: ' + (r.data.version||'') +
      '\nDescription: ' + (r.data.description||'N/A') + '\nAuthor: ' + (r.data.author||'N/A') +
      '\n\nRules:\n' + (r.data.rules||[]).map(function(r){return '  - '+r.name+' ('+r.threads+' threads, '+r.environment+')';}).join('\n');
    e.classList.add('success');
  } else { e.textContent = 'Parse failed'; e.classList.add('error'); }
}

// --- New: Diff ---
function showDiff() {
  document.getElementById('diff-ta').value = getToml();
  document.getElementById('diff-tb').value = '';
  document.getElementById('diff-result').textContent = '';
  document.getElementById('diff-modal').classList.remove('hidden');
}
function closeDiffModal() { document.getElementById('diff-modal').classList.add('hidden'); }
async function doDiff() {
  var a = document.getElementById('diff-ta').value;
  var b = document.getElementById('diff-tb').value;
  var r = await api('POST', '/api/workflows/diff', { toml_a: a, toml_b: b });
  if (r.data && r.data.diffs) {
    document.getElementById('diff-result').textContent = r.data.diff_count + ' differences:\n' +
      r.data.diffs.map(function(d){return '['+d.category+'] '+d.description;}).join('\n');
  } else { document.getElementById('diff-result').textContent = 'Diff failed: ' + (r.data.error||''); }
}

// --- New: Report ---
function closeReportModal() { document.getElementById('report-modal').classList.add('hidden'); }
async function generateReport() {
  var r = await api('POST', '/api/reports/generate', { toml_content: getToml(), format: 'html' });
  document.getElementById('report-content').innerHTML = typeof r.data === 'string' ? r.data : '<pre>'+JSON.stringify(r.data,null,2)+'</pre>';
  document.getElementById('report-modal').classList.remove('hidden');
}

// --- New: List Outputs ---
async function listOutputs() {
  var r = await api('POST', '/api/workflows/clean', { toml_content: getToml() });
  if (r.data && r.data.files_to_clean) {
    showOutput('Files to clean (' + r.data.total_files + '):\n' + r.data.files_to_clean.join('\n'), false);
  } else { showOutput('No output files detected', false); }
}

// --- New: Singularity Export ---
async function exportWorkflowSingularity() {
  var r = await api('POST', '/api/workflows/export', { toml_content: getToml(), format: 'singularity' });
  if (r.status === 200 && r.data.content) {
    document.getElementById('export-content').textContent = r.data.content;
    document.getElementById('export-modal').classList.remove('hidden');
  } else { showOutput('Export failed: ' + (r.data.error || ''), true); }
}

// --- Updated: Template CRUD from API ---
async function loadTemplates() {
  try {
    var r = await api('GET', '/api/templates');
    var grid = document.getElementById('templates-grid');
    grid.innerHTML = '';
    if (Array.isArray(r.data)) {
      r.data.forEach(function(t) {
        var tags = (t.tags||'').split(',').filter(Boolean);
        var card = document.createElement('div');
        card.className = 'template-card';
        card.style.cssText = 'background:var(--surface);border:1px solid var(--border);border-radius:var(--radius);padding:1rem;';
        card.innerHTML = '<div style="font-weight:600;font-size:0.9rem;margin-bottom:0.3rem">'+esc(t.name)+(t.is_system?' <span style="font-size:0.6rem;color:var(--text3)">[system]</span>':'')+'</div>'+
          '<div style="font-size:0.75rem;color:var(--accent);margin-bottom:0.5rem">'+esc(t.category)+'</div>'+
          '<div style="font-size:0.78rem;color:var(--text2);margin-bottom:0.75rem">'+esc(t.description)+'</div>'+
          '<div style="font-size:0.7rem;color:var(--text3);margin-bottom:0.75rem">Tags: '+(tags.length?tags.join(', '):'none')+'</div>'+
          '<button class="btn btn-sm btn-outline" onclick="previewTemplate(\''+t.id+'\')">Preview</button> '+
          '<button class="btn btn-sm btn-primary" onclick="useTemplate(\''+t.id+'\')">Use</button>'+
          (t.is_system ? '' : ' <button class="btn btn-sm btn-danger" onclick="deleteTemplate(\''+t.id+'\',\''+jsStr(t.name)+'\')">Del</button>');
        grid.appendChild(card);
      });
      if (r.data.length === 0) grid.innerHTML = '<div style="color:var(--text3);padding:2rem;text-align:center">No templates</div>';
    }
    // Also update quick template dropdown
    updateTemplateDropdown(r.data);
  } catch(e) {}
}

function showCreateTemplate() { document.getElementById('template-create-modal').classList.remove('hidden'); }
function closeCreateTemplate() { document.getElementById('template-create-modal').classList.add('hidden'); }
async function doCreateTemplate() {
  var name = document.getElementById('tpl-name').value.trim();
  var content = document.getElementById('tpl-content').value.trim();
  if (!name || !content) { alert('Name and TOML content required'); return; }
  var req = { name: name, toml_content: content,
    category: document.getElementById('tpl-category').value.trim() || 'general',
    description: document.getElementById('tpl-desc').value.trim() || '',
    tags: document.getElementById('tpl-tags').value.trim() || ''
  };
  var r = await api('POST', '/api/templates', req);
  if (r.status === 201) { closeCreateTemplate(); loadTemplates(); } else alert('Failed: '+(r.data.error||''));
}
async function deleteTemplate(tplId, name) {
  if (!confirm('Delete template "'+name+'"?')) return;
  var r = await api('DELETE', '/api/templates/'+tplId);
  if (r.status === 200) loadTemplates(); else alert('Delete failed: '+(r.data.error||''));
}

function updateTemplateDropdown(templates) {
  var sel = document.getElementById('template-select');
  if (!sel) return;
  var cur = sel.value;
  sel.innerHTML = '<option value="">-- Quick template --</option>';
  if (Array.isArray(templates)) {
    templates.forEach(function(t) {
      sel.innerHTML += '<option value="tpl:'+t.id+'">'+esc(t.name)+'</option>';
    });
  }
  sel.value = cur;
}

async function previewTemplate(id) {
  var r = await api('GET', '/api/templates/' + id);
  if (r.data) {
    currentPreviewTemplate = id;
    document.getElementById('template-preview-title').textContent = r.data.name;
    document.getElementById('template-preview-desc').textContent = r.data.description;
    document.getElementById('template-preview-category').textContent = r.data.category;
    document.getElementById('template-preview-tags').textContent = r.data.tags || 'none';
    document.getElementById('template-preview-content').textContent = r.data.toml_content;
    document.getElementById('template-preview-modal').classList.remove('hidden');
  }
}

async function useTemplate(id) {
  var r = await api('GET', '/api/templates/' + id);
  if (r.data && r.data.toml_content) {
    document.getElementById('editor-text').value = r.data.toml_content;
    document.getElementById('save-name').value = r.data.name;
    navigateTo('editor');
  }
}

// Hook old loadTemplate to handle template IDs from API
var origLoadTemplate = loadTemplate;
loadTemplate = function() {
  var sel = document.getElementById('template-select').value;
  if (sel && sel.startsWith('tpl:')) {
    useTemplate(sel.substring(4));
    document.getElementById('template-select').value = '';
  } else if (sel && templates[sel]) {
    origLoadTemplate();
  }
};

// --- Users ---
async function refreshUsers() {
  var r = await api('GET', '/api/users');
  var tbody = document.getElementById('users-tbody');
  if (Array.isArray(r.data)) {
    tbody.innerHTML = r.data.map(function(u) {
      return '<tr><td>'+esc(u.username)+'</td><td>'+esc(u.role)+'</td><td>'+esc(u.auth_type)+'</td>'+
        '<td style="font-size:0.78rem;color:var(--text2)">'+fmtTime(u.created_at)+'</td>'+
        '<td><button class="btn btn-sm btn-danger" onclick="deleteUser(\''+u.id+'\',\''+jsStr(u.username)+'\')">Del</button></td></tr>';
    }).join('');
    if (r.data.length === 0) tbody.innerHTML = '<tr><td colspan="5" style="color:var(--text3)">No users</td></tr>';
  }
}
function showAddUser() { document.getElementById('user-add-modal').classList.remove('hidden'); }
function closeAddUser() { document.getElementById('user-add-modal').classList.add('hidden'); }
async function doAddUser() {
  var uname = document.getElementById('new-username').value.trim();
  var role = document.getElementById('new-role').value;
  var pass = document.getElementById('new-password').value;
  if (!uname || !pass) { alert('Username and password required'); return; }
  var r = await api('POST', '/api/users', { username: uname, role: role, password: pass });
  if (r.status === 201) { closeAddUser(); refreshUsers(); }
  else { alert('Failed: ' + (r.data.error||r.status)); }
}
async function deleteUser(uid, name) {
  if (!confirm('Delete user "'+name+'"?')) return;
  var r = await api('DELETE', '/api/users/'+uid);
  if (r.status === 200) refreshUsers();
  else alert('Delete failed: '+(r.data.error||''));
}

// --- HPC Submit ---
function showHpcSubmit(runId) { currentHpcRunId = runId; document.getElementById('hpc-modal').classList.remove('hidden'); }
function closeHpcModal() { document.getElementById('hpc-modal').classList.add('hidden'); }
async function doHpcSubmit() {
  var sched = document.getElementById('hpc-scheduler').value;
  var req = { scheduler: sched };
  var partition = document.getElementById('hpc-partition').value.trim();
  if (partition) req.partition = partition;
  var cpus = parseInt(document.getElementById('hpc-cpus').value) || 4;
  req.cpus = cpus;
  var mem = document.getElementById('hpc-mem').value.trim();
  if (mem) req.memory = mem;
  var r = await api('POST', '/api/runs/'+currentHpcRunId+'/hpc-submit', req);
  var el = document.getElementById('hpc-result');
  if (r.status === 200) {
    el.innerHTML = '<div style="color:var(--success)">Submitted! HPC Job ID: '+esc(r.data.hpc_job_id)+'</div>';
    setTimeout(closeHpcModal, 2000);
  } else { el.innerHTML = '<div style="color:var(--error)">Failed: '+esc(r.data.error||'')+'</div>'; }
}

// --- Enhanced Log ---
var logFullText = '';
function filterLogs() {
  var q = document.getElementById('log-search').value.toLowerCase();
  var el = document.getElementById('log-content');
  if (!q) { el.textContent = logFullText; return; }
  el.textContent = logFullText.split('\n').filter(function(l){return l.toLowerCase().indexOf(q)>=0;}).join('\n');
}
function toggleLogAutoRefresh() {
  if (document.getElementById('log-autorefresh').checked) {
    logAutoRefreshTimer = setInterval(function(){ if (currentLogRunId) viewRunLogs(currentLogRunId); }, 3000);
  } else { clearInterval(logAutoRefreshTimer); }
}
function downloadLog() {
  var blob = new Blob([logFullText], {type:'text/plain'});
  var a = document.createElement('a'); a.href = URL.createObjectURL(blob);
  a.download = 'execution_'+currentLogRunId+'.log'; a.click();
}
// Override viewRunLogs to support new features
var origViewRunLogs = viewRunLogs;
viewRunLogs = function(runId) {
  currentLogRunId = runId;
  origViewRunLogs(runId);
  // Also store full text for filter/download
  api('GET', '/api/runs/'+runId+'/logs').then(function(r){
    if (typeof r.data === 'string') logFullText = r.data;
  });
};

// --- HPC button in runs table ---
// Override refreshRuns to add HPC submit button
var origRefreshRuns = refreshRuns;
refreshRuns = function() {
  origRefreshRuns();
  // After rendering, add HPC submit buttons
  setTimeout(function() {
    document.querySelectorAll('#all-runs-tbody tr').forEach(function(row) {
      var actions = row.querySelector('td:last-child');
      if (actions && actions.innerHTML.indexOf('HPC') < 0) {
        var runIdEl = row.querySelector('td:first-child');
        if (runIdEl) {
          var rid = runIdEl.textContent.replace('...','').trim();
          if (rid.length > 8) {
            actions.innerHTML += ' <button class="btn btn-sm btn-outline" onclick="showHpcSubmit(\''+rid+'\')">HPC</button>';
          }
        }
      }
    });
  }, 200);
};

// --- Environments ---
function refreshEnvironments() {
  api('GET', '/api/environments').then(function(r) {
    var el = document.getElementById('env-list');
    if (r.data && r.data.available) {
      el.innerHTML = r.data.available.map(function(e){return '<span class="badge badge-running">'+esc(e)+'</span>';}).join(' ');
    }
  });
}
// Call on page load
document.addEventListener('DOMContentLoaded', function() { refreshEnvironments(); });

// --- Navigation helper ---
function navigateTo(view) {
  document.querySelectorAll('.sidebar-nav button').forEach(function(b) { b.classList.remove('active'); });
  var btn = document.querySelector('[data-view="'+view+'"]');
  if (btn) btn.classList.add('active');
  document.querySelectorAll('.view').forEach(function(v) { v.classList.remove('active'); });
  document.getElementById('view-'+view).classList.add('active');
  document.getElementById('view-title').textContent = btn ? btn.textContent.trim().replace(/^\S+\s*/, '') : view;
}
// Clear currentEditingWfId when starting new workflow
var origNewWorkflow = newWorkflow;
newWorkflow = function() { currentEditingWfId = null; origNewWorkflow(); };

checkSession();
refreshDashboard();
updateTopbar();
setInterval(refreshDashboard, 30000);
// SSE-based live event stream for run status updates
try {
  var evtSource = new EventSource(API + '/api/events');
  evtSource.onmessage = function(e) {
    try {
      var msg = JSON.parse(e.data);
      if (msg.type && (msg.type === 'run_cancelled' || msg.type === 'workflow_completed' || msg.type === 'workflow_started')) {
        refreshRuns();
      }
    } catch(ex) {}
  };
  evtSource.onerror = function() { /* silently retry */ };
} catch(ex) {}
document.getElementById('avatar').addEventListener('click', function() { if (user) { if (confirm('Logout?')) { token=''; user=null; document.getElementById('user-name').textContent='Guest'; document.getElementById('avatar').textContent='G'; } } else { showLogin(); } });
