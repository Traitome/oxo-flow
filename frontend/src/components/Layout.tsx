import { useEffect, useState } from 'react';
import { Link, NavLink, Outlet } from 'react-router-dom';
import { LayoutDashboard, GitBranch, PlayCircle, Library, Settings, BookOpen, FlaskConical, Menu, X, MessageCircle, Users, ShieldCheck, Server } from 'lucide-react';
import ResultNotification from './ResultNotification';
import { usePipelineSession } from '../context/PipelineSession';
import { api } from '../api/client';
import { useServerVersion, fetchServerHealth } from '../api/version';
import { useI18n } from '../context/I18n';

function LicenseFooterLabel() {
  const [label, setLabel] = useState<string>('');
  useEffect(() => {
    api.licenseStatus()
      .then((l) => setLabel(l.license_type ? `${l.license_type} license` : 'academic license'))
      .catch(() => setLabel('academic license'));
  }, []);
  return <span>{label}</span>;
}

type NavItem = { to: string; icon: typeof LayoutDashboard; key: string; roles?: string[] };

const nav: NavItem[] = [
  { to: '/', icon: LayoutDashboard, key: 'nav.dashboard' },
  { to: '/editor', icon: GitBranch, key: 'nav.editor' },
  { to: '/pipelines', icon: Library, key: 'nav.pipelines' },
  { to: '/runs', icon: PlayCircle, key: 'nav.runs' },
  { to: '/chat', icon: MessageCircle, key: 'nav.chat' },
  { to: '/docs', icon: BookOpen, key: 'nav.docs' },
  // Management entries: admins always; regular users see Clusters +
  // Settings for their own AI config; viewers and guests see neither.
  { to: '/clusters', icon: Server, key: 'nav.clusters', roles: ['admin', 'user'] },
  { to: '/users', icon: Users, key: 'nav.users', roles: ['admin'] },
  { to: '/audit', icon: ShieldCheck, key: 'nav.audit', roles: ['admin'] },
  { to: '/settings', icon: Settings, key: 'nav.settings', roles: ['admin', 'user'] },
];

// /monitor was a duplicate of /runs (issue #82 P1-15) — merged.

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
  const { t, lang, setLang } = useI18n();
  const [theme, setThemeState] = useState<string>(
    () => localStorage.getItem('oxo_theme') || (window.matchMedia('(prefers-color-scheme: dark)').matches ? 'dark' : 'light'),
  );
  const toggleTheme = () => {
    const next = theme === 'dark' ? 'light' : 'dark';
    localStorage.setItem('oxo_theme', next);
    document.documentElement.dataset.theme = next;
    setThemeState(next);
  };
  const [userName, setUserName] = useState<string | null>(null);
  const [userRole, setUserRole] = useState<string | null>(null);
  useEffect(() => {
    api.authMe()
      .then((me) => {
        setUserName(me.authenticated ? (me.username ?? null) : null);
        setUserRole(me.authenticated ? (me.role ?? 'user') : null);
      })
      .catch(() => { setUserName(null); setUserRole(null); });
  }, []);

  // Role-trimmed navigation (issue #82 P1-15): authenticated viewers see
  // the core flow only; guests — including the single-user operator in
  // personal mode — see the full nav (the backend enforces 403s).
  const visibleNav = nav.filter((item) => {
    if (!item.roles) return true;
    if (userRole === null) return true;
    return item.roles.includes(userRole);
  });

  const [serverStatus, setServerStatus] = useState<ServerStatus>('checking');
  const session = usePipelineSession();

  useEffect(() => {
    let cancelled = false;
    const check = async (fresh: boolean) => {
      // Fresh polls bypass the cache so the status dot reflects live health;
      // the first check shares the single cached fetch with useServerVersion.
      const res = await fetchServerHealth(fresh);
      if (!cancelled) {
        setServerStatus(res === null ? 'down' : res.status === 'ok' ? 'ok' : res.status === 'degraded' ? 'degraded' : 'down');
      }
    };
    check(false);
    const timer = setInterval(() => check(true), STATUS_POLL_MS);
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
          {visibleNav.map(({ to, key }) => (
            <NavLink key={to} to={to} end={to === '/'} onClick={() => setMenuOpen(false)} className={({ isActive }) => `header-link${isActive ? ' active' : ''}`}>
              {t(key)}
            </NavLink>
          ))}
        </nav>
        <div className="header-right">
          <button className="btn-sm" onClick={() => setLang(lang === 'en' ? 'zh' : 'en')}
            title={lang === 'en' ? '切换到中文' : 'Switch to English'}>
            {t('lang.toggle')}
          </button>
          <button className="btn-sm" onClick={toggleTheme}
            title={theme === 'dark' ? 'Switch to light theme' : 'Switch to dark theme'}>
            {theme === 'dark' ? '☀️' : '🌙'}
          </button>
          <span id="header-status" role="status" aria-label={STATUS_TITLES[serverStatus]} className={`status-dot ${serverStatus}`} title={STATUS_TITLES[serverStatus]} />
          {userName ? (
            <span className="header-user" title={t('nav.signedIn')}>{userName}</span>
          ) : (
            <Link to="/login" className="header-user">{t('nav.guest')}</Link>
          )}
        </div>
      </header>

      {/* Sidebar + Content */}
      <div className="app-body">
        <aside className="sidebar">
          <nav className="sidebar-nav">
            {visibleNav.map(({ to, icon: Icon, key }) => (
              <NavLink key={to} to={to} end={to === '/'} className={({ isActive }) => `nav-item${isActive ? ' active' : ''}`}>
                <Icon size={18} /><span>{t(key)}</span>
                {key === 'nav.runs' && session.state.activeRunId && (
                  <span style={{ marginLeft: 'auto', width: 6, height: 6, borderRadius: '50%', background: 'var(--color-primary)', animation: 'pulse 1.5s infinite' }} title="Active run" />
                )}
              </NavLink>
            ))}
          </nav>
          <div className="sidebar-footer">
            <span>{version ? `oxo-flow v${version}` : 'oxo-flow'}</span>
            {/* License type renders from the server's own report, never a
                hardcoded label (issue #82 P2-6: commercial deployments were
                mislabeled "Academic"). */}
            <LicenseFooterLabel />
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
    </div>
  );
}
