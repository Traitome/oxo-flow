import { useEffect, useState } from 'react';
import { Link, useNavigate } from 'react-router-dom';
import { Download, GitFork, Pencil, Share2, Trash2, Upload, GitCompare, BookmarkPlus, Search } from 'lucide-react';
import { api, describeApiError } from '../api/client';
import type { Pipeline, Template, DiffEntry } from '../api/types';
import { useI18n } from '../context/I18n';
import Modal from '../components/Modal';

type Tab = 'templates' | 'mine';

export default function Pipelines() {
  const { t } = useI18n();
  const navigate = useNavigate();
  const [tab, setTab] = useState<Tab>('templates');
  const [templates, setTemplates] = useState<Template[]>([]);
  const [pipelines, setPipelines] = useState<Pipeline[]>([]);
  const [notice, setNotice] = useState<string | null>(null);
  // A failed load used to render as an empty list — indistinguishable from
  // "you have no templates/pipelines".
  const [error, setError] = useState<string | null>(null);
  // #907 D2: the styled delete confirm replaces window.confirm (which no
  // dialog can style and which block event loops in embedded webviews).
  const [deleteTarget, setDeleteTarget] = useState<Pipeline | null>(null);
  const [query, setQuery] = useState('');
  const [activeCat, setActiveCat] = useState('');
  const [showImportModal, setShowImportModal] = useState(false);
  const [importUrl, setImportUrl] = useState('');
  const [diffSource, setDiffSource] = useState<Pipeline | null>(null);
  const [diffTargetId, setDiffTargetId] = useState('');
  const [diffEntries, setDiffEntries] = useState<DiffEntry[] | null>(null);

  useEffect(() => {
    api
      .listTemplates()
      .then((tpl) => { setTemplates(tpl); setError(null); })
      .catch((err: unknown) => setError(t('common.loadFailed').replace('{{error}}', err instanceof Error ? err.message : t('common.unknownError'))));
  }, [t]);

  useEffect(() => {
    if (tab !== 'mine') return;
    api
      .listPipelines()
      .then((pl) => { setPipelines(pl); setError(null); })
      .catch((err: unknown) => {
        setPipelines([]);
        setError(t('common.loadFailed').replace('{{error}}', err instanceof Error ? err.message : t('common.unknownError')));
      });
  }, [tab, t]);

  const categories = [...new Set(templates.map((t) => t.category))];
  const filteredTemplates = templates.filter(
    (tpl) =>
      (!activeCat || tpl.category === activeCat) &&
      (!query.trim() ||
        tpl.name.toLowerCase().includes(query.toLowerCase()) ||
        tpl.description.toLowerCase().includes(query.toLowerCase()) ||
        tpl.tags.some((tg) => tg.toLowerCase().includes(query.toLowerCase()))),
  );
  const visibleCategories = [...new Set(filteredTemplates.map((t) => t.category))];
  const filteredPipelines = pipelines.filter(
    (p) => !query.trim() || p.name.toLowerCase().includes(query.toLowerCase()),
  );

  const handleDelete = async (id: string, name: string) => {
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
    // The share API call and the clipboard write are separate failure
    // domains: the token IS created server-side even when the clipboard
    // write fails (plain-HTTP LAN deployments have no navigator.clipboard),
    // and reporting that as "share failed" made users retry and mint
    // duplicate tokens (audit #654).
    let httpsUrl: string;
    try {
      const res = await api.sharePipeline(id, 'link', 30);
      // The API returns an oxo+http:// or oxo+https:// URL (scheme follows
      // the actual serving transport, #907 R16); surface a clickable
      // http(s):// link — stripping `oxo+` leaves the browser URL intact.
      httpsUrl = res.share_url.replace('oxo+', '');
    } catch (err: unknown) {
      setNotice(t('pipelines.shareFailed') + `: ${describeApiError(err, t('common.unknownError'))}`);
      return;
    }
    try {
      await navigator.clipboard.writeText(httpsUrl);
    } catch {
      // Clipboard unavailable (plain-HTTP deployment) — the URL is shown
      // in the notice either way.
    }
    setNotice(t('pipelines.shareCopied').replace('{{days}}', '30') + `: ${httpsUrl}`);
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
      <div style={{ display: 'flex', alignItems: 'center', justifyContent: 'space-between', gap: '0.75rem', flexWrap: 'wrap', marginBottom: '0.75rem' }}>
        <h1 className="page-title" style={{ margin: 0 }}>{t('pipelines.title')}</h1>
        <div className="row">
          <button className="btn-sm" onClick={() => setShowImportModal(true)}>
            <Upload size={14} /> {t('pipelines.importUrl')}
          </button>
        </div>
      </div>

      <div style={{ display: 'flex', alignItems: 'center', justifyContent: 'space-between', gap: '0.75rem', flexWrap: 'wrap', marginBottom: 16 }}>
        <div className="left-rail-tabs" role="tablist" style={{ maxWidth: 320, flex: '1 1 260px' }}>
          <button
            role="tab"
            aria-selected={tab === 'templates'}
            className={`left-rail-tab ${tab === 'templates' ? 'active' : ''}`}
            onClick={() => setTab('templates')}
          >
            {t('pipelines.templates')} ({templates.length})
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
        <div style={{ display: 'flex', alignItems: 'center', gap: 6, flex: '1 1 260px', maxWidth: 420 }}>
          <Search size={15} color="var(--color-text-tertiary)" />
          <input
            className="search-input"
            style={{ flex: 1 }}
            placeholder={t('pipelines.searchPlaceholder')}
            value={query}
            onChange={(e) => setQuery(e.target.value)}
          />
        </div>
      </div>

      {tab === 'templates' && categories.length > 1 && (
        <div className="pill-bar">
          <button type="button" className={`pill-chip ${activeCat === '' ? 'active' : ''}`} onClick={() => setActiveCat('')}>
            All ({templates.length})
          </button>
          {categories.map((cat) => (
            <button key={cat} type="button" className={`pill-chip ${activeCat === cat ? 'active' : ''}`} onClick={() => setActiveCat(cat)}>
              {cat}
            </button>
          ))}
        </div>
      )}

      {notice && (
        <div className="result-bar success" style={{ cursor: 'pointer' }} onClick={() => setNotice(null)}>
          {notice}
          <span style={{ marginLeft: 'auto', fontSize: '0.7rem', opacity: 0.7 }}>{t('common.dismiss')}</span>
        </div>
      )}

      {error && <div className="tool-palette-hint error" style={{ cursor: 'pointer' }} onClick={() => setError(null)}>{error}</div>}

      {tab === 'templates' &&
        (visibleCategories.length === 0 ? (
          <div className="empty-state">{t('pipelines.emptyTemplates')}</div>
        ) : (
          visibleCategories.map((cat) => (
            <div key={cat} className="section">
              <h2 className="section-title">{cat}</h2>
              <div className="template-grid">
                {filteredTemplates
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
                {filteredPipelines.map((p) => (
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
                        <button
                          className="btn-sm"
                          title={t('pipelines.saveAsTemplate')}
                          onClick={async () => {
                            try {
                              const full = await api.getPipeline(p.id);
                              await api.saveTemplate({
                                name: full.name,
                                category: 'Custom',
                                description: `Saved from pipeline ${full.name} (v${full.version})`,
                                tags: ['custom'],
                                toml_content: full.toml_content,
                              });
                              const tpl = await api.listTemplates();
                              setTemplates(tpl);
                              setNotice(`Saved "${full.name}" as reusable template`);
                            } catch (err: unknown) {
                              setNotice(describeApiError(err, 'Save template failed'));
                            }
                          }}
                        >
                          <BookmarkPlus size={13} /> {t('pipelines.saveAsTemplate')}
                        </button>
                        {pipelines.length > 1 && (
                          <button
                            className="btn-sm"
                            title={t('pipelines.diffWith')}
                            onClick={() => {
                              setDiffSource(p);
                              const other = pipelines.find((x) => x.id !== p.id);
                              setDiffTargetId(other?.id ?? '');
                              setDiffEntries(null);
                            }}
                          >
                            <GitCompare size={13} /> {t('pipelines.diffWith')}
                          </button>
                        )}
                        <button className="btn-sm" onClick={() => handleExport(p.id, 'dockerfile')} title={t('pipelines.exportDocker')}>
                          <Download size={13} /> {t('pipelines.exportDocker')}
                        </button>
                        <button className="btn-sm" onClick={() => handleExport(p.id, 'singularity')} title={t('pipelines.exportSingularity')}>
                          <Download size={13} /> {t('pipelines.exportSingularity')}
                        </button>
                        <button className="btn-sm" onClick={() => handleShare(p.id)} title={t('pipelines.share')} aria-label={t('pipelines.share')}>
                          <Share2 size={13} />
                        </button>
                        <button className="btn-sm btn-error" onClick={() => setDeleteTarget(p)} title={t('pipelines.delete')} aria-label={t('pipelines.delete')}>
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

      {deleteTarget && (
        <Modal onClose={() => setDeleteTarget(null)} labelledBy="delete-confirm-title">
          <h3 id="delete-confirm-title" style={{ marginTop: 0 }}>{t('pipelines.delete')}</h3>
          <p>{t('pipelines.deleteConfirm').replace('{{name}}', deleteTarget.name)}</p>
          <div className="modal-actions">
            <button className="btn-sm" onClick={() => setDeleteTarget(null)}>
              {t('inspector.cancel')}
            </button>
            <button
              className="btn-sm btn-error"
              onClick={() => {
                void handleDelete(deleteTarget.id, deleteTarget.name);
                setDeleteTarget(null);
              }}
            >
              <Trash2 size={13} /> {t('pipelines.delete')}
            </button>
          </div>
        </Modal>
      )}

      {showImportModal && (
        <Modal onClose={() => setShowImportModal(false)} labelledBy="import-share-title">
          <h3 id="import-share-title" style={{ marginTop: 0 }}>{t('pipelines.importModalTitle')}</h3>
          <input
            className="mono"
            placeholder={t('pipelines.importPlaceholder')}
            value={importUrl}
            onChange={(e) => setImportUrl(e.target.value)}
          />
          <div className="modal-actions">
            <button className="btn-sm" onClick={() => setShowImportModal(false)}>{t('inspector.cancel')}</button>
            <button
              className="btn-run"
              disabled={!importUrl.trim()}
              onClick={async () => {
                try {
                  const raw = importUrl.trim();
                  const normalized = raw.startsWith('http://') || raw.startsWith('https://') ? `oxo+${raw}` : raw;
                  const res = await api.importPipeline(normalized);
                  setShowImportModal(false);
                  setImportUrl('');
                  setNotice(`Imported pipeline "${res.name}"`);
                  navigate(`/editor?pipeline=${res.pipeline_id}`);
                } catch (err: unknown) {
                  setError(describeApiError(err, t('share.importFailed')));
                }
              }}
            >
              <Upload size={14} /> {t('pipelines.importUrl')}
            </button>
          </div>
        </Modal>
      )}

      {diffSource && (
        <Modal onClose={() => setDiffSource(null)} labelledBy="diff-modal-title">
          <h3 id="diff-modal-title" style={{ marginTop: 0 }}>{t('pipelines.diffWith')}: {diffSource.name}</h3>
          <label className="inspector-field" style={{ marginBottom: 10 }}>
            <span>Compare against pipeline</span>
            <select value={diffTargetId} onChange={(e) => setDiffTargetId(e.target.value)}>
              {pipelines.filter((x) => x.id !== diffSource.id).map((x) => (
                <option key={x.id} value={x.id}>{x.name} (v{x.version})</option>
              ))}
            </select>
          </label>
          <div className="row" style={{ marginBottom: 10 }}>
            <button
              className="btn-run"
              disabled={!diffTargetId}
              onClick={async () => {
                try {
                  const res = await api.diffPipelines({ pipeline_a_id: diffSource.id, pipeline_b_id: diffTargetId });
                  setDiffEntries(res.diffs);
                } catch (err: unknown) {
                  setError(describeApiError(err, 'Diff failed'));
                }
              }}
            >
              <GitCompare size={14} /> Run Diff
            </button>
          </div>
          {diffEntries && (
            diffEntries.length === 0 ? (
              <div className="result-bar success">Identical pipeline definitions — 0 structural differences.</div>
            ) : (
              <div className="overflow-x" style={{ maxHeight: 220 }}>
                <table className="data-table">
                  <thead><tr><th>Path</th><th>Category</th><th>Change</th></tr></thead>
                  <tbody>
                    {diffEntries.map((d, i) => (
                      <tr key={i}>
                        <td className="mono">{d.path}</td>
                        <td><span className="tag">{d.category}</span></td>
                        <td>{d.description}</td>
                      </tr>
                    ))}
                  </tbody>
                </table>
              </div>
            )
          )}
          <div className="modal-actions" style={{ marginTop: 12 }}>
            <button className="btn-sm" onClick={() => setDiffSource(null)}>{t('common.close')}</button>
          </div>
        </Modal>
      )}
    </div>
  );
}
