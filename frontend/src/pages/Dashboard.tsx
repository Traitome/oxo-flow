import { useEffect, useState } from 'react';
import { Link } from 'react-router-dom';
import { Play, GitBranch, Activity, HardDrive } from 'lucide-react';
import { api } from '../api/client';
import type { HealthResponse, SystemInfo, RunDetail } from '../api/types';

interface StatCard {
  label: string;
  value: string;
  icon: typeof Play;
  color: string;
}

export default function Dashboard() {
  const [health, setHealth] = useState<HealthResponse | null>(null);
  const [sys, setSys] = useState<SystemInfo | null>(null);
  const [runs, setRuns] = useState<RunDetail[]>([]);
  const [error] = useState('');

  useEffect(() => {
    api.health().then(setHealth).catch(() => {});
    api.system().then(setSys).catch(() => {});
    api.listRuns().then(setRuns).catch(() => {});
  }, []);

  const stats: StatCard[] = [
    {
      label: 'Engine Version',
      value: health?.version || '-',
      icon: Activity,
      color: '#4a6fa5',
    },
    {
      label: 'Total Runs',
      value: String(runs.length),
      icon: Play,
      color: '#38a169',
    },
    {
      label: 'Active Runs',
      value: String(runs.filter((r) => r.status === 'running' || r.status === 'pending').length),
      icon: GitBranch,
      color: '#d69e2e',
    },
    {
      label: 'Platform',
      value: sys ? `${sys.os}/${sys.arch}` : '-',
      icon: HardDrive,
      color: '#805ad5',
    },
  ];

  const recentRuns = runs.slice(0, 10);

  return (
    <div className="page">
      <h1 className="page-title">Dashboard</h1>
      {error && <div className="error-banner">{error}</div>}
      <div className="stat-grid">
        {stats.map((s) => (
          <div key={s.label} className="stat-card" style={{ borderLeftColor: s.color }}>
            <div className="stat-icon" style={{ color: s.color }}>
              <s.icon size={24} />
            </div>
            <div className="stat-body">
              <div className="stat-value">{s.value}</div>
              <div className="stat-label">{s.label}</div>
            </div>
          </div>
        ))}
      </div>

      <div className="section">
        <h2 className="section-title">Quick Actions</h2>
        <div className="action-row">
          <Link to="/editor" className="action-btn primary">
            <GitBranch size={18} /> New Pipeline
          </Link>
          <Link to="/editor" className="action-btn">
            <Play size={18} /> Run Workflow
          </Link>
        </div>
      </div>

      <div className="section">
        <h2 className="section-title">Recent Runs</h2>
        {recentRuns.length === 0 ? (
          <div className="empty-state">No runs yet. Create a pipeline to get started.</div>
        ) : (
          <table className="run-table">
            <thead>
              <tr>
                <th>Workflow</th>
                <th>Status</th>
                <th>Started</th>
                <th></th>
              </tr>
            </thead>
            <tbody>
              {recentRuns.map((r) => (
                <tr key={r.id}>
                  <td>{r.workflow_name}</td>
                  <td>
                    <span className={`status-badge ${r.status}`}>{r.status}</span>
                  </td>
                  <td>{r.started_at ? new Date(r.started_at).toLocaleString() : '-'}</td>
                  <td>
                    <Link to={`/runs/${r.id}`} className="view-link">View</Link>
                  </td>
                </tr>
              ))}
            </tbody>
          </table>
        )}
      </div>
    </div>
  );
}
