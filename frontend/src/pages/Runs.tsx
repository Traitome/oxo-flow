import { useEffect, useState } from 'react';
import { api } from '../api/client';
import type { RunItem, RunStatus, Diagnostics, DagStatus } from '../api/types';

export default function Runs() {
  const [runs, setRuns] = useState<RunItem[]>([]);
  const [selRun, setSelRun] = useState<RunItem | null>(null);
  const [runStatus, setRunStatus] = useState<RunStatus | null>(null);
  const [dagStatus, setDagStatus] = useState<DagStatus | null>(null);
  const [diagnostics, setDiagnostics] = useState<Diagnostics | null>(null);
  const [logs, setLogs] = useState<string | null>(null);
  const [tab, setTab] = useState<'status' | 'dag' | 'diagnostics' | 'logs'>('status');

  const loadRuns = () => api.listRuns().then(setRuns).catch(() => {});

  useEffect(() => { loadRuns(); }, []);

  const showDetail = async (r: RunItem) => {
    setSelRun(r); setTab('status'); setDiagnostics(null); setLogs(null);
    try { setRunStatus(await api.getRunStatus(r.id)); } catch { setRunStatus(null); }
    try { setDagStatus(await api.getDagStatus(r.id)); } catch { setDagStatus(null); }
    try { setDiagnostics(await api.getDiagnostics(r.id)); } catch { setDiagnostics(null); }
    try { setLogs(await api.getRunLogs(r.id)); } catch { setLogs(null); }
  };

  const handleRetry = async (id: string) => {
    try {
      const plan = await api.retryRun(id);
      alert(`Retry plan: ${plan.will_rerun.length} rules to rerun, ${plan.will_skip.length} to skip. New run: ${plan.new_run_id.slice(0, 8)}...`);
      loadRuns();
    } catch (err: unknown) { alert('Retry failed: ' + (err instanceof Error ? err.message : 'Unknown')); }
  };

  const handleCancel = async (id: string) => {
    try { await api.cancelRun(id); loadRuns(); } catch (err: unknown) { alert('Cancel failed: ' + (err instanceof Error ? err.message : 'Unknown')); }
  };

  const statusColor = (s: string) => s === 'success' ? 'var(--color-success)' : s === 'failed' ? 'var(--color-error)' : s === 'running' ? 'var(--color-warning)' : 'var(--color-text-tertiary)';

  return (
    <div className="page">
      <h1 className="page-title">Runs</h1>

      <div className="section">
        <div style={{ display: 'flex', justifyContent: 'space-between', alignItems: 'center', marginBottom: '0.5rem' }}>
          <h2 className="section-title" style={{ marginBottom: 0 }}>Run History</h2>
          <button className="btn-sm" onClick={loadRuns}>Refresh</button>
        </div>
        {runs.length === 0 ? (
          <div className="empty-state">No runs yet. Create a pipeline and launch it to see results here.</div>
        ) : (
          <table className="run-table">
            <thead><tr><th>ID</th><th>Pipeline</th><th>Status</th><th>Phase</th><th>Created</th><th>Actions</th></tr></thead>
            <tbody>
              {runs.map((r) => (
                <tr key={r.id}>
                  <td className="mono">{r.id.slice(0, 8)}</td>
                  <td className="mono">{r.pipeline_id?.slice(0, 8) || '-'}</td>
                  <td><span className={`status-badge ${r.status}`}>{r.status}</span></td>
                  <td>{r.phase || '-'}</td>
                  <td>{r.created_at ? new Date(r.created_at).toLocaleString() : '-'}</td>
                  <td>
                    <button className="btn-sm" onClick={() => showDetail(r)} style={{ marginRight: 4 }}>Detail</button>
                    {r.status === 'running' && <button className="btn-sm" onClick={() => handleCancel(r.id)} style={{ color: 'var(--color-error)' }}>Cancel</button>}
                    {r.status === 'failed' && <button className="btn-sm" onClick={() => handleRetry(r.id)} style={{ color: 'var(--color-warning)' }}>Retry</button>}
                  </td>
                </tr>
              ))}
            </tbody>
          </table>
        )}
      </div>

      {/* Run Detail Modal */}
      {selRun && (
        <div className="modal-overlay" onClick={() => setSelRun(null)}>
          <div className="modal" onClick={(e) => e.stopPropagation()} style={{ maxWidth: '800px' }}>
            <div style={{ display: 'flex', justifyContent: 'space-between', alignItems: 'center', marginBottom: '1rem' }}>
              <h2>Run {selRun.id.slice(0, 8)}... <span className={`status-badge ${selRun.status}`}>{selRun.status}</span></h2>
              <button className="btn-sm" onClick={() => setSelRun(null)}>Close</button>
            </div>

            <div className="detail-grid">
              <div><strong>Pipeline:</strong> {selRun.pipeline_id?.slice(0, 16) || '-'}</div>
              <div><strong>Phase:</strong> {selRun.phase || '-'}</div>
              <div><strong>Started:</strong> {selRun.started_at ? new Date(selRun.started_at).toLocaleString() : '-'}</div>
              <div><strong>Finished:</strong> {selRun.finished_at ? new Date(selRun.finished_at).toLocaleString() : '-'}</div>
            </div>

            {/* Tabs */}
            <div style={{ display: 'flex', gap: '4px', marginBottom: '0.75rem', borderBottom: '1px solid var(--color-border)', paddingBottom: '0.5rem' }}>
              {(['status', 'dag', 'diagnostics', 'logs'] as const).map((t) => (
                <button key={t} onClick={() => setTab(t)} className={tab === t ? 'btn-run' : 'btn-sm'} style={{ textTransform: 'capitalize' }}>{t === 'diagnostics' ? 'Diagnostics' : t === 'dag' ? 'DAG' : t}</button>
              ))}
            </div>

            {/* Status Tab */}
            {tab === 'status' && runStatus && (
              <div>
                <div style={{ marginBottom: '0.5rem', fontWeight: 600 }}>Nodes: {runStatus.nodes.length}</div>
                {runStatus.nodes.map((n) => (
                  <div key={n.rule} style={{ display: 'flex', alignItems: 'center', gap: '8px', padding: '4px 0', borderBottom: '1px solid var(--color-border-light)' }}>
                    <span style={{ width: 8, height: 8, borderRadius: '50%', background: statusColor(n.status), flexShrink: 0 }} />
                    <span style={{ flex: 1, fontFamily: 'var(--font-mono)', fontSize: '0.8rem' }}>{n.rule}</span>
                    <span className={`status-badge ${n.status}`}>{n.status}</span>
                    {n.duration_ms && <span style={{ fontSize: '0.75rem', color: 'var(--color-text-tertiary)' }}>{(n.duration_ms / 1000).toFixed(1)}s</span>}
                    {n.exit_code != null && n.exit_code !== 0 && <span style={{ color: 'var(--color-error)', fontSize: '0.75rem' }}>exit {n.exit_code}</span>}
                  </div>
                ))}
              </div>
            )}
            {tab === 'status' && !runStatus && <div className="empty-state">No status data available</div>}

            {/* DAG Tab */}
            {tab === 'dag' && dagStatus && (
              <div>
                <div style={{ marginBottom: '0.5rem' }}>
                  <strong>Total:</strong> {dagStatus.metrics.total_nodes} |
                  <span style={{ color: 'var(--color-success)' }}> Completed: {dagStatus.metrics.completed_nodes}</span> |
                  <span style={{ color: 'var(--color-error)' }}> Failed: {dagStatus.metrics.failed_nodes}</span> |
                  <span style={{ color: 'var(--color-warning)' }}> Running: {dagStatus.metrics.running_nodes}</span>
                  {dagStatus.metrics.eta_ms && <span> | ETA: {(dagStatus.metrics.eta_ms / 1000).toFixed(0)}s</span>}
                </div>
                <div style={{ display: 'flex', flexWrap: 'wrap', gap: '6px' }}>
                  {dagStatus.nodes.map((n) => (
                    <div key={n.id} style={{ padding: '4px 10px', borderRadius: 'var(--radius-sm)', background: n.color === 'green' ? 'var(--color-success-bg)' : n.color === 'red' ? 'var(--color-error-bg)' : n.color === 'blue' ? 'var(--color-primary-light)' : 'var(--color-bg-tertiary)', border: `1px solid ${n.color === 'green' ? 'var(--color-success)' : n.color === 'red' ? 'var(--color-error)' : n.color === 'blue' ? 'var(--color-primary)' : 'var(--color-border)'}`, fontSize: '0.8rem', fontFamily: 'var(--font-mono)' }}>
                      {n.label} {n.duration_ms ? `${(n.duration_ms / 1000).toFixed(1)}s` : ''}
                    </div>
                  ))}
                </div>
              </div>
            )}
            {tab === 'dag' && !dagStatus && <div className="empty-state">No DAG data available</div>}

            {/* Diagnostics Tab */}
            {tab === 'diagnostics' && diagnostics && (
              <div>
                {diagnostics.failed_nodes.length === 0 && diagnostics.warnings.length === 0 ? (
                  <div className="empty-state">No issues detected</div>
                ) : (
                  <>
                    {diagnostics.failed_nodes.map((fn, i) => (
                      <div key={i} style={{ background: 'var(--color-error-bg)', border: '1px solid var(--color-error)', borderRadius: 'var(--radius-md)', padding: '0.75rem', marginBottom: '0.75rem' }}>
                        <div style={{ fontWeight: 600, color: 'var(--color-error)', marginBottom: '0.25rem' }}>❌ {fn.rule} — {fn.error_pattern || 'Unknown error'}</div>
                        <div style={{ fontSize: '0.85rem', marginBottom: '0.25rem' }}><strong>Cause:</strong> {fn.likely_cause}</div>
                        {fn.suggestions.length > 0 && (
                          <div style={{ fontSize: '0.85rem' }}><strong>Suggestions:</strong>
                            <ul style={{ marginTop: '4px', paddingLeft: '1.2rem' }}>{fn.suggestions.map((s, j) => <li key={j}>{s}</li>)}</ul>
                          </div>
                        )}
                        {fn.relevant_log_lines.length > 0 && (
                          <pre style={{ background: 'var(--color-bg-tertiary)', padding: '0.5rem', borderRadius: 'var(--radius-sm)', fontSize: '0.75rem', marginTop: '0.5rem', maxHeight: '120px', overflow: 'auto', fontFamily: 'var(--font-mono)' }}>
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
                  </>
                )}
              </div>
            )}
            {tab === 'diagnostics' && !diagnostics && <div className="empty-state">No diagnostics data available</div>}

            {/* Logs Tab */}
            {tab === 'logs' && logs && <pre style={{ background: 'var(--color-bg-tertiary)', padding: '0.75rem', borderRadius: 'var(--radius-sm)', fontSize: '0.78rem', maxHeight: '400px', overflow: 'auto', fontFamily: 'var(--font-mono)', whiteSpace: 'pre-wrap', lineHeight: 1.5 }}>{logs}</pre>}
            {tab === 'logs' && !logs && <div className="empty-state">No log data available</div>}
          </div>
        </div>
      )}
    </div>
  );
}
