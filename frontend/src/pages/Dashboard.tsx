import { useEffect, useState } from 'react';
import { Link, useNavigate } from 'react-router-dom';
import { Wand2, ArrowRight } from 'lucide-react';
import { api } from '../api/client';
import type { HealthResponse, SystemInfo, RunItem, Template } from '../api/types';

export default function Dashboard() {
  const navigate = useNavigate();
  const [health, setHealth] = useState<HealthResponse | null>(null);
  const [sys, setSys] = useState<SystemInfo | null>(null);
  const [runs, setRuns] = useState<RunItem[]>([]);
  const [templates, setTemplates] = useState<Template[]>([]);
  const [intent, setIntent] = useState('');
  const [generating, setGenerating] = useState(false);
  const [genResult, setGenResult] = useState<string | null>(null);

  useEffect(() => {
    api.health().then(setHealth).catch(() => {});
    api.system().then(setSys).catch(() => {});
    api.listRuns().then(setRuns).catch(() => {});
    api.listTemplates().then(setTemplates).catch(() => {});
  }, []);

  const activeRuns = runs.filter((r) => r.status === 'running' || r.status === 'queued').length;

  const handleGenerate = async () => {
    if (!intent.trim()) return;
    setGenerating(true);
    setGenResult(null);
    try {
      const result = await api.aiTranslate(intent);
      setGenResult(result.pipeline_id);
      navigate(`/editor?intent=${encodeURIComponent(intent)}`);
    } catch (err: unknown) {
      const msg = err instanceof Error ? err.message : 'Generation failed';
      setGenResult(`Error: ${msg}`);
    }
    setGenerating(false);
  };

  const quickTemplates = templates.slice(0, 4);

  return (
    <div className="page">
      <h1 className="page-title">oxo-flow Command Center</h1>
      <p className="page-subtitle">Bioinformatics Pipelines, Simply Powerful</p>

      {/* Intent Composer */}
      <div className="intent-composer">
        <div className="intent-input-wrapper">
          <Wand2 size={20} className="intent-icon" />
          <input
            type="text"
            placeholder="Describe the analysis you need... e.g., 'RNA-seq, PE, hg38, STAR + featureCounts, strand-specific'"
            value={intent}
            onChange={(e) => setIntent(e.target.value)}
            onKeyDown={(e) => e.key === 'Enter' && handleGenerate()}
            className="intent-input"
          />
          <button onClick={handleGenerate} disabled={generating || !intent.trim()} className="intent-btn">
            {generating ? 'Generating...' : 'Generate Pipeline'}
            <ArrowRight size={16} />
          </button>
        </div>
        {genResult && genResult.startsWith('Error') && <div className="intent-error">{genResult}</div>}
        <div className="intent-hint">
          <span>Or <Link to="/editor">write TOML directly</Link></span>
        </div>
      </div>

      {/* Quick Start + Recent */}
      <div className="dashboard-grid">
        <div className="dash-card">
          <h3 className="dash-card-title">Quick Templates</h3>
          <div className="quick-templates">
            {quickTemplates.map((t) => (
              <button key={t.id} className="qt-btn" onClick={() => navigate(`/editor?template=${t.id}`)}>
                <span className="qt-name">{t.name}</span>
                <span className="qt-cat">{t.category}</span>
              </button>
            ))}
            <Link to="/templates" className="qt-btn qt-more">View all templates →</Link>
          </div>
        </div>
        <div className="dash-card">
          <h3 className="dash-card-title">Recent Runs</h3>
          {runs.length === 0 ? (
            <div className="empty-state">No runs yet. Create a pipeline to get started.</div>
          ) : (
            <table className="run-table">
              <thead><tr><th>ID</th><th>Status</th><th>Started</th><th></th></tr></thead>
              <tbody>
                {runs.slice(0, 5).map((r) => (
                  <tr key={r.id}>
                    <td><code>{r.id.slice(0, 8)}...</code></td>
                    <td><span className={`status-badge ${r.status}`}>{r.status}</span></td>
                    <td>{r.started_at ? new Date(r.started_at).toLocaleString() : '-'}</td>
                    <td><Link to={`/runs/${r.id}`} className="view-link">View</Link></td>
                  </tr>
                ))}
              </tbody>
            </table>
          )}
        </div>
      </div>

      {/* Stats */}
      <div className="stat-grid">
        <div className="stat-card"><div className="stat-value">{health?.version || '-'}</div><div className="stat-label">Version</div></div>
        <div className="stat-card"><div className="stat-value">{runs.length}</div><div className="stat-label">Total Runs</div></div>
        <div className="stat-card"><div className="stat-value" style={{ color: activeRuns > 0 ? '#D97706' : '#059669' }}>{activeRuns}</div><div className="stat-label">Active</div></div>
        <div className="stat-card"><div className="stat-value">{sys ? `${sys.os}/${sys.arch}` : '-'}</div><div className="stat-label">Platform</div></div>
      </div>
    </div>
  );
}
