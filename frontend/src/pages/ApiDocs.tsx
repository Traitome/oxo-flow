import { useEffect, useState } from 'react';

interface Endpoint {
  method: string; path: string; summary: string; tags: string[];
}

export default function ApiDocs() {
  const [spec, setSpec] = useState<Record<string, unknown> | null>(null);
  const [endpoints, setEndpoints] = useState<Endpoint[]>([]);
  const [search, setSearch] = useState('');
  const [selectedTag, setSelectedTag] = useState('');

  useEffect(() => {
    fetch('/api/openapi.json')
      .then((r) => r.json())
      .then((data) => {
        setSpec(data);
        const eps: Endpoint[] = [];
        if (data.paths) {
          for (const [path, methods] of Object.entries(data.paths as Record<string, Record<string, unknown>>)) {
            for (const [method, detail] of Object.entries(methods)) {
              if (method === 'parameters') continue;
              eps.push({
                method: method.toUpperCase(),
                path,
                summary: (detail as Record<string, unknown>)?.summary as string || '',
                tags: ((detail as Record<string, unknown>)?.tags as string[]) || [],
              });
            }
          }
        }
        setEndpoints(eps);
      })
      .catch(() => {});
  }, []);

  const tags = [...new Set(endpoints.flatMap((e) => e.tags))];
  const filtered = endpoints.filter(
    (e) =>
      (!search || e.path.toLowerCase().includes(search.toLowerCase()) || e.summary.toLowerCase().includes(search.toLowerCase())) &&
      (!selectedTag || e.tags.includes(selectedTag))
  );

  const methodColor = (m: string) => m === 'GET' ? 'var(--color-success)' : m === 'POST' ? 'var(--color-primary)' : m === 'PUT' ? 'var(--color-warning)' : m === 'DELETE' ? 'var(--color-error)' : 'var(--color-text-secondary)';

  return (
    <div className="page">
      <h1 className="page-title">API Reference</h1>
      <p className="page-subtitle" style={{ marginBottom: '1rem' }}>
        All 53 endpoints across 8 domains. OpenAPI 3.1 spec available at{' '}
        <a href="/api/openapi.json" target="_blank" style={{ color: 'var(--color-primary)' }}>/api/openapi.json</a>.
      </p>

      <div style={{ display: 'flex', gap: '0.5rem', marginBottom: '1rem', flexWrap: 'wrap' }}>
        <input
          type="text" placeholder="Search endpoints..." value={search}
          onChange={(e) => setSearch(e.target.value)}
          style={{ flex: 1, minWidth: '200px', padding: '0.5rem 0.75rem', border: '1px solid var(--color-border)', borderRadius: 'var(--radius-sm)', fontSize: '0.9rem', background: 'var(--color-bg)', color: 'var(--color-text)' }}
        />
        <select value={selectedTag} onChange={(e) => setSelectedTag(e.target.value)}
          style={{ padding: '0.5rem', border: '1px solid var(--color-border)', borderRadius: 'var(--radius-sm)', fontSize: '0.9rem', background: 'var(--color-bg)', color: 'var(--color-text)' }}>
          <option value="">All Domains</option>
          {tags.map((t) => (<option key={t} value={t}>{t}</option>))}
        </select>
      </div>

      <div style={{ display: 'flex', flexDirection: 'column', gap: '4px' }}>
        {filtered.map((ep, i) => (
          <div key={i} style={{ display: 'flex', alignItems: 'center', gap: '0.75rem', padding: '0.5rem 0.75rem', border: '1px solid var(--color-border-light)', borderRadius: 'var(--radius-sm)', background: 'var(--color-bg)' }}>
            <span style={{ fontWeight: 700, fontSize: '0.75rem', color: methodColor(ep.method), minWidth: '52px', fontFamily: 'var(--font-mono)' }}>{ep.method}</span>
            <code style={{ fontSize: '0.82rem', fontFamily: 'var(--font-mono)', flex: 1 }}>{ep.path}</code>
            <span style={{ fontSize: '0.8rem', color: 'var(--color-text-secondary)', minWidth: '80px', textAlign: 'right' }}>{ep.tags.join(', ')}</span>
            <span style={{ fontSize: '0.78rem', color: 'var(--color-text-tertiary)', minWidth: '120px', textAlign: 'right' }}>{ep.summary}</span>
          </div>
        ))}
      </div>

      {filtered.length === 0 && <div className="empty-state">No endpoints match</div>}

      {spec && (
        <details style={{ marginTop: '2rem' }}>
          <summary style={{ cursor: 'pointer', fontSize: '0.85rem', color: 'var(--color-text-secondary)' }}>Raw OpenAPI Spec</summary>
          <pre style={{ background: 'var(--color-bg-tertiary)', padding: '1rem', borderRadius: 'var(--radius-sm)', fontSize: '0.75rem', fontFamily: 'var(--font-mono)', maxHeight: '400px', overflow: 'auto', marginTop: '0.5rem' }}>
            {JSON.stringify(spec, null, 2)}
          </pre>
        </details>
      )}
    </div>
  );
}
