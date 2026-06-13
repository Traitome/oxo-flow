import { useEffect, useState, useCallback } from 'react';
import { api } from '../api/client';
import type { RunItem, RunStatus, Diagnostics, DagStatus } from '../api/types';
import { showToast } from '../components/Toast';

const STATUS_COLOR: Record<string, string> = {
  success: 'var(--color-success)', completed: 'var(--color-success)',
  running: 'var(--color-primary)', failed: 'var(--color-error)',
  pending: 'var(--color-text-tertiary)', queued: 'var(--color-warning)',
  skipped: 'var(--color-text-tertiary)', cancelled: 'var(--color-text-tertiary)',
};

function LoadingSkeleton() {
  return <div><div className="skeleton skeleton-text" /><div className="skeleton skeleton-text short" /><div className="skeleton skeleton-card" /></div>;
}

function EmptyState({ title, action, onAction }: { title: string; action?: string; onAction?: () => void }) {
  return (
    <div className="empty-state">
      <div className="empty-state-icon">📋</div>
      <p style={{ marginBottom: '0.75rem', fontSize: '0.9rem' }}>{title}</p>
      {action && <button className="btn-run" onClick={onAction}>{action}</button>}
    </div>
  );
}

function Modal({ title, message, onConfirm, onCancel }: { title: string; message: string; onConfirm: () => void; onCancel: () => void }) {
  return (
    <div className="modal-overlay" onClick={onCancel}>
      <div className="modal" onClick={(e) => e.stopPropagation()}>
        <div className="modal-header">
          <h3 style={{ fontSize: '1.1rem' }}>{title}</h3>
        </div>
        <div className="modal-body">
          <p style={{ fontSize: '0.9rem', color: 'var(--color-text-secondary)' }}>{message}</p>
        </div>
        <div className="modal-footer">
          <button className="btn-sm" onClick={onCancel}>Cancel</button>
          <button className="btn-run" onClick={onConfirm} style={{ background: 'var(--color-error)', color: '#fff' }}>Confirm</button>
        </div>
      </div>
    </div>
  );
}

export default function Runs() {
  const [runs, setRuns] = useState<RunItem[]>([]);
  const [loading, setLoading] = useState(true);
  const [selId, setSelId] = useState<string | null>(null);
  const [runStatus, setRunStatus] = useState<RunStatus | null>(null);
  const [dagStatus, setDagStatus] = useState<DagStatus | null>(null);
  const [diagnostics, setDiagnostics] = useState<Diagnostics | null>(null);
  const [logs, setLogs] = useState<string | null>(null);
  const [results, setResults] = useState<Array<{ name: string; path: string; size_bytes: number; is_dir: boolean }>>([]);
  const [tab, setTab] = useState<'monitor' | 'dag' | 'diagnostics' | 'results'>('monitor');
  const [tabLoading, setTabLoading] = useState(false);
  const [cancelTarget, setCancelTarget] = useState<string | null>(null);

  const loadRuns = async () => {
    setLoading(true);
    try { setRuns(await api.listRuns()); } catch { /* empty */ }
    setLoading(false);
  };
  useEffect(() => { loadRuns(); }, []);

  // SSE subscription for real-time run status updates
  useEffect(() => {
    const es = new EventSource('/api/events');
    es.onmessage = (evt) => {
      try {
        const event = JSON.parse(evt.data);
        if (event.type === 'run_started' || event.type === 'run_failed' || event.type === 'run_completed' || event.type === 'run_cancelled') {
          loadRuns();
          if (selId && event.data?.run_id === selId) {
            selectRun(selId);
          }
        }
      } catch { /* ignore parse errors */ }
    };
    es.onerror = () => { /* reconnect automatically */ };
    return () => es.close();
  }, [selId]);

  const selectRun = useCallback(async (id: string) => {
    setSelId(id); setTab('monitor'); setTabLoading(true);
    try { setRunStatus(await api.getRunStatus(id)); } catch { setRunStatus(null); }
    try { setDagStatus(await api.getDagStatus(id)); } catch { setDagStatus(null); }
    try { setDiagnostics(await api.getDiagnostics(id)); } catch { setDiagnostics(null); }
    try { setResults(await api.getRunResults(id)); } catch { setResults([]); }
    try { setLogs(await api.getRunLogs(id)); } catch { setLogs(null); }
    setTabLoading(false);
  }, []);

  const handleRetry = async (id: string) => {
    try {
      const plan = await api.retryRun(id);
      showToast(`Retry: ${plan.will_rerun.length} rules to rerun, ${plan.will_skip.length} skipped`, 'success');
      loadRuns();
    } catch { showToast('Retry failed', 'error'); }
  };

  const handleCancel = async (id: string) => {
    setCancelTarget(id);
  };

  const confirmCancel = async () => {
    if (!cancelTarget) return;
    const id = cancelTarget;
    setCancelTarget(null);
    try { await api.cancelRun(id); showToast('Run cancelled', 'success'); loadRuns(); }
    catch { showToast('Cancel failed', 'error'); }
  };

  // ── Execution Monitor ──
  const renderMonitor = () => {
    if (!runStatus) return <EmptyState title="No status data available" />;
    const nodes = runStatus.nodes;
    const total = nodes.length;
    const completed = nodes.filter(n => n.status === 'success' || n.status === 'skipped').length;
    const failed = nodes.filter(n => n.status === 'failed').length;
    const running = nodes.filter(n => n.status === 'running').length;
    const pct = total > 0 ? Math.round((completed / total) * 100) : 0;

    return (
      <div>
        {/* Progress bar */}
        <div style={{ marginBottom: '1rem' }}>
          <div style={{ display: 'flex', justifyContent: 'space-between', marginBottom: '4px', fontSize: '0.8rem' }}>
            <span>{completed}/{total} rules completed</span>
            <span>{failed > 0 ? `${failed} failed · ` : ''}{running} running</span>
          </div>
          <div style={{ background: 'var(--color-bg-tertiary)', height: '6px', borderRadius: '3px', overflow: 'hidden' }}>
            <div style={{ width: `${pct}%`, height: '100%', background: failed > 0 ? 'var(--color-error)' : 'var(--color-success)', transition: 'width 0.5s', borderRadius: '3px' }} />
          </div>
        </div>

        {/* Timeline */}
        <div style={{ fontSize: '0.82rem' }}>
          {nodes.map((n) => {
            const dur = n.duration_ms ? `${(n.duration_ms / 1000).toFixed(1)}s` : '';
            const maxDur = Math.max(...nodes.map(x => x.duration_ms || 0), 1);
            const barW = n.duration_ms ? Math.max((n.duration_ms / maxDur) * 60, 3) : 3;
            return (
              <div key={n.rule} style={{ display: 'flex', alignItems: 'center', gap: '8px', padding: '5px 0', borderBottom: '1px solid var(--color-border-light)' }}>
                <span style={{ width: '12px', height: '12px', borderRadius: '50%', background: STATUS_COLOR[n.status] || 'gray', flexShrink: 0 }} />
                <span style={{ width: '140px', fontFamily: 'var(--font-mono)', fontSize: '0.78rem', overflow: 'hidden', textOverflow: 'ellipsis', whiteSpace: 'nowrap' }} title={n.rule}>{n.rule}</span>
                <span style={{ flex: 1 }}>
                  <span style={{ display: 'inline-block', height: '4px', width: `${barW}%`, background: STATUS_COLOR[n.status] || 'gray', borderRadius: '2px', verticalAlign: 'middle' }} />
                </span>
                <span style={{ width: '60px', textAlign: 'right', fontSize: '0.75rem', color: 'var(--color-text-tertiary)', fontFamily: 'var(--font-mono)' }}>{dur}</span>
                <span className={`status-badge ${n.status}`} style={{ fontSize: '0.65rem' }}>{n.status}</span>
                {n.exit_code != null && n.exit_code !== 0 && <span style={{ color: 'var(--color-error)', fontSize: '0.7rem', fontFamily: 'var(--font-mono)' }}>exit {n.exit_code}</span>}
              </div>
            );
          })}
        </div>

        {/* Resource snapshot */}
        <div style={{ marginTop: '1rem', display: 'flex', gap: '1rem', fontSize: '0.78rem', color: 'var(--color-text-secondary)' }}>
          <span>CPU: {runStatus.resources?.cpu_pct?.toFixed(1) || '0'}%</span>
          <span>Mem: {runStatus.resources?.memory_mb || 0} MB</span>
          <span>Disk: {runStatus.resources?.disk_mb || 0} MB</span>
        </div>
      </div>
    );
  };

  // ── DAG View ──
  const renderDag = () => {
    if (!dagStatus) return <EmptyState title="No DAG data available" />;
    const m = dagStatus.metrics;
    return (
      <div>
        <div style={{ display: 'flex', gap: '1rem', marginBottom: '0.75rem', fontSize: '0.82rem', flexWrap: 'wrap' }}>
          <span>Total: <strong>{m.total_nodes}</strong></span>
          <span style={{ color: 'var(--color-success)' }}>Done: <strong>{m.completed_nodes}</strong></span>
          <span style={{ color: 'var(--color-primary)' }}>Running: <strong>{m.running_nodes}</strong></span>
          <span style={{ color: 'var(--color-error)' }}>Failed: <strong>{m.failed_nodes}</strong></span>
          <span>Pending: <strong>{m.pending_nodes}</strong></span>
          {m.eta_ms != null && <span>ETA: <strong>{(m.eta_ms / 1000).toFixed(0)}s</strong></span>}
        </div>
        <div style={{ display: 'flex', flexWrap: 'wrap', gap: '6px' }}>
          {dagStatus.nodes.map((n) => (
            <div key={n.id} style={{
              padding: '6px 12px', borderRadius: 'var(--radius-sm)', fontSize: '0.8rem', fontFamily: 'var(--font-mono)',
              background: n.color === 'green' ? 'var(--color-success-bg)' : n.color === 'red' ? 'var(--color-error-bg)' : n.color === 'blue' ? 'var(--color-primary-light)' : 'var(--color-bg-tertiary)',
              border: `1px solid ${n.color === 'green' ? 'var(--color-success)' : n.color === 'red' ? 'var(--color-error)' : n.color === 'blue' ? 'var(--color-primary)' : 'var(--color-border)'}`,
            }}>
              {n.label} {n.duration_ms ? `${(n.duration_ms / 1000).toFixed(1)}s` : ''}
            </div>
          ))}
        </div>
      </div>
    );
  };

  // ── Diagnostics ──
  const renderDiagnostics = () => {
    if (!diagnostics) return <EmptyState title="No diagnostics data available" />;
    if (diagnostics.failed_nodes.length === 0 && diagnostics.warnings.length === 0)
      return <EmptyState title="No issues detected — pipeline looks healthy" />;
    return (
      <div>
        {diagnostics.failed_nodes.map((fn, i) => (
          <div key={i} style={{ background: 'var(--color-error-bg)', border: '1px solid var(--color-error)', borderRadius: 'var(--radius-md)', padding: '0.75rem', marginBottom: '0.75rem' }}>
            <div style={{ fontWeight: 600, color: 'var(--color-error)', marginBottom: '0.25rem' }}>
              ❌ {fn.rule} — {fn.error_pattern || 'Unknown error'}
            </div>
            <div style={{ fontSize: '0.85rem', marginBottom: '0.25rem' }}><strong>Likely cause:</strong> {fn.likely_cause}</div>
            {fn.suggestions.length > 0 && (
              <div style={{ fontSize: '0.85rem' }}>
                <strong>Suggestions:</strong>
                <ul style={{ marginTop: '4px', paddingLeft: '1.2rem' }}>{fn.suggestions.map((s, j) => <li key={j} style={{ marginBottom: '2px' }}>{s}</li>)}</ul>
              </div>
            )}
            {fn.relevant_log_lines.length > 0 && (
              <pre style={{ background: 'var(--color-bg-tertiary)', padding: '0.5rem', borderRadius: 'var(--radius-sm)', fontSize: '0.75rem', marginTop: '0.5rem', maxHeight: '120px', overflow: 'auto', fontFamily: 'var(--font-mono)', whiteSpace: 'pre-wrap' }}>
                {fn.relevant_log_lines.join('\n')}
              </pre>
            )}
          </div>
        ))}
        {diagnostics.warnings.map((w, i) => (
          <div key={`w${i}`} style={{ background: 'var(--color-warning-bg)', border: '1px solid var(--color-warning)', borderRadius: 'var(--radius-md)', padding: '0.5rem 0.75rem', marginBottom: '0.5rem', fontSize: '0.85rem' }}>
            ⚠️ <strong>{w.rule}</strong>: {w.pattern} — {w.suggestion}
          </div>
        ))}
        {diagnostics.resource_bottlenecks.length > 0 && (
          <div style={{ marginTop: '0.5rem' }}>
            <strong style={{ fontSize: '0.85rem' }}>Resource Bottlenecks:</strong>
            {diagnostics.resource_bottlenecks.map((rb, i) => (
              <div key={i} style={{ fontSize: '0.8rem', padding: '4px 0' }}>{rb.rule}: {rb.metric} — actual {rb.actual}, limit {rb.limit}</div>
            ))}
          </div>
        )}
      </div>
    );
  };

  // ── Results Browser ──
  const renderResults = () => {
    if (results.length === 0) return <EmptyState title="No output files found" />;
    const totalSize = results.reduce((s, f) => s + f.size_bytes, 0);
    return (
      <div>
        {/* QC Cards */}
        <div style={{ display: 'grid', gridTemplateColumns: 'repeat(auto-fit, minmax(160px, 1fr))', gap: '0.5rem', marginBottom: '1rem' }}>
          <div className="stat-card"><div><div className="stat-value" style={{ fontSize: '1.2rem' }}>{results.length}</div><div className="stat-label">Output Files</div></div></div>
          <div className="stat-card"><div><div className="stat-value" style={{ fontSize: '1.2rem' }}>{(totalSize / 1024).toFixed(0)}</div><div className="stat-label">Total Size (KB)</div></div></div>
          <div className="stat-card"><div><div className="stat-value" style={{ fontSize: '1.2rem' }}>{results.filter(f => f.is_dir).length}</div><div className="stat-label">Directories</div></div></div>
        </div>

        {/* File tree */}
        <div style={{ maxHeight: '400px', overflow: 'auto' }}>
          <table className="run-table">
            <thead><tr><th>Name</th><th>Type</th><th>Size</th><th>Path</th></tr></thead>
            <tbody>
              {results.map((f, i) => (
                <tr key={i}>
                  <td style={{ fontFamily: 'var(--font-mono)', fontSize: '0.8rem' }}>{f.is_dir ? '📁' : '📄'} {f.name}</td>
                  <td>{f.is_dir ? 'Directory' : 'File'}</td>
                  <td style={{ fontFamily: 'var(--font-mono)', fontSize: '0.8rem' }}>{f.size_bytes > 1024 ? `${(f.size_bytes / 1024).toFixed(1)} KB` : `${f.size_bytes} B`}</td>
                  <td style={{ fontFamily: 'var(--font-mono)', fontSize: '0.75rem', color: 'var(--color-text-tertiary)', maxWidth: '300px', overflow: 'hidden', textOverflow: 'ellipsis', whiteSpace: 'nowrap' }}>{f.path}</td>
                </tr>
              ))}
            </tbody>
          </table>
        </div>

        {/* Logs preview */}
        {logs && (
          <details style={{ marginTop: '1rem' }}>
            <summary style={{ cursor: 'pointer', fontSize: '0.85rem', color: 'var(--color-text-secondary)' }}>Execution Log</summary>
            <pre style={{ background: 'var(--color-bg-tertiary)', padding: '0.75rem', borderRadius: 'var(--radius-sm)', fontSize: '0.78rem', maxHeight: '300px', overflow: 'auto', fontFamily: 'var(--font-mono)', whiteSpace: 'pre-wrap', lineHeight: 1.5, marginTop: '0.5rem' }}>
              {logs || 'No log output'}
            </pre>
          </details>
        )}
      </div>
    );
  };

  const run = runs.find(r => r.id === selId);

  return (
    <div className="page">
      <h1 className="page-title">Runs</h1>
      <p className="page-subtitle">Execution history, monitoring, and diagnostics</p>

      {/* Run list */}
      <div className="section">
        <div style={{ display: 'flex', justifyContent: 'space-between', alignItems: 'center', marginBottom: '0.5rem' }}>
          <h2 className="section-title" style={{ marginBottom: 0 }}>Run History</h2>
          <button className="btn-sm" onClick={loadRuns}>Refresh</button>
        </div>

        {loading ? <LoadingSkeleton /> : runs.length === 0 ? (
          <EmptyState title="No runs yet" action="Create a pipeline" onAction={() => window.location.href = '/editor'} />
        ) : (
          <table className="run-table">
            <thead><tr><th>ID</th><th>Pipeline</th><th>Status</th><th>Phase</th><th>Created</th><th>Actions</th></tr></thead>
            <tbody>
              {runs.map((r) => (
                <tr key={r.id} style={selId === r.id ? { background: 'var(--color-primary-light)' } : {}}>
                  <td className="mono">{r.id.slice(0, 8)}</td>
                  <td className="mono">{r.pipeline_id?.slice(0, 8) || '-'}</td>
                  <td><span className={`status-badge ${r.status}`}>{r.status}</span></td>
                  <td>{r.phase || '-'}</td>
                  <td style={{ fontSize: '0.8rem' }}>{r.created_at ? new Date(r.created_at).toLocaleString() : '-'}</td>
                  <td>
                    <button className="btn-sm" onClick={() => selectRun(r.id)} style={{ marginRight: 4 }}>Monitor</button>
                    {r.status === 'running' && <button className="btn-sm" onClick={() => handleCancel(r.id)} style={{ color: 'var(--color-error)', marginRight: 4 }}>Cancel</button>}
                    {r.status === 'failed' && <button className="btn-sm" onClick={() => handleRetry(r.id)} style={{ color: 'var(--color-warning)' }}>Retry</button>}
                  </td>
                </tr>
              ))}
            </tbody>
          </table>
        )}
      </div>

      {/* Run detail panel */}
      {selId && run && (
        <div className="dash-card" style={{ marginTop: '0.5rem' }}>
          <div style={{ display: 'flex', justifyContent: 'space-between', alignItems: 'center', marginBottom: '0.75rem', flexWrap: 'wrap', gap: '0.5rem' }}>
            <div>
              <h3 style={{ fontSize: '1rem', fontFamily: 'var(--font-mono)' }}>Run {selId.slice(0, 12)}...</h3>
              <div style={{ fontSize: '0.8rem', color: 'var(--color-text-secondary)', marginTop: '2px' }}>
                Pipeline: {run.pipeline_id?.slice(0, 16) || '-'} ·
                Phase: {run.phase || '-'} ·
                Started: {run.started_at ? new Date(run.started_at).toLocaleString() : '-'}
              </div>
            </div>
            <div style={{ display: 'flex', gap: '4px' }}>
              {(['monitor', 'dag', 'diagnostics', 'results'] as const).map((t) => (
                <button key={t} onClick={() => setTab(t)} className={tab === t ? 'btn-run' : 'btn-sm'}>
                  {t === 'monitor' ? '📊 Monitor' : t === 'dag' ? '🔷 DAG' : t === 'diagnostics' ? '🔍 Diagnostics' : '📁 Results'}
                </button>
              ))}
              <button className="btn-sm" onClick={() => setSelId(null)}>✕</button>
            </div>
          </div>

          {tabLoading ? <LoadingSkeleton /> : (
            <>
              {tab === 'monitor' && renderMonitor()}
              {tab === 'dag' && renderDag()}
              {tab === 'diagnostics' && renderDiagnostics()}
              {tab === 'results' && renderResults()}
            </>
          )}
        </div>
      )}

      {/* Confirm Cancel Modal */}
      {cancelTarget && (
        <Modal
          title="Cancel Run"
          message="Cancel this run? This action cannot be undone."
          onConfirm={confirmCancel}
          onCancel={() => setCancelTarget(null)}
        />
      )}
    </div>
  );
}
