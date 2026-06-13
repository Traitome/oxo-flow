import { useEffect, useState } from 'react';
import { api } from '../api/client';
import type { AiConfig, HealthResponse } from '../api/types';

export default function Settings() {
  const [health, setHealth] = useState<HealthResponse | null>(null);
  const [aiConfig, setAiConfig] = useState<AiConfig | null>(null);
  const [provider, setProvider] = useState('openai');
  const [apiKey, setApiKey] = useState('');
  const [apiUrl, setApiUrl] = useState('');
  const [model, setModel] = useState('');
  const [testResult, setTestResult] = useState<string | null>(null);
  const [saving, setSaving] = useState(false);

  useEffect(() => {
    api.health().then(setHealth).catch(() => {});
    api.aiConfig().then((c) => { setAiConfig(c); setProvider(c.provider); if (c.api_url) setApiUrl(c.api_url); if (c.model) setModel(c.model); }).catch(() => {});
  }, []);

  const handleSave = async () => {
    setSaving(true);
    try {
      await api.aiUpdateConfig(provider, apiKey || undefined, apiUrl || undefined, model || undefined);
      const c = await api.aiConfig();
      setAiConfig(c);
      setTestResult('✅ Saved. Provider: ' + c.provider);
    } catch (err: unknown) {
      setTestResult('❌ ' + (err instanceof Error ? err.message : 'Save failed'));
    }
    setSaving(false);
  };

  const handleTest = async () => {
    setTestResult('Testing...');
    try {
      const r = await api.aiTest();
      setTestResult(r.success ? '✅ Connected: ' + r.message : '❌ ' + r.message);
    } catch (err: unknown) {
      setTestResult('❌ ' + (err instanceof Error ? err.message : 'Test failed'));
    }
  };

  return (
    <div className="page">
      <h1 className="page-title">Settings</h1>

      {/* AI Configuration */}
      <div className="dash-card" style={{ marginBottom: '1.5rem' }}>
        <h3 className="dash-card-title">AI Provider Configuration</h3>
        <p style={{ fontSize: '0.85rem', color: 'var(--color-text-secondary)', marginBottom: '1rem' }}>
          Configure an AI provider to enable pipeline generation from natural language.
          Config is saved to disk and survives server restarts.
        </p>
        <div style={{ display: 'grid', gap: '0.75rem', maxWidth: '500px' }}>
          <div>
            <label style={{ fontSize: '0.8rem', fontWeight: 500, color: 'var(--color-text-secondary)', display: 'block', marginBottom: '4px' }}>Provider</label>
            <select value={provider} onChange={(e) => setProvider(e.target.value)}
              style={{ width: '100%', padding: '0.5rem', border: '1px solid var(--color-border)', borderRadius: 'var(--radius-sm)', fontSize: '0.9rem', background: 'var(--color-bg)', color: 'var(--color-text)' }}>
              <option value="openai">OpenAI / DeepSeek / Groq (OpenAI-compatible)</option>
              <option value="claude">Claude (Anthropic-compatible)</option>
              <option value="ollama">Ollama (local)</option>
              <option value="disabled">Disabled</option>
            </select>
          </div>
          <div>
            <label style={{ fontSize: '0.8rem', fontWeight: 500, color: 'var(--color-text-secondary)', display: 'block', marginBottom: '4px' }}>API Key</label>
            <input type="password" value={apiKey} onChange={(e) => setApiKey(e.target.value)} placeholder="sk-..."
              style={{ width: '100%', padding: '0.5rem', border: '1px solid var(--color-border)', borderRadius: 'var(--radius-sm)', fontSize: '0.9rem', background: 'var(--color-bg)', color: 'var(--color-text)' }} />
          </div>
          <div>
            <label style={{ fontSize: '0.8rem', fontWeight: 500, color: 'var(--color-text-secondary)', display: 'block', marginBottom: '4px' }}>API URL (optional)</label>
            <input type="text" value={apiUrl} onChange={(e) => setApiUrl(e.target.value)} placeholder="https://api.deepseek.com/v1/chat/completions"
              style={{ width: '100%', padding: '0.5rem', border: '1px solid var(--color-border)', borderRadius: 'var(--radius-sm)', fontSize: '0.9rem', background: 'var(--color-bg)', color: 'var(--color-text)' }} />
          </div>
          <div>
            <label style={{ fontSize: '0.8rem', fontWeight: 500, color: 'var(--color-text-secondary)', display: 'block', marginBottom: '4px' }}>Model</label>
            <input type="text" value={model} onChange={(e) => setModel(e.target.value)} placeholder="deepseek-v4-pro / gpt-4o / claude-sonnet-4-20250514"
              style={{ width: '100%', padding: '0.5rem', border: '1px solid var(--color-border)', borderRadius: 'var(--radius-sm)', fontSize: '0.9rem', background: 'var(--color-bg)', color: 'var(--color-text)' }} />
          </div>
          <div style={{ display: 'flex', gap: '0.5rem' }}>
            <button onClick={handleSave} disabled={saving} className="btn-run">{saving ? 'Saving...' : 'Save Config'}</button>
            <button onClick={handleTest} className="action-btn">Test Connection</button>
          </div>
          {testResult && <div className={`result-bar ${testResult.startsWith('✅') ? 'success' : 'error'}`}>{testResult}</div>}
        </div>
      </div>

      {/* Current Status */}
      <div className="dash-card" style={{ marginBottom: '1.5rem' }}>
        <h3 className="dash-card-title">Current AI Status</h3>
        <div style={{ fontSize: '0.85rem' }}>
          <div>Provider: <strong>{aiConfig?.provider || 'unknown'}</strong></div>
          <div>Model: <strong>{aiConfig?.model || 'default'}</strong></div>
          <div>API URL: <code style={{ fontSize: '0.75rem' }}>{aiConfig?.api_url || 'default'}</code></div>
          <div>Configured: <span className={`status-badge ${aiConfig?.is_configured ? 'success' : 'cancelled'}`}>{aiConfig?.is_configured ? 'Yes' : 'No'}</span></div>
        </div>
      </div>

      {/* System Info */}
      <div className="dash-card">
        <h3 className="dash-card-title">System Information</h3>
        <div style={{ fontSize: '0.85rem' }}>
          <div>Version: <strong>{health?.version || '-'}</strong></div>
          <div>Mode: <strong>{health?.mode || '-'}</strong></div>
          <div>License: <strong>{health?.license?.license_type || '-'}</strong></div>
          <div>Contact: <strong>w_shixiang@163.com</strong></div>
          <div style={{ marginTop: '0.5rem' }}>
            <a href="/api/openapi.json" target="_blank" className="view-link">📄 OpenAPI Specification (JSON)</a>
          </div>
        </div>
      </div>
    </div>
  );
}
