import { useEffect, useState } from 'react';
import { Trash2, Server, Activity } from 'lucide-react';
import { api } from '../api/client';
import type { ClusterInfo } from '../api/types';
import { useI18n } from '../context/I18n';

// Defensive sanitizer for probe errors: if the backend leaks a hostname or
// IP address in an error message, replace it with a generic token before
// displaying it in the UI.
const HOST_OR_IP_RE = /\b(?:\d{1,3}\.){3}\d{1,3}\b|(?:[a-zA-Z0-9-]+\.)+[a-zA-Z]{2,}\b/g;
function sanitizeClusterError(text: string): string {
  return text.replace(HOST_OR_IP_RE, '[redacted]');
}

// Cluster connections (SSH endpoints) — the "app anywhere, cluster
// elsewhere" configuration surface: define remote servers/clusters,
// probe them over SSH (connectivity + scheduler detection), and remove
// them. Definitions can also be seeded from the platform config file
// (oxo-flow.web.toml [[clusters]]), imported idempotently at startup.
export default function Clusters() {
  const { t } = useI18n();
  const [clusters, setClusters] = useState<ClusterInfo[]>([]);
  const [error, setError] = useState<string | null>(null);
  const [notice, setNotice] = useState<string | null>(null);
  const [probeResults, setProbeResults] = useState<Record<string, { ok: boolean; text: string }>>({});
  const [form, setForm] = useState({
    id: '', name: '', ssh_host: '', ssh_port: '22', ssh_user: '',
    ssh_key: '', scheduler: 'auto', remote_dir: '',
  });

  const reload = () => {
    api
      .listClusters()
      .then(setClusters)
      .catch((e: unknown) => setError(e instanceof Error ? e.message : t('clusters.loadFailed')));
  };
  useEffect(reload, []);

  const handleSave = async (e: React.FormEvent) => {
    e.preventDefault();
    setError(null);
    setNotice(null);
    try {
      await api.upsertCluster({
        id: form.id.trim(),
        name: form.name.trim(),
        ssh_host: form.ssh_host.trim(),
        ssh_port: Number(form.ssh_port) || 22,
        ssh_user: form.ssh_user.trim() || undefined,
        ssh_key: form.ssh_key.trim() || undefined,
        scheduler: form.scheduler || 'auto',
        remote_dir: form.remote_dir.trim() || undefined,
        enabled: true,
      });
      setNotice(t('clusters.saved').replace('{{name}}', form.name.trim()));
      setForm({ ...form, id: '', name: '', ssh_host: '', ssh_user: '', ssh_key: '', remote_dir: '' });
      reload();
    } catch (err: unknown) {
      setError(err instanceof Error ? err.message : t('clusters.saveFailed'));
    }
  };

  const handleProbe = async (id: string) => {
    setProbeResults((prev) => ({ ...prev, [id]: { ok: false, text: t('clusters.probing') } }));
    try {
      const r = await api.probeCluster(id);
      const text = r.ok
        ? t('clusters.probeResult')
            .replace('{{hostname}}', r.hostname ?? '')
            .replace('{{scheduler}}', r.scheduler ?? 'none')
            .replace('{{version}}', r.version ? ` · ${r.version}` : '')
            .replace('{{duration}}', String(r.duration_ms))
        : sanitizeClusterError(t('clusters.probeFailed').replace('{{error}}', r.error ?? t('common.unknownError')));
      setProbeResults((prev) => ({ ...prev, [id]: { ok: r.ok, text } }));
    } catch (err: unknown) {
      setProbeResults((prev) => ({
        ...prev,
        [id]: { ok: false, text: sanitizeClusterError(err instanceof Error ? err.message : t('clusters.loadFailed')) },
      }));
    }
  };

  const handleDelete = async (id: string, name: string) => {
    if (!window.confirm(t('clusters.removeConfirm').replace('{{name}}', name))) return;
    try {
      await api.deleteCluster(id);
      setNotice(t('clusters.removed').replace('{{name}}', name));
      reload();
    } catch (err: unknown) {
      setError(err instanceof Error ? err.message : t('clusters.removeFailed'));
    }
  };

  const field = (label: string, value: string, onChange: (v: string) => void, placeholder?: string, type = 'text') => (
    <label className="inspector-field" style={{ flex: 1, minWidth: 150 }}>
      <span>{label}</span>
      <input type={type} value={value} placeholder={placeholder} autoComplete="off" onChange={(e) => onChange(e.target.value)} />
    </label>
  );

  return (
    <div className="page">
      <h1 className="page-title">{t('clusters.title')}</h1>
      <p className="page-subtitle">{t('clusters.subtitle')}</p>

      {notice && <div className="tool-palette-hint">{notice}</div>}
      {error && <div className="tool-palette-hint error">{error}</div>}

      <form onSubmit={handleSave} className="login-form" style={{ maxWidth: 860, margin: '0 0 1.5rem' }}>
        <div style={{ display: 'flex', gap: 8, flexWrap: 'wrap' }}>
          {field(t('clusters.id'), form.id, (v) => setForm({ ...form, id: v }), 'lab-slurm')}
          {field(t('clusters.name'), form.name, (v) => setForm({ ...form, name: v }), 'Lab SLURM cluster')}
          {field(t('clusters.sshHost'), form.ssh_host, (v) => setForm({ ...form, ssh_host: v }), 'login.lab.example.edu')}
          {field(t('clusters.sshPort'), form.ssh_port, (v) => setForm({ ...form, ssh_port: v }), '22')}
          {field(t('clusters.sshUser'), form.ssh_user, (v) => setForm({ ...form, ssh_user: v }), 'bioinf')}
          {field(t('clusters.sshKey'), form.ssh_key, (v) => setForm({ ...form, ssh_key: v }), '~/.ssh/id_ed25519')}
          <label className="inspector-field" style={{ flex: 1, minWidth: 130 }}>
            <span>{t('clusters.scheduler')}</span>
            <select value={form.scheduler} onChange={(e) => setForm({ ...form, scheduler: e.target.value })}>
              {['auto', 'slurm', 'pbs', 'lsf', 'sge'].map((s) => (
                <option key={s} value={s}>{s}</option>
              ))}
            </select>
          </label>
          {field(t('clusters.remoteDir'), form.remote_dir, (v) => setForm({ ...form, remote_dir: v }), '~/oxo-flow-jobs')}
        </div>
        <button className="btn-run" type="submit" disabled={!form.id.trim() || !form.name.trim() || !form.ssh_host.trim()}>
          <Server size={14} /> {t('clusters.save')}
        </button>
      </form>

      <div className="overflow-x">
        <table className="data-table">
          <thead>
            <tr>
              <th>{t('clusters.table.cluster')}</th>
              <th>{t('clusters.table.endpoint')}</th>
              <th>{t('clusters.table.scheduler')}</th>
              <th>{t('clusters.table.probe')}</th>
              <th></th>
            </tr>
          </thead>
          <tbody>
            {clusters.map((c) => (
              <tr key={c.id}>
                <td>
                  <strong>{c.name}</strong>
                  <div style={{ fontSize: '0.72rem', opacity: 0.65 }}>{c.id}</div>
                </td>
                <td>
                  {c.ssh_user ? `${c.ssh_user}@` : ''}
                  {c.ssh_host}:{c.ssh_port}
                  {c.remote_dir && <div style={{ fontSize: '0.72rem', opacity: 0.65 }}>remote: {c.remote_dir}</div>}
                </td>
                <td>{c.scheduler ?? 'auto'}</td>
                <td>
                  {probeResults[c.id] ? (
                    <span style={{ fontSize: '0.78rem', color: probeResults[c.id].ok ? 'var(--color-success)' : 'var(--color-error)' }}>
                      {probeResults[c.id].text}
                    </span>
                  ) : (
                    <button className="btn-sm" onClick={() => handleProbe(c.id)}>
                      <Activity size={13} /> {t('clusters.probe')}
                    </button>
                  )}
                </td>
                <td>
                  <button className="icon-btn danger" title={t('clusters.removeConfirm').replace('{{name}}', c.name)} aria-label={t('clusters.removeConfirm').replace('{{name}}', c.name)} onClick={() => handleDelete(c.id, c.name)}>
                    <Trash2 size={14} />
                  </button>
                </td>
              </tr>
            ))}
            {clusters.length === 0 && (
              <tr>
                <td colSpan={5} style={{ textAlign: 'center', opacity: 0.6 }}>
                  {t('clusters.empty')}
                </td>
              </tr>
            )}
          </tbody>
        </table>
      </div>
    </div>
  );
}
