import { useEffect, useState } from 'react';
import { Link } from 'react-router-dom';
import { Download, Pencil, Trash2 } from 'lucide-react';
import { api } from '../api/client';
import type { Pipeline, Template } from '../api/types';

type Tab = 'templates' | 'mine';

export default function Pipelines() {
  const [tab, setTab] = useState<Tab>('templates');
  const [templates, setTemplates] = useState<Template[]>([]);
  const [pipelines, setPipelines] = useState<Pipeline[]>([]);
  const [notice, setNotice] = useState<string | null>(null);

  useEffect(() => {
    api.listTemplates().then(setTemplates).catch(() => {});
  }, []);

  useEffect(() => {
    if (tab !== 'mine') return;
    api
      .listPipelines()
      .then(setPipelines)
      .catch(() => setPipelines([]));
  }, [tab]);

  const categories = [...new Set(templates.map((t) => t.category))];

  const handleDelete = async (id: string, name: string) => {
    if (!window.confirm(`Delete pipeline "${name}"? Its runs keep their snapshots.`)) return;
    try {
      await api.deletePipeline(id);
      setPipelines((prev) => prev.filter((p) => p.id !== id));
      setNotice(`Deleted "${name}"`);
    } catch (err: unknown) {
      const msg = err instanceof Error ? err.message : 'Delete failed';
      setNotice(`Delete failed: ${msg}`);
    }
  };

  const handleExport = async (id: string, format: 'dockerfile' | 'singularity') => {
    try {
      const res = await api.exportPipeline(id, format);
      const blob = new Blob([res.content], { type: 'text/plain' });
      const url = URL.createObjectURL(blob);
      const a = document.createElement('a');
      a.href = url;
      a.download = format === 'dockerfile' ? 'Dockerfile' : 'Singularity.def';
      a.click();
      URL.revokeObjectURL(url);
      setNotice(`Exported ${format}`);
    } catch (err: unknown) {
      const msg = err instanceof Error ? err.message : 'Export failed';
      setNotice(`Export failed: ${msg}`);
    }
  };

  return (
    <div className="page">
      <h1 className="page-title">Pipelines</h1>

      <div className="left-rail-tabs" role="tablist" style={{ maxWidth: 320, marginBottom: 16 }}>
        <button
          role="tab"
          aria-selected={tab === 'templates'}
          className={`left-rail-tab ${tab === 'templates' ? 'active' : ''}`}
          onClick={() => setTab('templates')}
        >
          Templates
        </button>
        <button
          role="tab"
          aria-selected={tab === 'mine'}
          className={`left-rail-tab ${tab === 'mine' ? 'active' : ''}`}
          onClick={() => setTab('mine')}
        >
          My Pipelines
        </button>
      </div>

      {notice && (
        <div className="result-bar success" style={{ cursor: 'pointer' }} onClick={() => setNotice(null)}>
          {notice}
          <span style={{ marginLeft: 'auto', fontSize: '0.7rem', opacity: 0.7 }}>click to dismiss</span>
        </div>
      )}

      {tab === 'templates' &&
        (categories.length === 0 ? (
          <div className="empty-state">No templates available.</div>
        ) : (
          categories.map((cat) => (
            <div key={cat} className="section">
              <h2 className="section-title">{cat}</h2>
              <div className="template-grid">
                {templates
                  .filter((t) => t.category === cat)
                  .map((t) => (
                    <div key={t.id} className="template-card">
                      <h3>{t.name}</h3>
                      <p>{t.description}</p>
                      <div className="template-meta">
                        <span className="tag">{t.tags}</span>
                      </div>
                      <Link to={`/editor?template=${t.id}`} className="template-use">
                        Use Template
                      </Link>
                    </div>
                  ))}
              </div>
            </div>
          ))
        ))}

      {tab === 'mine' && (
        <>
          {pipelines.length === 0 ? (
            <div className="empty-state">
              No saved pipelines yet — build one on the{' '}
              <Link to="/editor">editor canvas</Link>, then Save.
            </div>
          ) : (
            <table className="pipeline-table">
              <thead>
                <tr>
                  <th>Name</th>
                  <th>Version</th>
                  <th>Rules</th>
                  <th>Updated</th>
                  <th>Actions</th>
                </tr>
              </thead>
              <tbody>
                {pipelines.map((p) => (
                  <tr key={p.id}>
                    <td>{p.name}</td>
                    <td>{p.version}</td>
                    <td>{p.rules_count}</td>
                    <td>{p.updated_at.slice(0, 10)}</td>
                    <td>
                      <span className="pipeline-actions">
                        <Link to={`/editor?pipeline=${p.id}`} className="btn-sm" title="Open in editor">
                          <Pencil size={13} /> Open
                        </Link>
                        <button className="btn-sm" onClick={() => handleExport(p.id, 'dockerfile')} title="Export Dockerfile">
                          <Download size={13} /> Docker
                        </button>
                        <button className="btn-sm" onClick={() => handleExport(p.id, 'singularity')} title="Export Singularity definition">
                          <Download size={13} /> Singularity
                        </button>
                        <button className="btn-sm btn-error" onClick={() => handleDelete(p.id, p.name)} title="Delete">
                          <Trash2 size={13} />
                        </button>
                      </span>
                    </td>
                  </tr>
                ))}
              </tbody>
            </table>
          )}
        </>
      )}
    </div>
  );
}
