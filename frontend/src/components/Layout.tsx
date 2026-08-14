import { useEffect, useState } from 'react';
import { Link, NavLink, Outlet } from 'react-router-dom';
import { LayoutDashboard, GitBranch, PlayCircle, BarChart3, Library, Settings, BookOpen, FlaskConical, Menu, X, MessageCircle, Users, ShieldCheck, Server } from 'lucide-react';
import Toast from './Toast';
import ResultNotification from './ResultNotification';
import { usePipelineSession } from '../context/PipelineSession';
import { api } from '../api/client';
import { useServerVersion } from '../api/version';

const nav = [
  { to: '/', icon: LayoutDashboard, label: 'Dashboard' },
  { to: '/editor', icon: GitBranch, label: 'Pipeline Editor' },
  { to: '/pipelines', icon: Library, label: 'Pipelines' },
  { to: '/runs', icon: PlayCircle, label: 'Runs' },
  { to: '/chat', icon: MessageCircle, label: 'AI Chat' },
  { to: '/monitor', icon: BarChart3, label: 'Monitor' },
  { to: '/docs', icon: BookOpen, label: 'API Docs' },
  { to: '/clusters', icon: Server, label: 'Clusters' },
  { to: '/users', icon: Users, label: 'Users' },
  { to: '/audit', icon: ShieldCheck, label: 'Audit' },
  { to: '/settings', icon: Settings, label: 'Settings' },
];

type ServerStatus = 'checking' | 'ok' | 'degraded' | 'down';

const STATUS_TITLES: Record<ServerStatus, string> = {
  checking: 'Checking server status...',
  ok: 'Server connected',
  degraded: 'Server degraded',
  down: 'Server unreachable',
};

const STATUS_POLL_MS = 30000;

export default function Layout() {
  const [menuOpen, setMenuOpen] = useState(false);
  const version = useServerVersion();
  const [userName, setUserName] = useState<string | null>(null);
  useEffect(() => {
    api.authMe()
      .then((me) => setUserName(me.authenticated ? (me.username ?? null) : null))
      .catch(() => setUserName(null));
  }, []);

  const [serverStatus, setServerStatus] = useState<ServerStatus>('checking');
  const session = usePipelineSession();

  useEffect(() => {
    let cancelled = false;
    const check = async () => {
      try {
        const res = await api.health();
        if (!cancelled) {
          setServerStatus(res.status === 'ok' ? 'ok' : res.status === 'degraded' ? 'degraded' : 'down');
        }
      } catch {
        if (!cancelled) setServerStatus('down');
      }
    };
    check();
    const timer = setInterval(check, STATUS_POLL_MS);
    return () => { cancelled = true; clearInterval(timer); };
  }, []);

  return (
    <div className="app-shell">
      {/* Header */}
      <header className="app-header">
        <div className="header-left">
          <button className="mobile-menu-btn" onClick={() => setMenuOpen(!menuOpen)} aria-label="Toggle menu">
            {menuOpen ? <X size={20} /> : <Menu size={20} />}
          </button>
          <FlaskConical size={20} />
          <span className="header-brand">oxo-flow</span>
          <span className="header-ver">{version ? `v${version}` : ''}</span>
        </div>
        <nav className={`header-nav${menuOpen ? ' open' : ''}`}>
          {nav.map(({ to, label }) => (
            <NavLink key={to} to={to} end={to === '/'} onClick={() => setMenuOpen(false)} className={({ isActive }) => `header-link${isActive ? ' active' : ''}`}>
              {label}
            </NavLink>
          ))}
        </nav>
        <div className="header-right">
          <span id="header-status" role="status" aria-label={STATUS_TITLES[serverStatus]} className={`status-dot ${serverStatus}`} title={STATUS_TITLES[serverStatus]} />
          {userName ? (
            <span className="header-user" title="Signed in">{userName}</span>
          ) : (
            <Link to="/login" className="header-user">Guest — sign in</Link>
          )}
        </div>
      </header>

      {/* Sidebar + Content */}
      <div className="app-body">
        <aside className="sidebar">
          <nav className="sidebar-nav">
            {nav.map(({ to, icon: Icon, label }) => (
              <NavLink key={to} to={to} end={to === '/'} className={({ isActive }) => `nav-item${isActive ? ' active' : ''}`}>
                <Icon size={18} /><span>{label}</span>
                {label === 'Runs' && session.state.activeRunId && (
                  <span style={{ marginLeft: 'auto', width: 6, height: 6, borderRadius: '50%', background: 'var(--color-primary)', animation: 'pulse 1.5s infinite' }} title="Active run" />
                )}
              </NavLink>
            ))}
          </nav>
          <div className="sidebar-footer">
            <span>{version ? `oxo-flow v${version}` : 'oxo-flow'}</span>
            <span>Academic License</span>
          </div>
        </aside>

        <main className="main-content">
          <ResultNotification />
          <Outlet />
        </main>
      </div>

      {/* Footer */}
      <footer className="app-footer">
        <span>{version ? `oxo-flow v${version}` : 'oxo-flow'} — Academic License. Free for academic use. Commercial use requires authorization.</span>
        <span>Contact: w_shixiang@163.com</span>
      </footer>

      {/* Toast notifications */}
      <Toast />
    </div>
  );
}
