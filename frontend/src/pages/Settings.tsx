import { useEffect, useState } from 'react';
import { api } from '../api/client';
import type { AiConfig, HealthResponse } from '../api/types';
import { FlaskConical, Cpu, HardDrive, Database, Shield } from 'lucide-react';

interface QuotaInfo { enabled: boolean; limits: { max_concurrent_runs: number; max_total_threads: number; max_total_memory_mb: number; max_runs_per_day: number } }

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
  const [refs, setRefs] = useState<{ installed: any[]; missing: string[] } | null>(null);


  useEffect(() => {
    api.health().then(setHealth).catch(() => {});
    api.aiConfig().then((c) => { setAiConfig(c); setProvider(c.provider); if (c.api_url) setApiUrl(c.api_url); if (c.model) setModel(c.model); }).catch(() => {});
    fetch('/api/quota').then(r => r.json()).then(setQuota).catch(() => {});
    api.referenceStatus().then(setRefs).catch(() => {});

  }, []);

  const handleSave = async () => {
    setSaving(true); setTestResult(null);
    try {
      await api.aiUpdateConfig(provider, apiKey || undefined, apiUrl || undefined, model || undefined);
      const c = await api.aiConfig(); setAiConfig(c);
      setTestResult('✅ Saved. Provider: ' + c.provider);
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

  const Section = ({ title, icon, children }: { title: string; icon: React.ReactNode; children: React.ReactNode }) => (
    <div className="dash-card" style={{ marginBottom: '1rem' }}>
      <h3 className="dash-card-title" style={{ display: 'flex', alignItems: 'center', gap: '8px' }}>{icon} {title}</h3>
      {children}
    </div>
  );

  const Label = ({ text }: { text: string }) => <label style={{ fontSize: '0.8rem', fontWeight: 500, color: 'var(--color-text-secondary)', display: 'block', marginBottom: '4px' }}>{text}</label>;

  const Input = (props: React.InputHTMLAttributes<HTMLInputElement>) => <input {...props} style={{ width: '100%', padding: '0.5rem', border: '1px solid var(--color-border)', borderRadius: 'var(--radius-sm)', fontSize: '0.9rem', background: 'var(--color-bg)', color: 'var(--color-text)', ...(props as any).style }} />;

  return (
    <div className="page">
      <h1 className="page-title">Settings</h1>
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
                <Label text="Provider" />
                <select value={provider} onChange={(e) => setProvider(e.target.value)}
                  style={{ width: '100%', padding: '0.5rem', border: '1px solid var(--color-border)', borderRadius: 'var(--radius-sm)', fontSize: '0.9rem', background: 'var(--color-bg)', color: 'var(--color-text)' }}>
                  <option value="openai">OpenAI / DeepSeek / Groq</option>
                  <option value="claude">Claude (Anthropic)</option>
                  <option value="ollama">Ollama (local)</option>
                  <option value="disabled">Disabled</option>
                </select>
              </div>
              <div><Label text="API Key" /><Input type="password" value={apiKey} onChange={(e) => setApiKey(e.target.value)} placeholder="sk-..." /></div>
              <div><Label text="Model" /><Input type="text" value={model} onChange={(e) => setModel(e.target.value)} placeholder="deepseek-v4-pro" /></div>
              <div><Label text="API URL (optional)" /><Input type="text" value={apiUrl} onChange={(e) => setApiUrl(e.target.value)} placeholder="https://api.deepseek.com/v1/chat/completions" /></div>
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
              <label style={{ display: 'flex', alignItems: 'center', gap: '6px', cursor: 'pointer', fontSize: '0.8rem' }}>
                <input type="checkbox" defaultChecked /> Internet search
              </label>
              <label style={{ display: 'flex', alignItems: 'center', gap: '6px', cursor: 'pointer', fontSize: '0.8rem' }}>
                <input type="checkbox" defaultChecked /> AI monitoring
              </label>
              <label style={{ display: 'flex', alignItems: 'center', gap: '6px', cursor: 'pointer', fontSize: '0.8rem' }}>
                <input type="checkbox" /> Auto retry without asking
              </label>
              <div style={{ marginTop: '6px' }}>
                <Label text="Max correction rounds" />
                <select defaultValue="3" style={{ padding: '2px 6px', border: '1px solid var(--color-border)', borderRadius: 'var(--radius-sm)', fontSize: '0.8rem' }}>
                  {[1,2,3,4,5].map(n => <option key={n} value={n}>{n}</option>)}
                </select>
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
            {refs?.installed?.map((ref: any, idx: number) => (
              <div key={idx} style={{ display: 'flex', alignItems: 'center', justifyContent: 'space-between', padding: '6px 0', borderBottom: '1px solid var(--color-border-light)' }}>
                <div>
                  <strong>{ref.genome || 'unknown'}</strong>
                  <span style={{ color: 'var(--color-text-secondary)', marginLeft: '8px', fontSize: '0.8rem' }}>{ref.components?.join(', ')}</span>
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
                  <button className="btn-sm" style={{ fontSize: '0.7rem' }}>Download</button>
                </div>
              </div>
            ))}
            {!refs && <div style={{ color: 'var(--color-text-secondary)' }}>Loading references...</div>}
          </div>
          <div style={{ marginTop: '0.75rem' }}>
            <button className="btn-sm">+ Add Reference Genome</button>
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

      {/* ── License ── */}
      <Section title="License" icon={<Shield size={16} color="var(--color-primary)" />}>
        <div style={{ fontSize: '0.85rem' }}>
          <div>Type: <strong>{health?.license?.license_type || 'academic'}</strong></div>
          <div>Status: <span className="status-badge success">Valid</span></div>
          <div style={{ marginTop: '4px' }}>Contact: <strong>{health?.license?.contact || 'w_shixiang@163.com'}</strong></div>
          <div style={{ marginTop: '4px', color: 'var(--color-text-secondary)' }}>{health?.license?.message || 'Free for academic use. Commercial use requires authorization.'}</div>
          <div style={{ marginTop: '0.75rem' }}>
            <button className="btn-sm">Upload Commercial License</button>
          </div>
        </div>
      </Section>
    </div>
  );
}
