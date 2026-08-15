// Task-oriented homepage (issue #82 P1-7): the first screen answers
// "what do you want to do?" with three big entry cards, an onboarding
// strip for first-time users, and the recent-run stream. System resources
// moved to a compact footer strip — the AI chat is one click away, not
// the entire first screen.

import { useEffect, useState } from 'react';
import { Link, useNavigate } from 'react-router-dom';
import { Bot, FileCode2, PlayCircle, Sparkles } from 'lucide-react';
import { api } from '../api/client';
import type { HealthResponse, SystemInfo, RunItem, Template } from '../api/types';
import { usePipelineSession } from '../context/PipelineSession';
import { useI18n } from '../context/I18n';
import Glossary from '../components/Glossary';

const ONBOARDING_KEY = 'oxo_onboarded';

export default function Dashboard() {
  const navigate = useNavigate();
  const session = usePipelineSession();
  const { t, lang } = useI18n();
  const [health, setHealth] = useState<HealthResponse | null>(null);
  const [sys, setSys] = useState<SystemInfo | null>(null);
  const [runs, setRuns] = useState<RunItem[]>([]);
  const [templates, setTemplates] = useState<Template[]>([]);
  const [showOnboarding, setShowOnboarding] = useState(
    () => localStorage.getItem(ONBOARDING_KEY) !== '1',
  );

  useEffect(() => {
    api.health().then(setHealth).catch(() => {});
    api.system().then(setSys).catch(() => {});
    api.listRuns().then((r) => setRuns(r.items)).catch(() => {});
    api.listTemplates().then(setTemplates).catch(() => {});
  }, []);

  const dismissOnboarding = () => {
    localStorage.setItem(ONBOARDING_KEY, '1');
    setShowOnboarding(false);
  };

  const activeRuns = runs.filter((r) => r.status === 'running' || r.status === 'queued').length;
  // Derived: once the user has real runs, the onboarding strip steps aside
  // even if it was never explicitly dismissed.
  const showOnboardingNow = showOnboarding && runs.length === 0;
  const quickTemplates = templates.slice(0, 4);
  const steps = t('dashboard.onboarding.steps').split('|');

  return (
    <div className="page">
      <h1 className="page-title">{t('dashboard.title')}</h1>
      <p className="page-subtitle">{t('dashboard.subtitle')}</p>

      {/* Onboarding strip for first-time users */}
      {showOnboardingNow && (
        <div className="dash-card" style={{ marginBottom: '1rem', borderLeft: '3px solid var(--color-primary)' }}>
          <div style={{ display: 'flex', justifyContent: 'space-between', alignItems: 'center', flexWrap: 'wrap', gap: '0.5rem' }}>
            <h3 style={{ fontSize: '0.95rem' }}>👋 {t('dashboard.onboarding')}</h3>
            <button className="btn-sm" onClick={dismissOnboarding}>{t('dashboard.onboarding.dismiss')}</button>
          </div>
          <ol style={{ margin: '8px 0 0 1.2rem', fontSize: '0.85rem', lineHeight: 1.7 }}>
            {steps.map((step) => <li key={step}>{step}</li>)}
          </ol>
        </div>
      )}

      {/* Three entry cards: AI / templates / editor */}
      <div className="dashboard-grid" style={{ gridTemplateColumns: 'repeat(auto-fit, minmax(240px, 1fr))', marginBottom: '1rem' }}>
        <button className="dash-card entry-card" onClick={() => navigate('/chat')}>
          <Sparkles size={22} style={{ color: 'var(--color-primary)' }} />
          <h3>{t('dashboard.ai')}</h3>
          <p>{t('dashboard.ai.desc')}</p>
        </button>
        <button className="dash-card entry-card" onClick={() => navigate('/pipelines')}>
          <FileCode2 size={22} style={{ color: 'var(--color-primary)' }} />
          <h3>{t('dashboard.templates')}</h3>
          <p>{t('dashboard.templates.desc')}</p>
        </button>
        <button
          className="dash-card entry-card"
          onClick={() => {
            session.setPipelineToml('');
            navigate('/editor');
          }}
        >
          <PlayCircle size={22} style={{ color: 'var(--color-primary)' }} />
          <h3>{t('dashboard.create')}</h3>
          <p>
            {lang === 'zh' ? (
              <>从零开始设计 <Glossary term="rule">规则</Glossary> 与 <Glossary term="pipeline">流程</Glossary>。</>
            ) : (
              <>Design <Glossary term="rule">rules</Glossary> and a <Glossary term="pipeline">pipeline</Glossary> from scratch.</>
            )}
          </p>
        </button>
      </div>

      {/* Quick templates + recent runs */}
      <div className="dashboard-grid">
        <div className="dash-card">
          <h3 className="dash-card-title">Templates</h3>
          <div className="quick-templates">
            {quickTemplates.map((tpl) => (
              <button key={tpl.id} className="qt-btn" onClick={() => navigate(`/editor?template=${tpl.id}`)}>
                <span className="qt-name">{tpl.name}</span>
                <span className="qt-cat">{tpl.category}</span>
              </button>
            ))}
            <Link to="/pipelines" className="qt-btn qt-more">View all templates →</Link>
          </div>
        </div>
        <div className="dash-card">
          <h3 className="dash-card-title">{t('dashboard.runs')}</h3>
          {runs.length === 0 ? (
            <div className="empty-state">{t('dashboard.noRuns')}</div>
          ) : (
            <table className="run-table">
              <thead><tr><th>Workflow</th><th>Status</th><th>Started</th><th></th></tr></thead>
              <tbody>
                {runs.slice(0, 5).map((r) => (
                  <tr key={r.id}>
                    <td>{r.workflow_name ?? r.id.slice(0, 8)}</td>
                    <td><span className={`status-badge ${r.status}`}>{r.status}</span></td>
                    <td>{r.started_at ? new Date(r.started_at).toLocaleString() : '-'}</td>
                    <td><Link to={`/runs/${r.id}`} className="view-link">View</Link></td>
                  </tr>
                ))}
              </tbody>
            </table>
          )}
        </div>
      </div>

      {/* Compact system strip — data kept, prominence reduced */}
      <div className="stat-grid" style={{ marginTop: '1rem', gridTemplateColumns: 'repeat(auto-fit, minmax(120px, 1fr))' }}>
        <div className="stat-card"><div className="stat-value">{health?.version || '-'}</div><div className="stat-label">Version</div></div>
        <div className="stat-card"><div className="stat-value">{runs.length}</div><div className="stat-label">Total Runs</div></div>
        <div className="stat-card"><div className="stat-value" style={{ color: activeRuns > 0 ? 'var(--color-warning)' : 'var(--color-success)' }}>{activeRuns}</div><div className="stat-label">Active</div></div>
        <div className="stat-card"><div className="stat-value">{sys ? `${sys.os}/${sys.arch}` : '-'}</div><div className="stat-label">Platform</div></div>
        {health?.resources && (
          <div className="stat-card">
            <div className="stat-value">{Math.round(health.resources.memory_used_pct * 100)}%</div>
            <div className="stat-label">Memory</div>
          </div>
        )}
      </div>

      {/* Quick access to the AI chat stays one click away */}
      <div style={{ marginTop: '1rem', fontSize: '0.8rem', color: 'var(--color-text-tertiary)' }}>
        <Bot size={13} style={{ verticalAlign: '-2px' }} /> {t('dashboard.ai')} — <Link to="/chat" style={{ color: 'var(--color-primary)' }}>open the AI chat</Link>
      </div>
    </div>
  );
}
