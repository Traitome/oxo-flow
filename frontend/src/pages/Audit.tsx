import { useEffect, useMemo, useState } from 'react';
import { Download } from 'lucide-react';
import { api } from '../api/client';
import type { AuditLogResponse } from '../api/types';
import { useI18n, getLocale } from '../context/I18n';

const PER_PAGE_OPTIONS = [25, 50, 100];

// Audit trail (issue #79 P1-05): every state-changing request is recorded
// server-side; this page renders the trail with server-side pagination and
// CSV export for compliance workflows.
export default function Audit() {
  const { lang, t } = useI18n();
  const [data, setData] = useState<AuditLogResponse | null>(null);
  const [days, setDays] = useState(7);
  const [page, setPage] = useState(1);
  const [perPage, setPerPage] = useState(50);
  const [error, setError] = useState<string | null>(null);

  const setDaysAndReset = (d: number) => {
    setDays(d);
    setPage(1);
  };
  const setPerPageAndReset = (n: number) => {
    setPerPage(n);
    setPage(1);
  };

  useEffect(() => {
    api
      .audit(days, page, perPage)
      .then(setData)
      .catch((e: unknown) => setError(e instanceof Error ? e.message : t('audit.loadFailed')));
  }, [days, page, perPage]);

  const totalPages = useMemo(() => {
    if (!data) return 1;
    return Math.max(1, Math.ceil(data.total / data.per_page));
  }, [data]);

  const exportCsv = () => {
    const entries = data?.entries ?? [];
    if (entries.length === 0) return;
    const csvCell = (v: string | null | undefined) => `"${String(v ?? '').replace(/"/g, '""')}"`;
    const rows = [
      ['timestamp', 'user', 'action', 'resource', 'result'].join(','),
      ...entries.map((e) =>
        [csvCell(e.timestamp), csvCell(e.user), csvCell(e.action), csvCell(e.resource), csvCell(e.result)].join(','),
      ),
    ];
    const blob = new Blob([rows.join('\n')], { type: 'text/csv;charset=utf-8;' });
    const url = URL.createObjectURL(blob);
    const a = document.createElement('a');
    a.href = url;
    a.download = `audit-${new Date().toISOString().slice(0, 10)}.csv`;
    a.click();
    URL.revokeObjectURL(url);
  };

  return (
    <div className="page">
      <h1 className="page-title">{t('audit.title')}</h1>
      <p className="page-subtitle">{t('audit.subtitle')}</p>

      <div style={{ display: 'flex', gap: '1rem', alignItems: 'flex-end', flexWrap: 'wrap', marginBottom: '1rem' }}>
        <label className="inspector-field" style={{ maxWidth: 200 }}>
          <span>{t('audit.lookback')}</span>
          <select value={days} onChange={(e) => setDaysAndReset(Number(e.target.value))} className="search-input">
            {[1, 3, 7, 14, 30].map((d) => (
              <option key={d} value={d}>
                {d}
              </option>
            ))}
          </select>
        </label>

        <label className="inspector-field" style={{ maxWidth: 140 }}>
          <span>{t('audit.perPage')}</span>
          <select value={perPage} onChange={(e) => setPerPageAndReset(Number(e.target.value))} className="search-input">
            {PER_PAGE_OPTIONS.map((n) => (
              <option key={n} value={n}>
                {n}
              </option>
            ))}
          </select>
        </label>

        <button className="btn-sm" onClick={exportCsv} disabled={!data || data.entries.length === 0}>
          <Download size={14} /> {t('audit.export')}
        </button>
      </div>

      {error && <div className="tool-palette-hint error">{error}</div>}

      <div className="overflow-x">
        <table className="data-table">
          <thead>
            <tr>
              <th>{t('audit.time')}</th>
              <th>{t('audit.user')}</th>
              <th>{t('audit.action')}</th>
              <th>{t('audit.resource')}</th>
              <th>{t('audit.result')}</th>
            </tr>
          </thead>
          <tbody>
            {(data?.entries ?? []).map((e, i) => (
              <tr key={i}>
                <td>{new Date(e.timestamp).toLocaleString(getLocale(lang))}</td>
                <td>{e.user}</td>
                <td>{e.action}</td>
                <td>{e.resource}</td>
                <td>
                  <span className={`status-badge ${e.result === 'success' ? 'completed' : 'failed'}`}>
                    {e.result}
                  </span>
                </td>
              </tr>
            ))}
            {(data?.entries ?? []).length === 0 && !error && (
              <tr>
                <td colSpan={5} style={{ textAlign: 'center', opacity: 0.6 }}>
                  {t('audit.empty').replace('{{days}}', String(days))}
                </td>
              </tr>
            )}
          </tbody>
        </table>
      </div>

      {data && data.total > 0 && (
        <div style={{ display: 'flex', alignItems: 'center', gap: '1rem', marginTop: '1rem', flexWrap: 'wrap' }}>
          <button className="btn-sm" onClick={() => setPage((p) => Math.max(1, p - 1))} disabled={page <= 1}>
            {t('audit.prev')}
          </button>
          <span style={{ fontSize: '0.85rem', color: 'var(--color-text-secondary)' }}>
            {t('audit.page')} {data.page} {t('audit.of')} {totalPages} {t('audit.page')} ({t('audit.total').replace('{{total}}', String(data.total))})
          </span>
          <button
            className="btn-sm"
            onClick={() => setPage((p) => Math.min(totalPages, p + 1))}
            disabled={page >= totalPages}
          >
            {t('audit.next')}
          </button>
        </div>
      )}
    </div>
  );
}
