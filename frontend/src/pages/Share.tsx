// Public share landing page (issue #82 P0-6): a share link opens this
// read-only page — pipeline identity, DAG shape, provenance, TOML, and an
// "import into my workspace" action. No session required to VIEW; importing
// requires a login (the API enforces it).

import { useEffect, useState } from 'react';
import { useParams, useNavigate } from 'react-router-dom';
import { api, ApiError } from '../api/client';
import type { ShareLanding } from '../api/types';
import { useI18n, getLocale } from '../context/I18n';

type LandingState =
  | { phase: 'loading' }
  | { phase: 'error'; message: string }
  | { phase: 'ready'; data: ShareLanding };

export default function Share() {
  const { token } = useParams<{ token: string }>();
  const navigate = useNavigate();
  const { lang } = useI18n();
  const locale = getLocale(lang);
  const [state, setState] = useState<LandingState>(() =>
    token ? { phase: 'loading' } : { phase: 'error', message: 'Missing share token.' },
  );
  const [importing, setImporting] = useState(false);
  const [notice, setNotice] = useState<string | null>(null);

  useEffect(() => {
    if (!token) return;
    api
      .shareLanding(token)
      .then((data) => setState({ phase: 'ready', data }))
      .catch((err: unknown) => {
        const msg = err instanceof ApiError ? err.message : 'Share not found or expired.';
        setState({ phase: 'error', message: msg });
      });
  }, [token]);

  const handleImport = async () => {
    if (!token) return;
    setImporting(true);
    try {
      // The URL format matches the import API contract; the server resolves
      // the token locally.
      const url = `oxo+https://${window.location.host}/share/${token}`;
      const result = await api.importPipeline(url);
      setNotice(`Imported as a new pipeline.`);
      navigate(`/editor?pipeline=${result.pipeline_id}`);
    } catch {
      setNotice('Import failed — you may need to sign in first.');
      navigate('/login');
    } finally {
      setImporting(false);
    }
  };

  return (
    <div className="page" style={{ maxWidth: 760, margin: '0 auto', padding: '2rem 1rem' }}>
      <div style={{ marginBottom: '1.5rem' }}>
        <h1 style={{ fontSize: '1.5rem', marginBottom: '0.25rem' }}>oxo-flow · Shared Pipeline</h1>
        <p style={{ fontSize: '0.85rem', color: 'var(--color-text-secondary)' }}>
          A pipeline shared from an oxo-flow workspace. You can inspect it here and import it
          into your own workspace to run it.
        </p>
      </div>

      {state.phase === 'loading' && <div className="empty-state">Loading shared pipeline…</div>}

      {state.phase === 'error' && (
        <div className="empty-state" style={{ color: 'var(--color-error)' }}>{state.message}</div>
      )}

      {state.phase === 'ready' && (
        <>
          <div className="dash-card">
            <div style={{ display: 'flex', justifyContent: 'space-between', flexWrap: 'wrap', gap: '0.5rem' }}>
              <div>
                <h2 style={{ fontSize: '1.15rem' }}>{state.data.pipeline.name}</h2>
                <div style={{ fontSize: '0.8rem', color: 'var(--color-text-secondary)' }}>
                  Version {state.data.pipeline.version}
                  {' · '}{state.data.pipeline.rules_count} rule{state.data.pipeline.rules_count === 1 ? '' : 's'}
                  {state.data.owner ? ` · shared by ${state.data.owner}` : ''}
                </div>
              </div>
              <div style={{ display: 'flex', flexDirection: 'column', gap: '6px', alignItems: 'flex-end' }}>
                <button className="btn-run" onClick={handleImport} disabled={importing}>
                  {importing ? 'Importing…' : '⬇ Import into my workspace'}
                </button>
                {state.data.expires_at && (
                  <span style={{ fontSize: '0.75rem', color: 'var(--color-text-tertiary)' }}>
                    expires {new Date(state.data.expires_at).toLocaleDateString(locale)}
                  </span>
                )}
              </div>
            </div>
          </div>

          {state.data.recent_run && (
            <div className="dash-card" style={{ marginTop: '0.75rem' }}>
              <h4 style={{ fontSize: '0.85rem', marginBottom: '6px' }}>Most recent run</h4>
              <span className={`status-badge ${state.data.recent_run.status}`}>
                {state.data.recent_run.status}
              </span>
              {state.data.recent_run.finished_at && (
                <span style={{ fontSize: '0.8rem', color: 'var(--color-text-secondary)', marginLeft: '8px' }}>
                  {new Date(state.data.recent_run.finished_at).toLocaleString(locale)}
                </span>
              )}
            </div>
          )}

          <div className="dash-card" style={{ marginTop: '0.75rem' }}>
            <h4 style={{ fontSize: '0.85rem', marginBottom: '6px' }}>Pipeline shape</h4>
            <div style={{ display: 'flex', gap: '6px', flexWrap: 'wrap' }}>
              {state.data.dag.map((rule, i) => (
                <span key={rule} style={{ display: 'flex', alignItems: 'center', gap: '6px' }}>
                  {i > 0 && <span style={{ color: 'var(--color-text-tertiary)' }}>→</span>}
                  <span className="tag">{rule}</span>
                </span>
              ))}
            </div>
          </div>

          <details className="dash-card" style={{ marginTop: '0.75rem' }}>
            <summary style={{ cursor: 'pointer', fontSize: '0.85rem' }}>View pipeline definition (TOML)</summary>
            <pre className="log-view" style={{ marginTop: '8px', maxHeight: 320 }}>
              {state.data.toml_content}
            </pre>
          </details>
        </>
      )}

      {notice && (
        <div className="result-bar success" style={{ marginTop: '0.75rem' }}>{notice}</div>
      )}
    </div>
  );
}
