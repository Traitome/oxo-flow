import { useEffect, useState } from 'react';
import { Link, useNavigate } from 'react-router-dom';
import { Download, GitFork, Pencil, Share2, Trash2 } from 'lucide-react';
import { api } from '../api/client';
import type { Pipeline, Template } from '../api/types';
import { useI18n } from '../context/I18n';

type Tab = 'templates' | 'mine';

export default function Pipelines() {
  const { t } = useI18n();
  const navigate = useNavigate();
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
    if (!window.confirm(t('pipelines.deleteConfirm').replace('{{name}}', name))) return;
    try {
      await api.deletePipeline(id);
      setPipelines((prev) => prev.filter((p) => p.id !== id));
      setNotice(t('pipelines.deleted').replace('{{name}}', name));
    } catch (err: unknown) {
      const msg = err instanceof Error ? err.message : t('common.unknownError');
      setNotice(t('pipelines.deleteFailed').replace('{{error}}', msg));
    }
  };

  const handleShare = async (id: string) => {
    try {
      const res = await api.sharePipeline(id, 'link', 30);
      // The API returns an oxo+https:// URL; surface a clickable https://
      // link (the oxo+ scheme is the import format).
      const httpsUrl = res.share_url.replace('oxo+', '');
      await navigator.clipboard.writeText(httpsUrl);
      setNotice(t('pipelines.shareCopied').replace('{{days}}', '30') + `: ${httpsUrl}`);
    } catch {
      setNotice(t('pipelines.shareFailed'));
    }
  };

  const handleFork = async (id: string) => {
    try {
      const res = await api.forkPipeline(id);
      setNotice(t('pipelines.forked').replace('{{name}}', res.name));
      navigate(`/editor?pipeline=${res.forked_id}`);
    } catch (err: unknown) {
      const msg = err instanceof Error ? err.message : t('common.unknownError');
      setNotice(t('pipelines.forkFailed').replace('{{error}}', msg));
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
      setNotice(t('pipelines.exported').replace('{{format}}', format));
    } catch (err: unknown) {
      const msg = err instanceof Error ? err.message : t('common.unknownError');
      setNotice(t('pipelines.exportFailed').replace('{{error}}', msg));
    }
  };

  return (
    <div className="page">
      <h1 className="page-title">{t('pipelines.title')}</h1>

      <div className="left-rail-tabs" role="tablist" style={{ maxWidth: 320, marginBottom: 16 }}>
        <button
          role="tab"
          aria-selected={tab === 'templates'}
          className={`left-rail-tab ${tab === 'templates' ? 'active' : ''}`}
          onClick={() => setTab('templates')}
        >
          {t('pipelines.templates')}
        </button>
        <button
          role="tab"
          aria-selected={tab === 'mine'}
          className={`left-rail-tab ${tab === 'mine' ? 'active' : ''}`}
          onClick={() => setTab('mine')}
        >
          {t('pipelines.mine')}
        </button>
      </div>

      {notice && (
        <div className="result-bar success" style={{ cursor: 'pointer' }} onClick={() => setNotice(null)}>
          {notice}
          <span style={{ marginLeft: 'auto', fontSize: '0.7rem', opacity: 0.7 }}>{t('common.dismiss')}</span>
        </div>
      )}

      {tab === 'templates' &&
        (categories.length === 0 ? (
          <div className="empty-state">{t('pipelines.emptyTemplates')}</div>
        ) : (
          categories.map((cat) => (
            <div key={cat} className="section">
              <h2 className="section-title">{cat}</h2>
              <div className="template-grid">
                {templates
                  .filter((tpl) => tpl.category === cat)
                  .map((tpl) => (
                    <div key={tpl.id} className="template-card">
                      <h3>{tpl.name}</h3>
                      <p>{tpl.description}</p>
                      {tpl.tags.length > 0 && (
                        <div className="template-meta">
                          {tpl.tags.map((tag) => <span key={tag} className="tag">{tag}</span>)}
                        </div>
                      )}
                      <Link to={`/editor?template=${tpl.id}`} className="template-use">
                        {t('pipelines.useTemplate')}
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
              {t('pipelines.emptyMine').split(t('pipelines.emptyMineLink'))[0]}
              <Link to="/editor">{t('pipelines.emptyMineLink')}</Link>
              {t('pipelines.emptyMine').split(t('pipelines.emptyMineLink'))[1]}
            </div>
          ) : (
            <table className="pipeline-table">
              <thead>
                <tr>
                  <th>{t('pipelines.name')}</th>
                  <th>{t('pipelines.version')}</th>
                  <th>{t('pipelines.rules')}</th>
                  <th>{t('pipelines.updated')}</th>
                  <th>{t('pipelines.actions')}</th>
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
                        <Link to={`/editor?pipeline=${p.id}`} className="btn-sm" title={t('pipelines.open')}>
                          <Pencil size={13} /> {t('pipelines.open')}
                        </Link>
                        <button className="btn-sm" onClick={() => handleFork(p.id)} title={t('pipelines.fork')}>
                          <GitFork size={13} /> {t('pipelines.fork')}
                        </button>
                        <button className="btn-sm" onClick={() => handleExport(p.id, 'dockerfile')} title={t('pipelines.exportDocker')}>
                          <Download size={13} /> {t('pipelines.exportDocker')}
                        </button>
                        <button className="btn-sm" onClick={() => handleExport(p.id, 'singularity')} title={t('pipelines.exportSingularity')}>
                          <Download size={13} /> {t('pipelines.exportSingularity')}
                        </button>
                        <button className="btn-sm" onClick={() => handleShare(p.id)} title={t('pipelines.share')}>
                          <Share2 size={13} />
                        </button>
                        <button className="btn-sm btn-error" onClick={() => handleDelete(p.id, p.name)} title={t('pipelines.delete')}>
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
