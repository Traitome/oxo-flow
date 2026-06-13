import { useState, useEffect, useCallback } from 'react';
import { api, apiV2 } from '../api/client';
import type { RunItem, MonitorStatus, ReportData, DagStatus, Diagnostics } from '../api/types';
import { Play, Pause, RotateCcw, AlertTriangle, FileText, BarChart3, Loader2, Bot } from 'lucide-react';

type TabType = 'monitor' | 'report' | 'diagnostics' | 'dag';

const STATUS_COLORS: Record<string, string> = {
  success: '#059669', completed: '#059669',
  running: '#2563EB', failed: '#DC2626',
  pending: '#94A3B8', queued: '#D97706',
  skipped: '#94A3B8', paused: '#D97706',
};

function StatCard({ value, label, color }: { value: string; label: string; color?: string }) {
  return (
    <div className="stat-card">
      <div className="stat-value" style={{ color: color || 'var(--color-text)' }}>{value}</div>
      <div className="stat-label">{label}</div>
    </div>
  );
}

export default function MonitorReport() {
  const [runs, setRuns] = useState<RunItem[]>([]);
  const [selId, setSelId] = useState<string | null>(null);
  const [monitorStatus, setMonitorStatus] = useState<MonitorStatus | null>(null);
  const [reportData, setReportData] = useState<ReportData | null>(null);
  const [dagStatus, setDagStatus] = useState<DagStatus | null>(null);
  const [diagnostics, setDiagnostics] = useState<Diagnostics | null>(null);
  const [tab, setTab] = useState<TabType>('monitor');
  const [loading, setLoading] = useState(true);
  const [qaInput, setQaInput] = useState('');
  const [qaAnswer, setQaAnswer] = useState<string | null>(null);
  const [alertOpen, setAlertOpen] = useState<string[]>([]);

  const toggleAlert = (idx: string) => {
    setAlertOpen(prev => prev.includes(idx) ? prev.filter(x => x !== idx) : [...prev, idx]);
  };

  useEffect(() => {
    api.listRuns().then(r => { setRuns(r); setLoading(false); }).catch(() => setLoading(false));
  }, []);

  // Update monitor status in real-time via SSE
  useEffect(() => {
    if (!selId) return;
    const es = new EventSource('/api/events');
    const interval = setInterval(async () => {
      try {
        const status = await apiV2.aiStatus(selId);
        setMonitorStatus(status);
      } catch { /* ignore */ }
    }, 5000);

    es.onmessage = (evt) => {
      try {
        const event = JSON.parse(evt.data);
        if (event.data?.run_id === selId) {
          if (event.type === 'run_completed' || event.type === 'run_failed') {
            clearInterval(interval);
            api.listRuns().then(setRuns);
          }
        }
      } catch { /* ignore */ }
    };
    return () => { clearInterval(interval); es.close(); };
  }, [selId]);

  const selectRun = useCallback(async (id: string) => {
    setSelId(id);
    setTab('monitor');
    setQaAnswer(null);
    try { setMonitorStatus(await apiV2.aiStatus(id)); } catch { setMonitorStatus(null); }
    try { setReportData(await apiV2.runReport(id)); } catch { setReportData(null); }
    try { setDagStatus(await api.getDagStatus(id)); } catch { setDagStatus(null); }
    try { setDiagnostics(await api.getDiagnostics(id)); } catch { setDiagnostics(null); }
  }, []);

  const handlePause = async () => {
    if (!selId) return;
    try {
      await apiV2.pauseRun(selId, 'user_request');
      const s = await apiV2.aiStatus(selId);
      setMonitorStatus(s);
    } catch { /* ignore */ }
  };

  const handleResume = async () => {
    if (!selId) return;
    try {
      await apiV2.resumeRun(selId);
      const s = await apiV2.aiStatus(selId);
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

  const handleAsk = async () => {
    if (!qaInput.trim() || !selId) return;
    try {
      const answer = await apiV2.askReport(selId, qaInput);
      setQaAnswer(answer);
    } catch {
      setQaAnswer('Sorry, I could not answer that question. Please try rephrasing.');
    }
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
          </div>
        </div>

        {/* AI Alert Cards */}
        {hasAlerts && (
          <div style={{ marginBottom: '1rem' }}>
            {monitorStatus.alerts.map((alert, i) => {
              const levelColors: Record<string, string> = {
                info: '#2563EB', warn: '#D97706', alert: '#EA580C', critical: '#DC2626',
              };
              const levelNames: Record<string, string> = {
                info: '🟢 Info', warn: '🟡 Warning', alert: '🟠 Alert', critical: '🔴 Critical',
              };
              return (
                <div key={i} style={{
                  border: `1px solid ${levelColors[alert.level] || '#94A3B8'}`,
                  background: `${levelColors[alert.level] || '#94A3B8'}08`,
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
                      <button className="btn-sm" style={{ background: '#059669', color: '#fff' }}>🔧 Fix & Retry</button>
                      <button className="btn-sm">📝 Manual Edit</button>
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
            <StatCard value={monitorStatus.resource_forecast.cpu_trend} label="CPU Trend" color="#2563EB" />
            <StatCard value={monitorStatus.resource_forecast.memory_trend} label="Memory Trend" color="#D97706" />
            <StatCard value={monitorStatus.resource_forecast.disk_trend} label="Disk Trend" color="#DC2626" />
            <StatCard value={`${(monitorStatus.resource_forecast.oom_risk * 100).toFixed(0)}%`} label="OOM Risk" color={monitorStatus.resource_forecast.oom_risk > 0.5 ? '#DC2626' : '#059669'} />
            <StatCard value={`${(monitorStatus.resource_forecast.timeout_risk * 100).toFixed(0)}%`} label="Timeout Risk" color={monitorStatus.resource_forecast.timeout_risk > 0.5 ? '#EA580C' : '#059669'} />
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
          <StatCard value={`${reportData.qc_summary?.total_files || 0}`} label="Output Files" color="#059669" />
          <StatCard value={reportData.qc_summary?.total_size_mb || '0'} label="Total Size (MB)" />
          <StatCard value={`${reportData.qc_summary?.directories || 0}`} label="Directories" />
          <StatCard value={reportData.key_findings.length > 0 ? `${reportData.key_findings.length}` : '0'} label="Findings" color={reportData.key_findings.length > 0 ? '#D97706' : '#059669'} />
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
            <h4 style={{ fontSize: '0.85rem', marginBottom: '6px', color: '#D97706' }}>⚠️ Caveats</h4>
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
        {diagnostics.failed_nodes.map((fn, i) => (
          <div key={i} className="dash-card" style={{ background: 'var(--color-error-bg)', border: '1px solid var(--color-error)', marginBottom: '8px' }}>
            <div style={{ fontWeight: 600, color: 'var(--color-error)' }}>❌ {fn.rule}</div>
            <div style={{ fontSize: '0.85rem' }}>{fn.likely_cause}</div>
            {fn.suggestions.length > 0 && (
              <ul style={{ margin: '4px 0', paddingLeft: '1.2rem', fontSize: '0.82rem' }}>
                {fn.suggestions.map((s, j) => <li key={j}>{s}</li>)}
              </ul>
            )}
          </div>
        ))}
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
    if (!dagStatus) return <div className="empty-state">No DAG status available</div>;
    return (
      <div>
        <div style={{ display: 'flex', gap: '1rem', marginBottom: '0.75rem', fontSize: '0.82rem', flexWrap: 'wrap' }}>
          <span>Total: <strong>{dagStatus.metrics.total_nodes}</strong></span>
          <span style={{ color: '#059669' }}>Done: <strong>{dagStatus.metrics.completed_nodes}</strong></span>
          <span style={{ color: '#2563EB' }}>Running: <strong>{dagStatus.metrics.running_nodes}</strong></span>
          <span style={{ color: '#DC2626' }}>Failed: <strong>{dagStatus.metrics.failed_nodes}</strong></span>
          <span>Pending: <strong>{dagStatus.metrics.pending_nodes}</strong></span>
        </div>
        <div style={{ display: 'flex', flexWrap: 'wrap', gap: '6px' }}>
          {dagStatus.nodes.map((n) => (
            <div key={n.id} style={{
              padding: '6px 12px', borderRadius: 'var(--radius-sm)', fontSize: '0.8rem',
              background: n.color === 'green' ? '#05966920' : n.color === 'red' ? '#DC262620' : n.color === 'blue' ? '#2563EB20' : '#94A3B820',
              border: `1px solid ${n.color === 'green' ? '#059669' : n.color === 'red' ? '#DC2626' : n.color === 'blue' ? '#2563EB' : '#94A3B8'}`,
            }}>
              {n.label} {n.duration_ms ? `${(n.duration_ms / 1000).toFixed(1)}s` : ''}
            </div>
          ))}
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
          <button className="btn-sm" onClick={() => api.listRuns().then(setRuns)}>Refresh</button>
        </div>
        <table className="run-table">
          <thead><tr><th>ID</th><th>Status</th><th>Phase</th><th>Created</th><th>Monitor</th></tr></thead>
          <tbody>
            {runs.map((r) => (
              <tr key={r.id} style={selId === r.id ? { background: '#2563EB20' } : {}}>
                <td className="mono">{r.id.slice(0, 8)}</td>
                <td><span className={`status-badge ${r.status}`}>{r.status}</span></td>
                <td>{r.phase || '-'}</td>
                <td style={{ fontSize: '0.8rem' }}>{r.created_at ? new Date(r.created_at).toLocaleString() : '-'}</td>
                <td>
                  <button className="btn-sm" onClick={() => selectRun(r.id)}>
                    {r.status === 'running' ? <Loader2 size={12} className="spin" style={{ marginRight: 4 }} /> : null}
                    {r.status === 'completed' ? '📊 Report' : r.status === 'failed' ? '🔍 Diagnose' : '📡 Monitor'}
                  </button>
                </td>
              </tr>
            ))}
          </tbody>
        </table>
      </div>

      {/* Run Detail */}
      {selId && selectedRun && (
        <div className="dash-card">
          <div style={{ display: 'flex', justifyContent: 'space-between', alignItems: 'center', marginBottom: '0.75rem', flexWrap: 'wrap', gap: '0.5rem' }}>
            <div>
              <h3 style={{ fontSize: '1rem', fontFamily: 'var(--font-mono)' }}>Run {selId.slice(0, 12)}...</h3>
              <div style={{ fontSize: '0.8rem', color: 'var(--color-text-secondary)' }}>
                Status: <span className={`status-badge ${selectedRun.status}`}>{selectedRun.status}</span>
                · Phase: {selectedRun.phase || '-'}
              </div>
            </div>
            <div style={{ display: 'flex', gap: '4px' }}>
              {(['monitor', 'report', 'diagnostics', 'dag'] as const).map((t) => (
                <button key={t} onClick={() => { setTab(t); setQaAnswer(null); }}
                  className={tab === t ? 'btn-run' : 'btn-sm'}>
                  {t === 'monitor' ? '📡 Monitor' : t === 'report' ? '📊 Report' : t === 'diagnostics' ? '🔍 Diagnostics' : '🔷 DAG'}
                </button>
              ))}
            </div>
          </div>

          {tab === 'monitor' && renderMonitor()}
          {tab === 'report' && renderReport()}
          {tab === 'diagnostics' && renderDiagnostics()}
          {tab === 'dag' && renderDag()}
        </div>
      )}
    </div>
  );
}
