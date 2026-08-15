import { useState, useEffect, useCallback, useRef } from 'react';
import { useParams, useNavigate } from 'react-router-dom';
import { api, createEventSource } from '../api/client';
import type { RunItem, MonitorStatus, ReportData, DagStatus, Diagnostics, DryRunPreview, RunInstance } from '../api/types';
import { Play, Pause, RotateCcw, BarChart3, Loader2, Bot, Ban, Trash2, StepForward } from 'lucide-react';
import WorkflowCanvas from '../components/WorkflowCanvas';
import { usePipelineSession } from '../context/PipelineSession';

type TabType = 'monitor' | 'report' | 'diagnostics' | 'dag' | 'logs' | 'instances';

function StatCard({ value, label, color }: { value: string; label: string; color?: string }) {
  return (
    <div className="stat-card">
      <div className="stat-value" style={{ color: color || 'var(--color-text)' }}>{value}</div>
      <div className="stat-label">{label}</div>
    </div>
  );
}

export default function MonitorReport() {
  const session = usePipelineSession();
  const [runs, setRuns] = useState<RunItem[]>([]);
  const [selId, setSelId] = useState<string | null>(null);
  const [monitorStatus, setMonitorStatus] = useState<MonitorStatus | null>(null);
  const [reportData, setReportData] = useState<ReportData | null>(null);
  const [dagStatus, setDagStatus] = useState<DagStatus | null>(null);
  const [diagnostics, setDiagnostics] = useState<Diagnostics | null>(null);
  const [preview, setPreview] = useState<DryRunPreview | null>(null);
  const [tab, setTab] = useState<TabType>('monitor');
  const [logs, setLogs] = useState<string | null>(null);
  const [logQuery, setLogQuery] = useState('');
  const [logRule, setLogRule] = useState('');
  const [instances, setInstances] = useState<RunInstance[] | null>(null);
  const [onlyFailed, setOnlyFailed] = useState(false);
  const [explainState, setExplainState] = useState<Record<string, { loading?: boolean; text?: string }>>({});
  const [, setLoading] = useState(true);
  // Pagination: the full list is ≤100 rows (API LIMIT); render a page of
  // 20 so the Run Detail card below is not buried under 5700px of history
  // (issue #79 P2). "Show more" grows the page.
  const [visibleCount, setVisibleCount] = useState(20);
  const detailRef = useRef<HTMLDivElement>(null);
  const [qaInput, setQaInput] = useState('');
  const [qaAnswer, setQaAnswer] = useState<string | null>(null);

  const { id: routeId } = useParams();
  const navigate = useNavigate();


  const [, setAlertOpen] = useState<string[]>([]);
  const selectRun = useCallback(async (id: string) => {
    setSelId(id);
    setTab('monitor');
    setQaAnswer(null);
    session.setActiveRunId(id);
    session.setChatContext('monitor');
    try { setMonitorStatus(await api.aiStatus(id)); } catch { setMonitorStatus(null); }
    try { setReportData(await api.runReport(id)); } catch { setReportData(null); }
    try { setDagStatus(await api.getDagStatus(id)); } catch { setDagStatus(null); }
    try { setDiagnostics(await api.getDiagnostics(id)); } catch { setDiagnostics(null); }
    try { setPreview(await api.getRunPreview(id)); } catch { setPreview(null); }
    // A row click means the user wants the detail — bring it into view
    // instead of leaving it below a long list (issue #79 P2).
    requestAnimationFrame(() => detailRef.current?.scrollIntoView({ behavior: 'smooth', block: 'start' }));
  }, []);

  useEffect(() => {
    if (routeId && routeId !== selId) {
      // Defer to a macrotask: selectRun triggers several state updates and
      // must not run synchronously inside the effect body.
      const timer = setTimeout(() => selectRun(routeId), 0);
      return () => clearTimeout(timer);
    }
  }, [routeId, selectRun, selId]);

  const toggleAlert = (idx: string) => {
    setAlertOpen(prev => prev.includes(idx) ? prev.filter(x => x !== idx) : [...prev, idx]);
  };

  useEffect(() => {
    api.listRuns().then((r) => { setRuns(r.items); setLoading(false); }).catch(() => setLoading(false));
  }, []);

  // Update monitor status in real-time via SSE
  useEffect(() => {
    if (!selId) return;
    const es = createEventSource();
    const interval = setInterval(async () => {
      try {
        const status = await api.aiStatus(selId);
        setMonitorStatus(status);
      } catch { /* ignore */ }
    }, 5000);

    es.onmessage = (evt) => {
      try {
        const event = JSON.parse(evt.data);
        // Events are scoped to their owning user (issue #82 P0-5); the
        // server already filters the stream, this is a belt-and-suspenders
        // guard for anonymous/personal-mode streams.
        const mine = !event.user || event.user === localStorage.getItem('oxo_user_id');
        if (mine && event.data?.run_id === selId) {
          if (event.type === 'run_completed' || event.type === 'run_failed') {
            clearInterval(interval);
            api.listRuns().then((r) => setRuns(r.items));
          }
        }
      } catch { /* ignore */ }
    };
    return () => { clearInterval(interval); es.close(); };
  }, [selId]);



  const handlePause = async () => {
    if (!selId) return;
    try {
      await api.pauseRun(selId, 'user_request');
      const s = await api.aiStatus(selId);
      setMonitorStatus(s);
    } catch { /* ignore */ }
  };

  const handleResume = async () => {
    if (!selId) return;
    try {
      await api.resumeRun(selId);
      const s = await api.aiStatus(selId);
      setMonitorStatus(s);
    } catch { /* ignore */ }
  };

  const handleRetry = async () => {
    if (!selId) return;
    try {
      const plan = await api.retryRun(selId);
      if (plan.new_run_id) setSelId(plan.new_run_id);
    } catch { /* ignore */ }
  };

  const handleCancel = async () => {
    if (!selId) return;
    if (!window.confirm('Cancel this run? Its processes will be terminated.')) return;
    try {
      await api.cancelRun(selId);
      api.listRuns().then((r) => setRuns(r.items));
      const s = await api.aiStatus(selId);
      setMonitorStatus(s);
    } catch { /* ignore */ }
  };

  const handleAsk = async () => {
    if (!qaInput.trim() || !selId) return;
    try {
      const answer = await api.askReport(selId, qaInput);
      setQaAnswer(answer);
    } catch {
      setQaAnswer('Sorry, I could not answer that question. Please try rephrasing.');
    }
  };

  // ── Logs ── (issue #82 P0-7: the raw execution.log with search,
  // per-rule filtering, failure highlighting, and download)
  useEffect(() => {
    if (tab === 'logs' && selId && logs === null) {
      api.getRunLogs(selId).then(setLogs).catch(() => setLogs(''));
    }
  }, [tab, selId, logs]);

  const ruleNames = (dagStatus?.nodes ?? []).map(n => n.label);
  const logSections = (logs ?? '').split(/(?=Running: )/);
  const filteredSections = logSections.filter(sec => {
    const head = sec.split('\n')[0];
    const matchesRule = !logRule || head.includes(logRule);
    const matchesQuery = !logQuery || sec.toLowerCase().includes(logQuery.toLowerCase());
    return matchesRule && matchesQuery;
  });

  const downloadLogs = () => {
    const blob = new Blob([logs ?? ''], { type: 'text/plain' });
    const url = URL.createObjectURL(blob);
    const a = document.createElement('a');
    a.href = url; a.download = `execution-${(selId ?? 'run').slice(0, 8)}.log`; a.click();
    URL.revokeObjectURL(url);
  };

  const renderLogs = () => (
    <div>
      <div style={{ display: 'flex', gap: '0.5rem', marginBottom: '0.75rem', flexWrap: 'wrap', alignItems: 'center' }}>
        <input className="search-input" placeholder="Search logs…" value={logQuery}
          onChange={e => setLogQuery(e.target.value)} style={{ flex: 1, minWidth: 180 }} />
        <select value={logRule} onChange={e => setLogRule(e.target.value)}
          className="search-input" style={{ minWidth: 140 }} aria-label="Filter by rule">
          <option value="">All rules</option>
          {ruleNames.map(r => <option key={r} value={r}>{r}</option>)}
        </select>
        <button className="btn-sm" onClick={downloadLogs} title="Download full log">⬇ Download</button>
        <button className="btn-sm" onClick={() => setLogs(null)} title="Reload">↻</button>
      </div>
      {logs === null ? (
        <div className="empty-state">Loading logs…</div>
      ) : logs === '' || filteredSections.length === 0 ? (
        <div className="empty-state">No matching log output.</div>
      ) : (
        <pre className="log-view">
          {filteredSections.map((sec, i) => {
            const failed = /✗|failed|Error:/.test(sec);
            return (
              <div key={i} className={failed ? 'log-line-failed' : undefined}>{sec.trimEnd()}</div>
            );
          })}
        </pre>
      )}
    </div>
  );

  // ── Instances ── (issue #82 P1-1: the sample×rule table answering
  // "which sample under which rule failed")
  useEffect(() => {
    if (tab === 'instances' && selId && instances === null) {
      api.getRunInstances(selId).then(setInstances).catch(() => setInstances([]));
    }
  }, [tab, selId, instances]);

  const renderInstances = () => {
    const rows = (instances ?? []).filter(r => !onlyFailed || r.status === 'failed');
    return (
      <div>
        <div style={{ display: 'flex', gap: '0.5rem', marginBottom: '0.75rem', alignItems: 'center' }}>
          <label style={{ fontSize: '0.82rem', display: 'flex', gap: '6px', alignItems: 'center' }}>
            <input type="checkbox" checked={onlyFailed} onChange={e => setOnlyFailed(e.target.checked)} />
            Show failed only
          </label>
          <span style={{ fontSize: '0.8rem', color: 'var(--color-text-secondary)' }}>
            {instances === null ? 'Loading…' : `${rows.length} instance${rows.length === 1 ? '' : 's'}`}
          </span>
        </div>
        {instances === null ? (
          <div className="empty-state">Loading instances…</div>
        ) : rows.length === 0 ? (
          <div className="empty-state">No instance records — the checkpoint has no per-sample data yet.</div>
        ) : (
          <table className="run-table">
            <thead>
              <tr><th>Instance</th><th>Rule</th><th>Sample</th><th>Status</th><th>Duration</th><th>Exit</th></tr>
            </thead>
            <tbody>
              {rows.map(r => (
                <tr key={r.instance}>
                  <td className="mono">{r.instance}</td>
                  <td className="mono">{r.rule}</td>
                  <td>{r.sample ?? '-'}</td>
                  <td><span className={`status-badge ${r.status}`}>{r.status}</span></td>
                  <td>{r.duration_ms != null ? `${(r.duration_ms / 1000).toFixed(1)}s` : '-'}</td>
                  <td>{r.exit_code ?? '-'}</td>
                </tr>
              ))}
            </tbody>
          </table>
        )}
      </div>
    );
  };



  // ── Monitor Dashboard ──
  const renderMonitor = () => {
    if (!monitorStatus) return <div className="empty-state">No monitor data available</div>;

    const hasAlerts = monitorStatus.alerts.length > 0;

    return (
      <div>
        {/* Overall Status */}
        <div style={{ display: 'flex', gap: '1rem', marginBottom: '1rem', alignItems: 'center', flexWrap: 'wrap' }}>
          <span className={`status-badge ${monitorStatus.overall}`} style={{ fontSize: '0.85rem', padding: '4px 12px' }}>
            {monitorStatus.overall === 'normal' ? '🟢' : monitorStatus.overall === 'warning' ? '🟡' : monitorStatus.overall === 'alert' ? '🟠' : '🔴'} {monitorStatus.overall.toUpperCase()}
          </span>
          {monitorStatus.estimated_completion && (
            <span style={{ fontSize: '0.82rem', color: 'var(--color-text-secondary)' }}>
              ETA: {monitorStatus.estimated_completion}
            </span>
          )}
          {/* Pause/Resume/Retry buttons */}
          <div style={{ marginLeft: 'auto', display: 'flex', gap: '4px' }}>
            <button className="btn-sm" onClick={handlePause} title="Pause"><Pause size={14} /></button>
            <button className="btn-sm" onClick={handleResume} title="Resume"><Play size={14} /></button>
            <button className="btn-sm" onClick={handleRetry} title="Retry"><RotateCcw size={14} /></button>
            <button className="btn-sm" style={{ color: 'var(--color-error)', borderColor: 'var(--color-error)' }}
              onClick={handleCancel} title="Cancel run"><Ban size={14} /></button>
            <button className="btn-sm" title="Resume from checkpoint (re-runs unfinished rules)"
              onClick={async () => {
                if (!selId) return;
                if (!window.confirm('Resume this run from its checkpoint? Unfinished rules continue in place.')) return;
                try {
                  const res = await api.resumeCheckpoint(selId);
                  setSelId(res.run_id);
                } catch { /* ignore */ }
              }}>
              <StepForward size={14} />
            </button>
            <button className="btn-sm" title="Clean run workdir (chunks + stale state)"
              onClick={async () => {
                if (!selId) return;
                if (!window.confirm('Clean this run\'s workdir? Chunk files and stale state are removed (checkpoint stays).')) return;
                try {
                  await api.cleanRun(selId);
                } catch { /* ignore */ }
              }}>
              <Trash2 size={14} />
            </button>
          </div>
        </div>

        {/* AI Alert Cards */}
        {hasAlerts && (
          <div style={{ marginBottom: '1rem' }}>
            {monitorStatus.alerts.map((alert, i) => {
              const levelColors: Record<string, string> = {
                info: 'var(--color-info)',
                warn: 'var(--color-warning)',
                alert: 'var(--color-warning)',
                critical: 'var(--color-error)',
              };
              const levelNames: Record<string, string> = {
                info: '🟢 Info', warn: '🟡 Warning', alert: '🟠 Alert', critical: '🔴 Critical',
              };
              return (
                <div key={i} style={{
                  border: `1px solid ${levelColors[alert.level] || 'var(--color-text-tertiary)'}`,
                  background: `${levelColors[alert.level] || 'var(--color-text-tertiary)'}08`,
                  borderRadius: 'var(--radius-md)', padding: '12px', marginBottom: '8px',
                }}>
                  <div style={{ display: 'flex', justifyContent: 'space-between', cursor: 'pointer' }}
                    onClick={() => toggleAlert(`${i}`)}>
                    <div style={{ fontWeight: 600, fontSize: '0.85rem' }}>
                      {levelNames[alert.level] || alert.level}: {alert.rule_name || 'System'}
                    </div>
                    <div style={{ fontSize: '0.75rem', color: 'var(--color-text-tertiary)' }}>
                      {new Date(alert.timestamp).toLocaleTimeString()}
                    </div>
                  </div>
                  <div style={{ fontSize: '0.82rem', marginTop: '6px' }}>
                    <div><strong>Prediction:</strong> {alert.prediction}</div>
                    <div style={{ marginTop: '4px' }}>💡 {alert.suggestion}</div>
                  </div>
                  {alert.auto_fixable && (
                    <div style={{ marginTop: '8px', display: 'flex', gap: '6px' }}>
                      {/* issue #82 P1-4: these were dead buttons — retry is
                          now a real run, manual edit opens the snapshot in
                          the editor. */}
                      <button className="btn-sm" style={{ background: 'var(--color-success)', color: '#fff' }}
                        onClick={() => void handleRetry()}>🔧 Fix & Retry</button>
                      <button className="btn-sm" onClick={async () => {
                        if (!selId) return;
                        try {
                          const run = await api.getRun(selId);
                          session.setPipelineToml(run.pipeline_snapshot ?? '');
                          navigate('/editor');
                        } catch { /* ignore */ }
                      }}>📝 Manual Edit</button>
                    </div>
                  )}
                </div>
              );
            })}
          </div>
        )}

        {/* Resource Forecast */}
        <div className="dash-card" style={{ marginBottom: '1rem' }}>
          <h4 style={{ fontSize: '0.85rem', marginBottom: '8px', display: 'flex', alignItems: 'center', gap: '6px' }}>
            <BarChart3 size={14} /> Resource Forecast
          </h4>
          <div style={{ display: 'grid', gridTemplateColumns: 'repeat(auto-fit, minmax(120px, 1fr))', gap: '0.5rem' }}>
            <StatCard value={monitorStatus.resource_forecast.cpu_trend} label="CPU Trend" color="var(--color-info)" />
            <StatCard value={monitorStatus.resource_forecast.memory_trend} label="Memory Trend" color="var(--color-warning)" />
            <StatCard value={monitorStatus.resource_forecast.disk_trend} label="Disk Trend" color="var(--color-error)" />
            <StatCard value={`${(monitorStatus.resource_forecast.oom_risk * 100).toFixed(0)}%`} label="OOM Risk" color={monitorStatus.resource_forecast.oom_risk > 0.5  ? 'var(--color-error)' : 'var(--color-success)'} />
            <StatCard value={`${(monitorStatus.resource_forecast.timeout_risk * 100).toFixed(0)}%`} label="Timeout Risk" color={monitorStatus.resource_forecast.timeout_risk > 0.5 ? 'var(--color-warning)' : 'var(--color-success)'} />
          </div>
        </div>

        {/* Node status table */}
        {monitorStatus.alerts.length > 0 && (
          <div style={{ marginTop: '0.5rem', fontSize: '0.82rem' }}>
            <h4 style={{ marginBottom: '6px' }}>Recent Events</h4>
            <table className="run-table">
              <thead><tr><th>Time</th><th>Rule</th><th>Level</th><th>Prediction</th></tr></thead>
              <tbody>
                {monitorStatus.alerts.slice(0, 10).map((a, i) => (
                  <tr key={i}>
                    <td style={{ fontSize: '0.75rem' }}>{new Date(a.timestamp).toLocaleTimeString()}</td>
                    <td style={{ fontFamily: 'var(--font-mono)', fontSize: '0.75rem' }}>{a.rule_name || '-'}</td>
                    <td><span className={`status-badge ${a.level}`}>{a.level}</span></td>
                    <td style={{ fontSize: '0.78rem', maxWidth: '300px', overflow: 'hidden', textOverflow: 'ellipsis', whiteSpace: 'nowrap' }}>{a.prediction}</td>
                  </tr>
                ))}
              </tbody>
            </table>
          </div>
        )}
      </div>
    );
  };

  // ── Report Viewer ──
  const renderReport = () => {
    if (!reportData) return <div className="empty-state">No report data available</div>;

    return (
      <div>
        {/* QC Stats */}
        <div style={{ display: 'grid', gridTemplateColumns: 'repeat(auto-fit, minmax(120px, 1fr))', gap: '0.5rem', marginBottom: '1rem' }}>
          <StatCard value={`${reportData.qc_summary?.total_files || 0}`} label="Output Files" color="var(--color-success)" />
          <StatCard value={String(reportData.qc_summary?.total_size_mb || '0')} label="Total Size (MB)" />
          <StatCard value={`${reportData.qc_summary?.directories || 0}`} label="Directories" />
          <StatCard value={reportData.key_findings.length > 0 ? `${reportData.key_findings.length}` : '0'} label="Findings" color={reportData.key_findings.length > 0 ? 'var(--color-warning)' : 'var(--color-success)'} />
        </div>

        {/* AI Narrative */}
        <div className="dash-card" style={{ marginBottom: '1rem' }}>
          <div style={{ fontWeight: 600, marginBottom: '8px', display: 'flex', alignItems: 'center', gap: '6px' }}>
            <Bot size={16} /> AI Narrative
          </div>
          <div style={{ fontSize: '0.85rem', lineHeight: 1.7, whiteSpace: 'pre-wrap' }}>
            {reportData.narrative_md || 'No narrative generated.'}
          </div>
        </div>

        {/* Key Findings */}
        {reportData.key_findings.length > 0 && (
          <div className="dash-card" style={{ marginBottom: '1rem' }}>
            <h4 style={{ fontSize: '0.85rem', marginBottom: '6px' }}>🔬 Key Findings</h4>
            {reportData.key_findings.map((f, i) => (
              <div key={i} style={{ padding: '6px 0', borderBottom: '1px solid var(--color-border-light)', fontSize: '0.82rem' }}>
                <div><strong>{f.finding}</strong> <span className="status-badge">{f.significance}</span></div>
                <div style={{ color: 'var(--color-text-secondary)', marginTop: '2px' }}>{f.evidence}</div>
              </div>
            ))}
          </div>
        )}

        {/* Suggested Next Steps */}
        {reportData.suggested_next.length > 0 && (
          <div className="dash-card" style={{ marginBottom: '1rem' }}>
            <h4 style={{ fontSize: '0.85rem', marginBottom: '6px' }}>💡 Suggested Next Steps</h4>
            <ul style={{ margin: 0, paddingLeft: '1.2rem', fontSize: '0.82rem' }}>
              {reportData.suggested_next.map((s, i) => (
                <li key={i} style={{ marginBottom: '4px' }}>{s}</li>
              ))}
            </ul>
          </div>
        )}

        {/* Caveats */}
        {reportData.caveats.length > 0 && (
          <div className="dash-card" style={{ marginBottom: '1rem', background: 'var(--color-warning-bg)', border: '1px solid var(--color-warning)' }}>
            <h4 style={{ fontSize: '0.85rem', marginBottom: '6px', color: 'var(--color-warning)' }}>⚠️ Caveats</h4>
            <ul style={{ margin: 0, paddingLeft: '1.2rem', fontSize: '0.82rem' }}>
              {reportData.caveats.map((c, i) => (
                <li key={i} style={{ marginBottom: '2px' }}>{c}</li>
              ))}
            </ul>
          </div>
        )}

        {/* File Tree */}
        {reportData.file_tree.length > 0 && (
          <div className="dash-card" style={{ marginBottom: '1rem' }}>
            <h4 style={{ fontSize: '0.85rem', marginBottom: '6px' }}>📁 Output Files</h4>
            <div style={{ maxHeight: '200px', overflow: 'auto', fontSize: '0.78rem' }}>
              <table className="run-table">
                <thead><tr><th>Name</th><th>Size</th><th>Type</th></tr></thead>
                <tbody>
                  {reportData.file_tree.map((f, i) => (
                    <tr key={i}>
                      <td style={{ fontFamily: 'var(--font-mono)' }}>{f.is_dir ? '📁' : '📄'} {f.name}</td>
                      <td>{f.size_bytes > 1024 ? `${(f.size_bytes / 1024).toFixed(1)} KB` : `${f.size_bytes} B`}</td>
                      <td>{f.is_dir ? 'Directory' : 'File'}</td>
                    </tr>
                  ))}
                </tbody>
              </table>
            </div>
          </div>
        )}

        {/* Charts */}
        {reportData.charts.length > 0 && (
          <div className="dash-card" style={{ marginBottom: '1rem' }}>
            <h4 style={{ fontSize: '0.85rem', marginBottom: '6px' }}>📊 Available Charts</h4>
            <div style={{ display: 'flex', gap: '6px', flexWrap: 'wrap' }}>
              {reportData.charts.map((c, i) => (
                <button key={i} className="btn-sm">{c.title}</button>
              ))}
            </div>
          </div>
        )}

        {/* Q&A Input */}
        <div className="dash-card">
          <h4 style={{ fontSize: '0.85rem', marginBottom: '6px', display: 'flex', alignItems: 'center', gap: '6px' }}>
            <Bot size={14} /> Ask AI About Results
          </h4>
          <div style={{ display: 'flex', gap: '6px' }}>
            <input
              type="text"
              value={qaInput}
              onChange={e => setQaInput(e.target.value)}
              onKeyDown={e => e.key === 'Enter' && handleAsk()}
              placeholder="Ask a question about the results..."
              className="intent-input"
              style={{ flex: 1 }}
            />
            <button onClick={handleAsk} className="btn-run" disabled={!qaInput.trim()}>
              <Bot size={14} style={{ marginRight: 4 }} /> Ask
            </button>
          </div>
          {qaAnswer && (
            <div style={{ marginTop: '8px', padding: '8px 12px', background: 'var(--color-bg-tertiary)', borderRadius: 'var(--radius-sm)', fontSize: '0.85rem', lineHeight: 1.6 }}>
              {qaAnswer}
            </div>
          )}
        </div>
      </div>
    );
  };

  // ── Diagnostics ──
  const renderDiagnostics = () => {
    if (!diagnostics) return <div className="empty-state">No diagnostics available</div>;
    const hasIssues = diagnostics.failed_nodes.length > 0 || diagnostics.warnings.length > 0;
    if (!hasIssues) return <div className="empty-state">✅ No issues detected — pipeline looks healthy</div>;
    return (
      <div>
        {diagnostics.failed_nodes.map((fn, i) => {
          const exp = explainState[fn.rule];
          return (
            <div key={i} className="dash-card" style={{ background: 'var(--color-error-bg)', border: '1px solid var(--color-error)', marginBottom: '8px' }}>
              <div style={{ fontWeight: 600, color: 'var(--color-error)' }}>❌ {fn.rule}</div>
              <div style={{ fontSize: '0.85rem' }}>{fn.likely_cause}</div>
              {fn.suggestions.length > 0 && (
                <ul style={{ margin: '4px 0', paddingLeft: '1.2rem', fontSize: '0.82rem' }}>
                  {fn.suggestions.map((s, j) => <li key={j}>{s}</li>)}
                </ul>
              )}
              {/* AI explanation grounded in the deterministic diagnosis
                  (issue #82 P1-10: the explain endpoint existed but the
                  UI never called it). */}
              <button className="btn-sm" style={{ marginTop: '6px' }}
                disabled={exp?.loading}
                onClick={async () => {
                  setExplainState((prev) => ({ ...prev, [fn.rule]: { loading: true } }));
                  try {
                    // Run-scoped explain (issue #82 P1-10): deterministic
                    // grounding + LLM prose, optionally in Chinese.
                    const result = selId ? await api.aiExplain(selId, 'zh') : null;
                    const text = result
                      ? `${result.summary}${result.fix_suggestion ? `\n→ ${result.fix_suggestion.action}` : ''}`
                      : 'AI explanation unavailable — the deterministic diagnosis above is authoritative.';
                    setExplainState((prev) => ({ ...prev, [fn.rule]: { text } }));
                  } catch {
                    setExplainState((prev) => ({ ...prev, [fn.rule]: { text: 'AI explanation unavailable — the deterministic diagnosis above is authoritative.' } }));
                  }
                }}>
                {exp?.loading ? 'Explaining…' : '🤖 AI 解释'}
              </button>
              {exp?.text && (
                <div style={{ marginTop: '6px', fontSize: '0.82rem', background: 'var(--color-bg-tertiary)', padding: '8px', borderRadius: 'var(--radius-sm)' }}>
                  {exp.text}
                </div>
              )}
            </div>
          );
        })}
        {diagnostics.warnings.map((w, i) => (
          <div key={i} className="dash-card" style={{ background: 'var(--color-warning-bg)', border: '1px solid var(--color-warning)', marginBottom: '6px', fontSize: '0.85rem' }}>
            ⚠️ <strong>{w.rule}</strong>: {w.pattern} — {w.suggestion}
          </div>
        ))}
      </div>
    );
  };

  // ── DAG Status ──
  const renderDag = () => {
    if (!dagStatus || dagStatus.nodes.length === 0) return <div className="empty-state">No DAG status available</div>;
    return (
      <div>
        <div style={{ display: 'flex', gap: '1rem', marginBottom: '0.75rem', fontSize: '0.82rem', flexWrap: 'wrap' }}>
          <span>Total: <strong>{dagStatus.metrics.total_nodes}</strong></span>
          <span style={{ color: 'var(--color-success)' }}>✅ Done: <strong>{dagStatus.metrics.completed_nodes}</strong></span>
          <span style={{ color: 'var(--color-info)' }}>🔄 Running: <strong>{dagStatus.metrics.running_nodes}</strong></span>
          <span style={{ color: 'var(--color-error)' }}>❌ Failed: <strong>{dagStatus.metrics.failed_nodes}</strong></span>
          <span>⏳ Pending: <strong>{dagStatus.metrics.pending_nodes}</strong></span>
          {dagStatus.metrics.eta_ms != null && dagStatus.metrics.eta_ms > 0 && (
            <span style={{ color: 'var(--color-text-secondary)' }}>ETA: {(dagStatus.metrics.eta_ms / 60000).toFixed(0)}min</span>
          )}
        </div>
        <div style={{ height: '480px', border: '1px solid var(--color-border)', borderRadius: 'var(--radius-sm)' }}>
          <WorkflowCanvas
            dag={{
              nodes: dagStatus.nodes.map(n => ({
                id: n.id,
                label: n.label,
                color: n.color,
                environment: 'system',
                rule: {},
              })),
              edges: dagStatus.edges.map(e => ({ from: e.source, to: e.target, kind: 'declared' as const })),
            }}
            editable={false}
            scopeKey={`monitor-${routeId}`}
            statusById={Object.fromEntries(dagStatus.nodes.map(n => [n.id, n.status]))}
            context="monitor"
          />
        </div>
      </div>
    );
  };

  const selectedRun = runs.find(r => r.id === selId);

  return (
    <div className="page">
      <h1 className="page-title">Monitor & Reports</h1>
      <p className="page-subtitle">AI-powered execution monitoring and results interpretation</p>

      {/* Run selector */}
      <div className="section">
        <div style={{ display: 'flex', justifyContent: 'space-between', alignItems: 'center', marginBottom: '0.5rem' }}>
          <h2 className="section-title" style={{ marginBottom: 0 }}>Run History</h2>
          <button className="btn-sm" onClick={() => api.listRuns().then((r) => setRuns(r.items))}>Refresh</button>
        </div>
        <table className="run-table">
          <thead><tr><th>ID</th><th>Status</th><th>Phase</th><th>Created</th><th>Monitor</th></tr></thead>
          <tbody>
            {runs.slice(0, visibleCount).map((r) => (
              <tr
                key={r.id}
                onClick={() => selectRun(r.id)}
                style={selId === r.id ? { background: 'var(--color-primary-light)', cursor: 'pointer' } : { cursor: 'pointer' }}
              >
                <td className="mono">{r.id.slice(0, 8)}</td>
                <td><span className={`status-badge ${r.status}`}>{r.status}</span></td>
                <td>{r.phase || '-'}</td>
                <td style={{ fontSize: '0.8rem' }}>{r.created_at ? new Date(r.created_at).toLocaleString() : '-'}</td>
                <td>
                  <button className="btn-sm" onClick={() => navigate(`/runs/${r.id}`)}>
                    {r.status === 'running' ? <Loader2 size={12} className="spin" style={{ marginRight: 4 }} /> : null}
                    {r.status === 'completed' ? '📊 Report' : r.status === 'failed' ? '🔍 Diagnose' : '📡 Monitor'}
                  </button>
                </td>
              </tr>
            ))}
          </tbody>
        </table>
        {runs.length > visibleCount && (
          <button className="btn-sm" style={{ marginTop: '0.5rem' }} onClick={() => setVisibleCount((n) => n + 20)}>
            Show {Math.min(20, runs.length - visibleCount)} more of {runs.length} runs
          </button>
        )}
      </div>

      {/* Dry-run preview: instance-level plan from the CLI's --json output
          (issue #79 P2 — the preview used to show unexpanded rules). */}
      {preview && (
        <div className="dash-card" ref={detailRef}>
          <h3 style={{ fontSize: '1rem', fontFamily: 'var(--font-mono)', marginBottom: '0.5rem' }}>
            Dry-run preview
          </h3>
          <div style={{ display: 'flex', gap: '0.75rem', marginBottom: '0.75rem', flexWrap: 'wrap' }}>
            <span className={`status-badge ${preview.checkpoint_preview.summary.will_run > 0 ? 'running' : 'skipped'}`}>
              {preview.checkpoint_preview.summary.will_run} will run
            </span>
            <span className="status-badge skipped">
              {preview.checkpoint_preview.summary.will_skip} will skip
            </span>
            <span style={{ fontSize: '0.8rem', color: 'var(--color-text-secondary)' }}>
              {preview.checkpoint_preview.summary.protected_outside} completed outside this set (preserved)
            </span>
          </div>
          <div className="overflow-x">
            <table className="data-table">
              <thead><tr><th>Instance</th><th>Status</th></tr></thead>
              <tbody>
                {preview.checkpoint_preview.plan.map((p) => (
                  <tr key={p.name}>
                    <td className="mono">{p.name}</td>
                    <td>
                      <span className={`status-badge ${p.status.includes('skip') ? 'skipped' : p.status.includes('rerun') ? 'running' : 'queued'}`}>
                        {p.status.replace(/-/g, ' ')}
                      </span>
                    </td>
                  </tr>
                ))}
              </tbody>
            </table>
          </div>
        </div>
      )}

      {/* Run Detail */}
      {(selId) && (
        <div className="dash-card" ref={detailRef}>
          <div style={{ display: 'flex', justifyContent: 'space-between', alignItems: 'center', marginBottom: '0.75rem', flexWrap: 'wrap', gap: '0.5rem' }}>
            <div>
              <h3 style={{ fontSize: '1rem', fontFamily: 'var(--font-mono)' }}>Run {selId.slice(0, 12)}...</h3>
              <div style={{ fontSize: '0.8rem', color: 'var(--color-text-secondary)' }}>
                Status: <span className={`status-badge ${selectedRun?.status || 'unknown'}`}>{selectedRun?.status || 'unknown'}</span>
                · Phase: {selectedRun?.phase || '-'}
              </div>
            </div>
            <div style={{ display: 'flex', gap: '4px' }}>
              {(['monitor', 'report', 'diagnostics', 'dag', 'logs', 'instances'] as const).map((t) => (
                <button key={t} onClick={() => { setTab(t); setQaAnswer(null); }}
                  className={tab === t ? 'btn-run' : 'btn-sm'}>
                  {t === 'monitor' ? '📡 Monitor' : t === 'report' ? '📊 Report' : t === 'diagnostics' ? '🔍 Diagnostics' : t === 'dag' ? '🔷 DAG' : t === 'logs' ? '📜 Logs' : '🧬 Instances'}
                </button>
              ))}
            </div>
          </div>

          {tab === 'monitor' && renderMonitor()}
          {tab === 'report' && renderReport()}
          {tab === 'diagnostics' && renderDiagnostics()}
          {tab === 'dag' && renderDag()}
          {tab === 'logs' && renderLogs()}
          {tab === 'instances' && renderInstances()}
        </div>
      )}
    </div>
  );
}
