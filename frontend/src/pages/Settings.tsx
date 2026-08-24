import { useEffect, useRef, useState } from 'react';
import { api } from '../api/client';
import type { AiConfig, HealthResponse, QuotaInfo } from '../api/types';
import { FlaskConical, Cpu, HardDrive, Database, Shield } from 'lucide-react';
import StatCard from '../components/StatCard';
import { useI18n, getLocale } from '../context/I18n';

interface QuotaFormState {
  max_concurrent_runs: number;
  max_total_threads: number;
  max_total_memory_mb: number;
  max_runs_per_day: number;
}

const DEFAULT_QUOTA: QuotaFormState = {
  max_concurrent_runs: 10,
  max_total_threads: 64,
  max_total_memory_mb: 262144,
  max_runs_per_day: 100,
};

// The typed client has no quota endpoint — mirror its request pattern
// (base-path prefix + bearer token, see api/client.ts) so team-mode
// deployments under a sub-path or with auth enabled still load.
function fetchQuota(): Promise<QuotaInfo> {
  const base = (window as { __OXO_BASE__?: string }).__OXO_BASE__ ?? '';
  const token = localStorage.getItem('oxo_token');
  const headers: Record<string, string> = {};
  if (token) headers['Authorization'] = `Bearer ${token}`;
  return fetch(`${base}/api/quota`, { headers }).then((r) => {
    if (!r.ok) throw new Error(`quota: ${r.status}`);
    return r.json() as Promise<QuotaInfo>;
  });
}

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
  return <input {...props} className="search-input" style={{ width: '100%', ...(props.style as object) }} />;
}

export default function Settings() {
  const { lang, t } = useI18n();
  const [health, setHealth] = useState<HealthResponse | null>(null);
  const [aiConfig, setAiConfig] = useState<AiConfig | null>(null);
  const [quota, setQuota] = useState<QuotaInfo | null>(null);
  const [quotaForm, setQuotaForm] = useState<QuotaFormState>(DEFAULT_QUOTA);
  const [quotaSaving, setQuotaSaving] = useState(false);
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
    fetchQuota().then((q) => { setQuota(q); setQuotaForm(q.limits); }).catch(() => setNotice(t('settings.quota.loadFailed')));
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
      setTestResult(t('settings.ai.saveSuccess').replace('{{provider}}', c.provider));
    } catch (err: unknown) { setTestResult(t('settings.ai.saveFailed').replace('{{error}}', err instanceof Error ? err.message : t('common.unknownError'))); }
    setSaving(false);
  };

  const handleTest = async () => {
    setTestResult(t('settings.ai.testing'));
    try {
      const r = await api.aiTest();
      setTestResult(r.success ? t('settings.ai.testSuccess').replace('{{message}}', r.message) : t('settings.ai.testFailed').replace('{{error}}', r.message));
    } catch (err: unknown) { setTestResult(t('settings.ai.testFailed').replace('{{error}}', err instanceof Error ? err.message : t('common.unknownError'))); }
  };

  const handleSaveQuota = async () => {
    setQuotaSaving(true);
    try {
      await api.updateQuota(quotaForm);
      const q = await fetchQuota();
      setQuota(q);
      setQuotaForm(q.limits);
      setNotice(t('settings.quota.saved'));
    } catch (err: unknown) {
      const msg = err instanceof Error ? err.message : t('common.unknownError');
      setNotice(t('settings.quota.saveFailed').replace('{{error}}', msg));
    }
    setQuotaSaving(false);
  };

  const formatMemory = (mb: number) => {
    if (mb >= 1024 * 1024) return `${(mb / 1024 / 1024).toFixed(1)} TB`;
    if (mb >= 1024) return `${(mb / 1024).toFixed(0)} GB`;
    return `${mb} MB`;
  };

  return (
    <div className="page">
      <h1 className="page-title">{t('settings.title')}</h1>
      {notice && <div className="result-bar success" style={{ cursor: 'pointer' }} onClick={() => setNotice(null)}>{notice}</div>}
      <p className="page-subtitle">{t('settings.subtitle')}</p>

      {/* ── AI Provider ── */}
      <Section title={t('settings.ai.title')} icon={<Cpu size={16} color="var(--color-primary)" />}>
        <div className="settings-grid">
          <div>
            <div className="settings-note">
              {t('settings.ai.priority')}: <strong>{t('settings.ai.priorityNote')}</strong>
            </div>
            <div className="settings-form">
              <div>
                <SettingLabel text={t('settings.ai.provider')} />
                <select value={provider} onChange={(e) => setProvider(e.target.value)}
                  className="search-input" style={{ width: '100%' }}>
                  <option value="openai">OpenAI / DeepSeek / Groq</option>
                  <option value="deepseek">DeepSeek (native)</option>
                  <option value="claude">Claude (Anthropic)</option>
                  <option value="ollama">Ollama (local)</option>
                  <option value="disabled">Disabled</option>
                </select>
              </div>
              <div><SettingLabel text={t('settings.ai.key')} /><SettingInput type="password" value={apiKey} onChange={(e) => setApiKey(e.target.value)} placeholder="sk-..." /></div>
              <div><SettingLabel text={t('settings.ai.model')} /><SettingInput type="text" value={model} onChange={(e) => setModel(e.target.value)} placeholder="deepseek-v4-pro" /></div>
              <div><SettingLabel text={t('settings.ai.url')} /><SettingInput type="text" value={apiUrl} onChange={(e) => setApiUrl(e.target.value)} placeholder="https://api.deepseek.com/v1/chat/completions" /></div>
              <div className="row">
                <button onClick={handleSave} disabled={saving} className="btn-run">{saving ? 'Saving...' : t('settings.ai.save')}</button>
                <button onClick={handleTest} className="action-btn">{t('settings.ai.test')}</button>
              </div>
              {testResult && <div className={`result-bar ${testResult.startsWith('✅') ? 'success' : 'error'}`} style={{ marginTop: 0 }}>{testResult}</div>}
            </div>
          </div>
          <div className="settings-side">
            <div style={{ fontWeight: 600, marginBottom: '0.5rem' }}>{t('settings.ai.status')}</div>
            <div>{t('settings.ai.providerLabel')}: <strong>{aiConfig?.provider || 'unknown'}</strong></div>
            <div>{t('settings.ai.modelLabel')}: <strong>{aiConfig?.model || 'default'}</strong></div>
            <div>{t('settings.ai.urlLabel')}: <code style={{ fontSize: '0.7rem', overflowWrap: 'anywhere' }}>{aiConfig?.api_url || 'default'}</code></div>
            <div style={{ marginTop: '4px' }}>{t('settings.license.status')}: <span className={`status-badge ${aiConfig?.is_configured ? 'success' : 'failed'}`}>{aiConfig?.is_configured ? t('settings.ai.configured') : t('settings.ai.notConfigured')}</span></div>
            <div className="settings-note" style={{ marginTop: '1rem', marginBottom: 0 }}>
              <div style={{ fontWeight: 600, marginBottom: '4px' }}>{t('settings.ai.advanced')}</div>
              {/* issue #82 P1-4: these controls were display-only; now they
                  read and persist the per-user AI config. */}
              <label className="check-label">
                <input type="checkbox" checked={adv.search_enabled} onChange={(e) => setAdv({ ...adv, search_enabled: e.target.checked })} /> {t('settings.ai.search')}
              </label>
              <label className="check-label">
                <input type="checkbox" checked={adv.monitor_enabled} onChange={(e) => setAdv({ ...adv, monitor_enabled: e.target.checked })} /> {t('settings.ai.monitor')}
              </label>
              <label className="check-label">
                <input type="checkbox" checked={adv.auto_retry_enabled} onChange={(e) => setAdv({ ...adv, auto_retry_enabled: e.target.checked })} /> {t('settings.ai.autoRetry')}
              </label>
              <div style={{ marginTop: '6px' }}>
                <SettingLabel text={t('settings.ai.maxRounds')} />
                <select value={adv.max_correction_rounds} onChange={(e) => setAdv({ ...adv, max_correction_rounds: Number(e.target.value) })}
                  className="search-input">
                  {[1,2,3,4,5].map(n => <option key={n} value={n}>{n}</option>)}
                </select>
              </div>
              <div style={{ marginTop: '8px' }}>
                <button className="btn-sm" onClick={() => api.aiUpdateConfigUser({
                  search_enabled: adv.search_enabled,
                  monitor_enabled: adv.monitor_enabled,
                  auto_retry_enabled: adv.auto_retry_enabled,
                  max_correction_rounds: adv.max_correction_rounds,
                }).then(() => setNotice(t('settings.ai.saveAdvanced'))).catch(() => setNotice(t('settings.ai.advancedSaveFailed')))}>
                  {t('settings.ai.saveAdvanced')}
                </button>
              </div>
            </div>
          </div>
        </div>
      </Section>

      {/* ── References ── */}
      <Section title={t('settings.refs.title')} icon={<Database size={16} color="var(--color-primary)" />}>
        <div style={{ fontSize: '0.85rem' }}>
          <div className="settings-note">
            {t('settings.refs.basePath')}: <code>/data/references</code>
          </div>
          <div style={{ display: 'grid', gap: '2px' }}>
            {refs?.installed?.map((ref: Record<string, unknown>, idx: number) => (
              <div key={idx} className="ref-row">
                <div>
                  <strong>{String(ref.genome || 'unknown')}</strong>
                  <span className="muted" style={{ marginLeft: '8px' }}>{Array.isArray(ref.components) ? ref.components.join(', ') : ''}</span>
                </div>
                <div className="row">
                  <span className="status-badge success">{t('settings.refs.complete')}</span>
                </div>
              </div>
            ))}
            {refs?.missing?.map((missingName: string, idx: number) => (
              <div key={`missing-${idx}`} className="ref-row">
                <div>
                  <strong>{missingName}</strong>
                </div>
                <div className="row">
                  <span className="status-badge warning">{t('settings.refs.missing')}</span>
                  <button className="btn-sm" style={{ fontSize: '0.7rem' }}
                    onClick={() => { api.discoverReference(missingName, []).then(() => api.referenceStatus().then(setRefs)).catch(() => setNotice(t('settings.refs.downloadFailed').replace('{{name}}', missingName))); }}>
                    {t('settings.refs.download')}
                  </button>
                </div>
              </div>
            ))}
            {!refs && <div style={{ color: 'var(--color-text-secondary)' }}>{t('settings.refs.loading')}</div>}
          </div>
          <div style={{ marginTop: '0.75rem' }}>
            <button className="btn-sm" onClick={() => {
              const name = window.prompt(t('settings.refs.prompt'));
              if (!name) return;
              api.discoverReference(name.trim(), []).then(() => api.referenceStatus().then(setRefs)).catch(() => setNotice(t('settings.refs.addFailed').replace('{{name}}', name.trim())));
            }}>{t('settings.refs.add')}</button>
          </div>
        </div>
      </Section>

      {/* ── Environments ── */}
      <Section title={t('settings.env.title')} icon={<FlaskConical size={16} color="var(--color-primary)" />}>
        <div style={{ fontSize: '0.85rem' }}>
          <div style={{ marginBottom: '0.5rem' }}>
            {t('settings.env.default')}: <strong>{t('settings.env.defaultValue')}</strong>
          </div>
          <div style={{ display: 'grid', gap: '6px' }}>
            {['conda', 'docker', 'singularity', 'pixi'].map(envName => {
              const available = null; // env detection via system API
              return (
              <div key={envName} className="ref-row">
                <div>
                  <strong>{envName}</strong>
                  <span className="muted" style={{ marginLeft: '8px' }}>{available ? t('settings.env.available') : t('settings.env.unavailable')}</span>
                </div>
                <span className={`status-badge ${available ? 'success' : 'cancelled'}`}>{available ? t('settings.env.available') : t('settings.env.unavailable')}</span>
              </div>
            )})}
          </div>
        </div>
      </Section>

      {/* ── Quota (Team) ── */}
      <Section title={t('settings.quota.title')} icon={<HardDrive size={16} color="var(--color-primary)" />}>
        <div style={{ fontSize: '0.85rem' }}>
          {quota?.enabled ? (
            <>
              <div className="stat-grid" style={{ marginBottom: '1rem' }}>
                <StatCard value={quota.usage.active_runs} label={t('settings.quota.activeRuns')} />
                <StatCard value={quota.usage.used_threads} label={t('settings.quota.usedThreads')} />
                <StatCard value={formatMemory(quota.usage.used_memory_mb)} label={t('settings.quota.usedMemory')} />
                <StatCard value={quota.usage.runs_today} label={t('settings.quota.runsToday')} />
              </div>
              <div style={{ color: 'var(--color-text-secondary)', fontWeight: 600, marginBottom: '0.5rem' }}>{t('settings.quota.usage')}</div>
              <div className="settings-form" style={{ gridTemplateColumns: 'repeat(auto-fit, minmax(180px, 1fr))' }}>
                <div>
                  <SettingLabel text={t('settings.quota.maxConcurrent')} />
                  <SettingInput type="number" min={1} value={quotaForm.max_concurrent_runs}
                    onChange={(e) => setQuotaForm({ ...quotaForm, max_concurrent_runs: Math.max(1, Number(e.target.value)) })} />
                </div>
                <div>
                  <SettingLabel text={t('settings.quota.maxThreads')} />
                  <SettingInput type="number" min={1} value={quotaForm.max_total_threads}
                    onChange={(e) => setQuotaForm({ ...quotaForm, max_total_threads: Math.max(1, Number(e.target.value)) })} />
                </div>
                <div>
                  <SettingLabel text={t('settings.quota.maxMemory')} />
                  <SettingInput type="number" min={1} value={quotaForm.max_total_memory_mb}
                    onChange={(e) => setQuotaForm({ ...quotaForm, max_total_memory_mb: Math.max(1, Number(e.target.value)) })} />
                </div>
                <div>
                  <SettingLabel text={t('settings.quota.maxDaily')} />
                  <SettingInput type="number" min={1} value={quotaForm.max_runs_per_day}
                    onChange={(e) => setQuotaForm({ ...quotaForm, max_runs_per_day: Math.max(1, Number(e.target.value)) })} />
                </div>
              </div>
              <div style={{ marginTop: '0.75rem', display: 'flex', gap: '8px', alignItems: 'center' }}>
                <button className="btn-run" onClick={handleSaveQuota} disabled={quotaSaving}>
                  {quotaSaving ? 'Saving...' : t('settings.quota.save')}
                </button>
                <span style={{ fontSize: '0.75rem', color: 'var(--color-text-tertiary)' }}>{t('settings.quota.adminOnly')}</span>
              </div>
            </>
          ) : (
            <span style={{ color: 'var(--color-text-secondary)' }}>Quota system enabled for team mode.</span>
          )}
        </div>
      </Section>

      {/* ── API Keys (issue #82 P1-13) ── */}
      <Section title={t('settings.keys.title')} icon={<Shield size={16} color="var(--color-primary)" />}>
        <div style={{ fontSize: '0.85rem' }}>
          <p style={{ color: 'var(--color-text-secondary)', marginBottom: '0.5rem' }}>
            {t('settings.keys.hint')}
          </p>
          {apiKeys.length === 0 ? (
            <div style={{ color: 'var(--color-text-tertiary)' }}>{t('settings.keys.none')}</div>
          ) : (
            <table className="run-table">
              <thead><tr><th>{t('settings.keys.name')}</th><th>{t('settings.keys.createdAt')}</th><th>{t('settings.keys.lastUsed')}</th><th></th></tr></thead>
              <tbody>
                {apiKeys.map((k) => (
                  <tr key={k.id}>
                    <td>{k.name}</td>
                    <td>{new Date(k.created_at).toLocaleString(getLocale(lang))}</td>
                    <td>{k.last_used_at ? new Date(k.last_used_at).toLocaleString(getLocale(lang)) : t('settings.keys.never')}</td>
                    <td>
                      <button className="btn-sm btn-error" onClick={() => {
                        if (!window.confirm(t('settings.keys.revokeConfirm').replace('{{name}}', k.name))) return;
                        api.revokeApiKey(k.id).then(() => loadApiKeys()).catch(() => setNotice(t('settings.keys.revokeFailed')));
                      }}>{t('settings.keys.revoke')}</button>
                    </td>
                  </tr>
                ))}
              </tbody>
            </table>
          )}
          <div style={{ marginTop: '0.75rem', display: 'flex', gap: '8px' }}>
            <input className="search-input" placeholder={t('settings.keys.name')} value={newKeyName}
              onChange={(e) => setNewKeyName(e.target.value)} style={{ flex: 1, maxWidth: 240 }} />
            <button className="btn-sm" onClick={async () => {
              const name = newKeyName.trim() || 'unnamed';
              try {
                const created = await api.createApiKey(name);
                setNewKeyName('');
                setNotice(t('settings.keys.created').replace('{{key}}', created.key));
                await loadApiKeys();
              } catch { setNotice(t('settings.keys.createFailed')); }
            }}>{t('settings.keys.create')}</button>
          </div>
        </div>
      </Section>

      {/* ── Webhooks (issue #82 P1-12) ── */}
      <Section title={t('settings.webhook.title')} icon={<Shield size={16} color="var(--color-primary)" />}>
        <div style={{ fontSize: '0.85rem' }}>
          <p style={{ color: 'var(--color-text-secondary)', marginBottom: '0.5rem' }}>
            {t('settings.webhook.hint')}
          </p>
          <SettingLabel text={t('settings.webhook.url')} />
          <SettingInput placeholder="https://example.com/oxo-webhook" value={webhook.url}
            onChange={(e) => setWebhook({ ...webhook, url: e.target.value })} />
          <div style={{ marginTop: '0.5rem' }}>
            <SettingLabel text={webhook.secret_set ? t('settings.webhook.secretSet') : t('settings.webhook.secret')} />
            <SettingInput type="password" placeholder={webhook.secret_set ? '••••••••' : 'secret'} value={webhookSecret}
              onChange={(e) => setWebhookSecret(e.target.value)} />
          </div>
          <label className="check-label" style={{ marginTop: '0.5rem' }}>
            <input type="checkbox" checked={webhook.enabled} onChange={(e) => setWebhook({ ...webhook, enabled: e.target.checked })} />
            {t('settings.webhook.enabled')}
          </label>
          <div style={{ marginTop: '0.75rem', display: 'flex', gap: '8px' }}>
            <button className="btn-sm" onClick={() => {
              api.webhookUpdate({ enabled: webhook.enabled, url: webhook.url, secret: webhookSecret || undefined })
                .then(() => { setWebhookSecret(''); api.webhookConfig().then(setWebhook); setNotice(t('settings.webhook.saved')); })
                .catch(() => setNotice(t('settings.webhook.saveFailed')));
            }}>{t('settings.webhook.save')}</button>
          </div>
        </div>
      </Section>

      {/* ── License ── */}
      <Section title={t('settings.license.title')} icon={<Shield size={16} color="var(--color-primary)" />}>
        <div style={{ fontSize: '0.85rem' }}>
          <div>{t('settings.license.type')}: <strong>{health?.license?.license_type || t('layout.academicLicense')}</strong></div>
          <div>{t('settings.license.status')}: <span className="status-badge success">{t('settings.license.valid')}</span></div>
          <div style={{ marginTop: '4px' }}>{t('settings.license.contact')}: <strong>{health?.license?.contact || t('settings.license.defaultContact')}</strong></div>
          <div style={{ marginTop: '4px', color: 'var(--color-text-secondary)' }}>{health?.license?.message || t('settings.license.defaultMessage')}</div>
          <div style={{ marginTop: '0.75rem', display: 'flex', gap: '8px', alignItems: 'center' }}>
            <button className="btn-sm" onClick={() => licenseInputRef.current?.click()}>{t('settings.license.upload')}</button>
            <input ref={licenseInputRef} type="file" accept=".key,.lic,.txt"
              style={{ display: 'none' }}
              onChange={(e) => {
                const file = e.target.files?.[0];
                if (!file) return;
                file.text().then((text) => api.uploadLicense(text))
                  .then(() => setNotice(t('settings.license.submitted')))
                  .catch(() => setNotice(t('settings.license.uploadFailed')));
              }} />
          </div>
        </div>
      </Section>
    </div>
  );
}
