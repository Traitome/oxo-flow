import { useEffect, useRef, useState } from 'react';
import { api } from '../api/client';
import type { AiConfig, HealthResponse } from '../api/types';
import { FlaskConical, Cpu, HardDrive, Database, Shield } from 'lucide-react';

interface QuotaInfo { enabled: boolean; limits: { max_concurrent_runs: number; max_total_threads: number; max_total_memory_mb: number; max_runs_per_day: number } }

// Module-level components — defined outside the render function so React
// preserves their identity across re-renders (prevents input focus loss).
function Section({ title, icon, children }: { title: string; icon: React.ReactNode; children: React.ReactNode }) {
  return (
    <div className="dash-card" style={{ marginBottom: '1rem' }}>
      <h3 className="dash-card-title" style={{ display: 'flex', alignItems: 'center', gap: '8px' }}>{icon} {title}</h3>
      {children}
    </div>
  );
}

function SettingLabel({ text }: { text: string }) {
  return <label style={{ fontSize: '0.8rem', fontWeight: 500, color: 'var(--color-text-secondary)', display: 'block', marginBottom: '4px' }}>{text}</label>;
}

function SettingInput(props: React.InputHTMLAttributes<HTMLInputElement>) {
  return <input {...props} style={{ width: '100%', padding: '0.5rem', border: '1px solid var(--color-border)', borderRadius: 'var(--radius-sm)', fontSize: '0.9rem', background: 'var(--color-bg)', color: 'var(--color-text)', ...(props.style as object) }} />;
}

export default function Settings() {
  const [health, setHealth] = useState<HealthResponse | null>(null);
  const [aiConfig, setAiConfig] = useState<AiConfig | null>(null);
  const [quota, setQuota] = useState<QuotaInfo | null>(null);
  const [provider, setProvider] = useState('openai');
  const [apiKey, setApiKey] = useState('');
  const [apiUrl, setApiUrl] = useState('');
  const [model, setModel] = useState('');
  const [testResult, setTestResult] = useState<string | null>(null);
  const [saving, setSaving] = useState(false);
  const [refs, setRefs] = useState<{ installed: Array<Record<string, unknown>>; missing: string[] } | null>(null);
  const [notice, setNotice] = useState<string | null>(null);
  const [adv, setAdv] = useState({ search_enabled: true, monitor_enabled: true, auto_retry_enabled: false, max_correction_rounds: 3 });
  const [webhook, setWebhook] = useState<{ enabled: boolean; url: string; secret_set: boolean; events: string[] }>({ enabled: false, url: '', secret_set: false, events: [] });
  const [webhookSecret, setWebhookSecret] = useState('');
  const [apiKeys, setApiKeys] = useState<Array<{ id: string; name: string; created_at: string; last_used_at: string | null }>>([]);
  const [newKeyName, setNewKeyName] = useState('');

  const loadApiKeys = async () => {
    try { setApiKeys(await api.listApiKeys()); } catch { setApiKeys([]); }
  };
  const licenseInputRef = useRef<HTMLInputElement>(null);


  useEffect(() => {
    api.health().then(setHealth).catch(() => {});
    api.aiConfig().then((c) => { setAiConfig(c); setProvider(c.provider); if (c.api_url) setApiUrl(c.api_url); if (c.model) setModel(c.model); }).catch(() => {});
    fetch('/api/quota').then(r => r.json()).then(setQuota).catch(() => {});
    api.referenceStatus().then(setRefs).catch(() => {});
    api.aiConfigUser().then((c) => {
      const u = c.user_config as Partial<typeof adv> | undefined;
      if (u) setAdv({
        search_enabled: u.search_enabled ?? true,
        monitor_enabled: u.monitor_enabled ?? true,
        auto_retry_enabled: u.auto_retry_enabled ?? false,
        max_correction_rounds: u.max_correction_rounds ?? 3,
      });
    }).catch(() => {});
    api.webhookConfig().then(setWebhook).catch(() => {});
    api.listApiKeys().then(setApiKeys).catch(() => setApiKeys([]));

  }, []);

  const handleSave = async () => {
    setSaving(true); setTestResult(null);
    try {
      // Two channels (issue #79 §9b claim #9 was falsified: Save only
      // touched memory and the config vanished on restart):
      //  1. POST /api/ai/config — reconfigures the live provider now
      //  2. PUT /api/ai/config/user — persists to DB, restored on startup
      await api.aiUpdateConfig(provider, apiKey || undefined, apiUrl || undefined, model || undefined);
      await api.aiUpdateConfigUser({
        provider,
        api_key: apiKey || undefined,
        api_url: apiUrl || undefined,
        model: model || undefined,
      });
      const c = await api.aiConfig(); setAiConfig(c);
      setTestResult('✅ Saved & persisted. Provider: ' + c.provider);
    } catch (err: unknown) { setTestResult('❌ ' + (err instanceof Error ? err.message : 'Save failed')); }
    setSaving(false);
  };

  const handleTest = async () => {
    setTestResult('Testing...');
    try {
      const r = await api.aiTest();
      setTestResult(r.success ? '✅ Connected: ' + r.message : '❌ ' + r.message);
    } catch (err: unknown) { setTestResult('❌ ' + (err instanceof Error ? err.message : 'Test failed')); }
  };

  return (
    <div className="page">
      <h1 className="page-title">Settings</h1>
      {notice && <div className="result-bar success" style={{ cursor: 'pointer' }} onClick={() => setNotice(null)}>{notice}</div>}
      <p className="page-subtitle">Configure AI, references, environments, and system preferences</p>

      {/* ── AI Provider ── */}
      <Section title="AI Provider Configuration" icon={<Cpu size={16} color="var(--color-primary)" />}>
        <div style={{ display: 'grid', gridTemplateColumns: '1fr 1fr', gap: '1rem' }}>
          <div>
            <div style={{ marginBottom: '0.5rem', padding: '8px 12px', background: 'var(--color-bg-tertiary)', borderRadius: 'var(--radius-sm)', fontSize: '0.75rem', color: 'var(--color-text-secondary)' }}>
              Config Priority: <strong>User Settings</strong> → Server Config → Environment → Default
            </div>
            <div style={{ display: 'grid', gap: '0.75rem' }}>
              <div>
                <SettingLabel text="Provider" />
                <select value={provider} onChange={(e) => setProvider(e.target.value)}
                  style={{ width: '100%', padding: '0.5rem', border: '1px solid var(--color-border)', borderRadius: 'var(--radius-sm)', fontSize: '0.9rem', background: 'var(--color-bg)', color: 'var(--color-text)' }}>
                  <option value="openai">OpenAI / DeepSeek / Groq</option>
                  <option value="claude">Claude (Anthropic)</option>
                  <option value="ollama">Ollama (local)</option>
                  <option value="disabled">Disabled</option>
                </select>
              </div>
              <div><SettingLabel text="API Key" /><SettingInput type="password" value={apiKey} onChange={(e) => setApiKey(e.target.value)} placeholder="sk-..." /></div>
              <div><SettingLabel text="Model" /><SettingInput type="text" value={model} onChange={(e) => setModel(e.target.value)} placeholder="deepseek-v4-pro" /></div>
              <div><SettingLabel text="API URL (optional)" /><SettingInput type="text" value={apiUrl} onChange={(e) => setApiUrl(e.target.value)} placeholder="https://api.deepseek.com/v1/chat/completions" /></div>
              <div style={{ display: 'flex', gap: '0.5rem' }}>
                <button onClick={handleSave} disabled={saving} className="btn-run">{saving ? 'Saving...' : 'Save'}</button>
                <button onClick={handleTest} className="action-btn">Test Connection</button>
              </div>
              {testResult && <div className={`result-bar ${testResult.startsWith('✅') ? 'success' : 'error'}`}>{testResult}</div>}
            </div>
          </div>
          <div style={{ fontSize: '0.85rem', paddingLeft: '1rem', borderLeft: '1px solid var(--color-border)' }}>
            <div style={{ fontWeight: 600, marginBottom: '0.5rem' }}>Current Status</div>
            <div>Provider: <strong>{aiConfig?.provider || 'unknown'}</strong></div>
            <div>Model: <strong>{aiConfig?.model || 'default'}</strong></div>
            <div>URL: <code style={{ fontSize: '0.7rem' }}>{aiConfig?.api_url || 'default'}</code></div>
            <div style={{ marginTop: '4px' }}>Status: <span className={`status-badge ${aiConfig?.is_configured ? 'success' : 'cancelled'}`}>{aiConfig?.is_configured ? 'Configured' : 'Not Configured'}</span></div>
            <div style={{ marginTop: '1rem', background: 'var(--color-bg-tertiary)', padding: '8px 12px', borderRadius: 'var(--radius-sm)', fontSize: '0.75rem' }}>
              <div style={{ fontWeight: 600, marginBottom: '4px' }}>Advanced Options</div>
              {/* issue #82 P1-4: these controls were display-only; now they
                  read and persist the per-user AI config. */}
              <label style={{ display: 'flex', alignItems: 'center', gap: '6px', cursor: 'pointer', fontSize: '0.8rem' }}>
                <input type="checkbox" checked={adv.search_enabled} onChange={(e) => setAdv({ ...adv, search_enabled: e.target.checked })} /> Internet search
              </label>
              <label style={{ display: 'flex', alignItems: 'center', gap: '6px', cursor: 'pointer', fontSize: '0.8rem' }}>
                <input type="checkbox" checked={adv.monitor_enabled} onChange={(e) => setAdv({ ...adv, monitor_enabled: e.target.checked })} /> AI monitoring
              </label>
              <label style={{ display: 'flex', alignItems: 'center', gap: '6px', cursor: 'pointer', fontSize: '0.8rem' }}>
                <input type="checkbox" checked={adv.auto_retry_enabled} onChange={(e) => setAdv({ ...adv, auto_retry_enabled: e.target.checked })} /> Auto retry without asking
              </label>
              <div style={{ marginTop: '6px' }}>
                <SettingLabel text="Max correction rounds" />
                <select value={adv.max_correction_rounds} onChange={(e) => setAdv({ ...adv, max_correction_rounds: Number(e.target.value) })}
                  style={{ padding: '2px 6px', border: '1px solid var(--color-border)', borderRadius: 'var(--radius-sm)', fontSize: '0.8rem' }}>
                  {[1,2,3,4,5].map(n => <option key={n} value={n}>{n}</option>)}
                </select>
              </div>
              <div style={{ marginTop: '8px' }}>
                <button className="btn-sm" onClick={() => api.aiUpdateConfigUser({
                  search_enabled: adv.search_enabled,
                  monitor_enabled: adv.monitor_enabled,
                  auto_retry_enabled: adv.auto_retry_enabled,
                  max_correction_rounds: adv.max_correction_rounds,
                }).then(() => setNotice('Advanced AI options saved')).catch(() => setNotice('Could not save advanced options'))}>
                  Save advanced options
                </button>
              </div>
            </div>
          </div>
        </div>
      </Section>

      {/* ── References ── */}
      <Section title="Reference Genomes" icon={<Database size={16} color="var(--color-primary)" />}>
        <div style={{ fontSize: '0.85rem' }}>
          <div style={{ marginBottom: '0.5rem', padding: '8px 12px', background: 'var(--color-bg-tertiary)', borderRadius: 'var(--radius-sm)', fontSize: '0.75rem', color: 'var(--color-text-secondary)' }}>
            Base path: <code>/data/references</code>
          </div>
          <div style={{ display: 'grid', gap: '6px' }}>
            {refs?.installed?.map((ref: Record<string, unknown>, idx: number) => (
              <div key={idx} style={{ display: 'flex', alignItems: 'center', justifyContent: 'space-between', padding: '6px 0', borderBottom: '1px solid var(--color-border-light)' }}>
                <div>
                  <strong>{String(ref.genome || 'unknown')}</strong>
                  <span style={{ color: 'var(--color-text-secondary)', marginLeft: '8px', fontSize: '0.8rem' }}>{Array.isArray(ref.components) ? ref.components.join(', ') : ''}</span>
                </div>
                <div style={{ display: 'flex', alignItems: 'center', gap: '8px' }}>
                  <span className="status-badge success">Complete</span>
                </div>
              </div>
            ))}
            {refs?.missing?.map((missingName: string, idx: number) => (
              <div key={`missing-${idx}`} style={{ display: 'flex', alignItems: 'center', justifyContent: 'space-between', padding: '6px 0', borderBottom: '1px solid var(--color-border-light)' }}>
                <div>
                  <strong>{missingName}</strong>
                </div>
                <div style={{ display: 'flex', alignItems: 'center', gap: '8px' }}>
                  <span className="status-badge warning">Missing</span>
                  <button className="btn-sm" style={{ fontSize: '0.7rem' }}
                    onClick={() => { api.discoverReference(missingName, []).then(() => api.referenceStatus().then(setRefs)).catch(() => setNotice(`Could not start download for ${missingName}`)); }}>
                    Download
                  </button>
                </div>
              </div>
            ))}
            {!refs && <div style={{ color: 'var(--color-text-secondary)' }}>Loading references...</div>}
          </div>
          <div style={{ marginTop: '0.75rem' }}>
            <button className="btn-sm" onClick={() => {
              const name = window.prompt('Reference genome name (e.g. GRCh38)?');
              if (!name) return;
              api.discoverReference(name.trim(), []).then(() => api.referenceStatus().then(setRefs)).catch(() => setNotice(`Could not add reference ${name}`));
            }}>+ Add Reference Genome</button>
          </div>
        </div>
      </Section>

      {/* ── Environments ── */}
      <Section title="Computing Environments" icon={<FlaskConical size={16} color="var(--color-primary)" />}>
        <div style={{ fontSize: '0.85rem' }}>
          <div style={{ marginBottom: '0.5rem' }}>
            Default: <strong>Conda (bioconda channel)</strong>
          </div>
          <div style={{ display: 'grid', gap: '6px' }}>
            {['conda', 'docker', 'singularity', 'pixi'].map(envName => {
              const available = null; // env detection via system API
              return (
              <div key={envName} style={{ display: 'flex', alignItems: 'center', justifyContent: 'space-between', padding: '6px 0', borderBottom: '1px solid var(--color-border-light)' }}>
                <div>
                  <strong>{envName}</strong>
                  <span style={{ color: 'var(--color-text-secondary)', marginLeft: '8px', fontSize: '0.8rem' }}>{available ? 'detected' : 'not detected'}</span>
                </div>
                <span className={`status-badge ${available ? 'success' : 'cancelled'}`}>{available ? 'available' : 'unavailable'}</span>
              </div>
            )})}
          </div>
        </div>
      </Section>

      {/* ── Quota (Team) ── */}
      <Section title="Resource Quota" icon={<HardDrive size={16} color="var(--color-primary)" />}>
        <div style={{ fontSize: '0.85rem' }}>
          {quota?.enabled ? (
            <div style={{ display: 'grid', gridTemplateColumns: 'repeat(4, 1fr)', gap: '1rem' }}>
              <div className="stat-card"><div className="stat-value">{quota.limits.max_concurrent_runs}</div><div className="stat-label">Max Concurrent Runs</div></div>
              <div className="stat-card"><div className="stat-value">{quota.limits.max_total_threads}</div><div className="stat-label">Max Total Threads</div></div>
              <div className="stat-card"><div className="stat-value">{(quota.limits.max_total_memory_mb / 1024).toFixed(0)} GB</div><div className="stat-label">Max Total Memory</div></div>
              <div className="stat-card"><div className="stat-value">{quota.limits.max_runs_per_day}</div><div className="stat-label">Max Runs / Day</div></div>
            </div>
          ) : (
            <span style={{ color: 'var(--color-text-secondary)' }}>Quota system enabled for team mode.</span>
          )}
        </div>
      </Section>

      {/* ── API Keys (issue #82 P1-13) ── */}
      <Section title="API Keys" icon={<Shield size={16} color="var(--color-primary)" />}>
        <div style={{ fontSize: '0.85rem' }}>
          <p style={{ color: 'var(--color-text-secondary)', marginBottom: '0.5rem' }}>
            Machine credentials for scripts and CI — send them in the{' '}
            <code>X-API-Key</code> header. The key is shown only once at creation.
          </p>
          {apiKeys.length === 0 ? (
            <div style={{ color: 'var(--color-text-tertiary)' }}>No keys yet.</div>
          ) : (
            <table className="run-table">
              <thead><tr><th>Name</th><th>Created</th><th>Last used</th><th></th></tr></thead>
              <tbody>
                {apiKeys.map((k) => (
                  <tr key={k.id}>
                    <td>{k.name}</td>
                    <td>{new Date(k.created_at).toLocaleString()}</td>
                    <td>{k.last_used_at ? new Date(k.last_used_at).toLocaleString() : 'never'}</td>
                    <td>
                      <button className="btn-sm btn-error" onClick={() => {
                        if (!window.confirm(`Revoke key "${k.name}"? Requests using it will fail immediately.`)) return;
                        api.revokeApiKey(k.id).then(() => loadApiKeys()).catch(() => setNotice('Could not revoke key'));
                      }}>Revoke</button>
                    </td>
                  </tr>
                ))}
              </tbody>
            </table>
          )}
          <div style={{ marginTop: '0.75rem', display: 'flex', gap: '8px' }}>
            <input className="search-input" placeholder="Key name (e.g. ci-bot)" value={newKeyName}
              onChange={(e) => setNewKeyName(e.target.value)} style={{ flex: 1, maxWidth: 240 }} />
            <button className="btn-sm" onClick={async () => {
              const name = newKeyName.trim() || 'unnamed';
              try {
                const created = await api.createApiKey(name);
                setNewKeyName('');
                setNotice(`API key created — copy it NOW (shown only once): ${created.key}`);
                await loadApiKeys();
              } catch { setNotice('Could not create API key'); }
            }}>+ Create key</button>
          </div>
        </div>
      </Section>

      {/* ── Webhooks (issue #82 P1-12) ── */}
      <Section title="Run Notifications (Webhook)" icon={<Shield size={16} color="var(--color-primary)" />}>
        <div style={{ fontSize: '0.85rem' }}>
          <p style={{ color: 'var(--color-text-secondary)', marginBottom: '0.5rem' }}>
            Runs POST an HMAC-SHA256-signed payload to this endpoint when they finish
            (verified via the <code>X-OxoFlow-Signature</code> header).
          </p>
          <SettingLabel text="Webhook URL" />
          <SettingInput placeholder="https://example.com/oxo-webhook" value={webhook.url}
            onChange={(e) => setWebhook({ ...webhook, url: e.target.value })} />
          <div style={{ marginTop: '0.5rem' }}>
            <SettingLabel text={webhook.secret_set ? 'Signing secret (set — leave blank to keep)' : 'Signing secret'} />
            <SettingInput type="password" placeholder={webhook.secret_set ? '••••••••' : 'secret'} value={webhookSecret}
              onChange={(e) => setWebhookSecret(e.target.value)} />
          </div>
          <label style={{ display: 'flex', alignItems: 'center', gap: '6px', marginTop: '0.5rem', fontSize: '0.82rem' }}>
            <input type="checkbox" checked={webhook.enabled} onChange={(e) => setWebhook({ ...webhook, enabled: e.target.checked })} />
            Enable notifications
          </label>
          <div style={{ marginTop: '0.75rem', display: 'flex', gap: '8px' }}>
            <button className="btn-sm" onClick={() => {
              api.webhookUpdate({ enabled: webhook.enabled, url: webhook.url, secret: webhookSecret || undefined })
                .then(() => { setWebhookSecret(''); api.webhookConfig().then(setWebhook); setNotice('Webhook settings saved'); })
                .catch(() => setNotice('Could not save webhook settings (team mode: admin only)'));
            }}>Save webhook</button>
          </div>
        </div>
      </Section>

      {/* ── License ── */}
      <Section title="License" icon={<Shield size={16} color="var(--color-primary)" />}>
        <div style={{ fontSize: '0.85rem' }}>
          <div>Type: <strong>{health?.license?.license_type || 'academic'}</strong></div>
          <div>Status: <span className="status-badge success">Valid</span></div>
          <div style={{ marginTop: '4px' }}>Contact: <strong>{health?.license?.contact || 'w_shixiang@163.com'}</strong></div>
          <div style={{ marginTop: '4px', color: 'var(--color-text-secondary)' }}>{health?.license?.message || 'Free for academic use. Commercial use requires authorization.'}</div>
          <div style={{ marginTop: '0.75rem', display: 'flex', gap: '8px', alignItems: 'center' }}>
            <button className="btn-sm" onClick={() => licenseInputRef.current?.click()}>Upload Commercial License</button>
            <input ref={licenseInputRef} type="file" accept=".key,.lic,.txt"
              style={{ display: 'none' }}
              onChange={(e) => {
                const file = e.target.files?.[0];
                if (!file) return;
                file.text().then((text) => api.uploadLicense(text))
                  .then(() => setNotice('License submitted'))
                  .catch(() => setNotice('License upload failed'));
              }} />
          </div>
        </div>
      </Section>
    </div>
  );
}
