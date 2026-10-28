import { useCallback, useEffect, useRef, useState } from 'react';
import { Link, NavLink, Outlet, useLocation } from 'react-router-dom';
import {
  LayoutDashboard, GitBranch, PlayCircle, Library, Settings, BookOpen, Menu, X,
  MessageCircle, Users, ShieldCheck, Server, Sun, Moon, PanelLeftClose, PanelLeftOpen,
} from 'lucide-react';
import ResultNotification from './ResultNotification';
import { usePipelineSession } from '../context/PipelineSession';
import { api, apiUrl } from '../api/client';
import { useServerVersion, fetchServerHealth } from '../api/version';
import { useI18n } from '../context/I18n';

// License type renders from the server's own report, never a hardcoded
// label (issue #82 P2-6: commercial deployments were mislabeled "Academic").
function LicenseFooterLabel() {
  const { t } = useI18n();
  const [label, setLabel] = useState<string>('');
  useEffect(() => {
    api.licenseStatus()
      .then((l) => setLabel(l.license_type ? t('layout.license').replace('{{type}}', l.license_type) : t('layout.academicLicense')))
      .catch(() => setLabel(t('layout.academicLicense')));
  }, [t]);
  return <span>{label}</span>;
}

// Single source of navigation (issue #947): the sidebar renders from this
// one definition — the header carries no links, so there is no second nav
// to keep in sync. A section header renders only when at least one of its
// items survives role filtering, so a trimmed role never shows an empty
// section. Role policy preserved from issue #82 P1-15: admins see
// everything; regular users keep Clusters + Settings for their own AI
// config; guests — including the single-user operator in personal mode —
// see everything (the backend enforces 403s).
type NavItem = { to: string; icon: typeof LayoutDashboard; key: string; roles?: string[] };
type NavSection = { key: string; items: NavItem[] };

const NAV: NavSection[] = [
  {
    key: 'nav.section.workspace',
    items: [
      { to: '/', icon: LayoutDashboard, key: 'nav.dashboard' },
      { to: '/editor', icon: GitBranch, key: 'nav.editor' },
      { to: '/pipelines', icon: Library, key: 'nav.pipelines' },
      { to: '/runs', icon: PlayCircle, key: 'nav.runs' },
    ],
  },
  {
    key: 'nav.section.intelligence',
    items: [
      { to: '/chat', icon: MessageCircle, key: 'nav.chat' },
      { to: '/docs', icon: BookOpen, key: 'nav.docs' },
    ],
  },
  {
    key: 'nav.section.infrastructure',
    items: [
      { to: '/clusters', icon: Server, key: 'nav.clusters', roles: ['admin', 'user'] },
    ],
  },
  {
    key: 'nav.section.administration',
    items: [
      { to: '/users', icon: Users, key: 'nav.users', roles: ['admin'] },
      { to: '/audit', icon: ShieldCheck, key: 'nav.audit', roles: ['admin'] },
      { to: '/settings', icon: Settings, key: 'nav.settings', roles: ['admin', 'user'] },
    ],
  },
];

// Per-device sidebar collapse (issue #947): the choice is a UI preference,
// not account state, so it lives in localStorage — never the server.
const SIDEBAR_COLLAPSED_KEY = 'oxo_sidebar_collapsed';

type ServerStatus = 'checking' | 'ok' | 'degraded' | 'down';

function useStatusTitles(t: (key: string) => string): Record<ServerStatus, string> {
  return {
    checking: t('status.checking'),
    ok: t('status.ok'),
    degraded: t('status.degraded'),
    down: t('status.down'),
  };
}

const STATUS_POLL_MS = 30000;

/** Reactive matchMedia so breakpoint switches re-render the shell. */
function useMediaQuery(query: string): boolean {
  const [matches, setMatches] = useState(() => window.matchMedia(query).matches);
  useEffect(() => {
    const mq = window.matchMedia(query);
    const onChange = (e: MediaQueryListEvent) => setMatches(e.matches);
    mq.addEventListener('change', onChange);
    return () => mq.removeEventListener('change', onChange);
  }, [query]);
  return matches;
}

export default function Layout() {
  // Drawer = <1024px overlay navigation; separate from the desktop
  // collapse so a narrow window never flips the user's desktop choice.
  const [drawerOpen, setDrawerOpen] = useState(false);
  const version = useServerVersion();
  const { t, lang, setLang } = useI18n();
  const STATUS_TITLES = useStatusTitles(t);
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
  const refreshIdentity = () => {
    api.authMe()
      .then((me) => {
        setUserName(me.authenticated ? (me.username ?? null) : null);
        setUserRole(me.authenticated ? (me.role ?? 'user') : null);
      })
      .catch(() => { setUserName(null); setUserRole(null); });
  };
  useEffect(() => {
    refreshIdentity();
    // #907 T1: re-check on login/sign-out (client-side navigate() keeps this
    // component mounted, so mount-only deps left the identity chip and the
    // role-trimmed nav stale until a full reload).
    window.addEventListener('oxo:auth-changed', refreshIdentity);
    return () => window.removeEventListener('oxo:auth-changed', refreshIdentity);
  }, []);

  // Role-trimmed navigation (issue #82 P1-15): authenticated viewers see
  // the core flow only; guests — including the single-user operator in
  // personal mode — see the full nav (the backend enforces 403s).
  const visibleSections = NAV
    .map((section) => ({
      ...section,
      items: section.items.filter((item) => !item.roles || userRole === null || item.roles.includes(userRole)),
    }))
    .filter((section) => section.items.length > 0);

  const [serverStatus, setServerStatus] = useState<ServerStatus>('checking');
  const session = usePipelineSession();
  const location = useLocation();

  // Sidebar collapse (issue #947): ≥1280px the user's choice governs
  // (persisted); 1024–1279px is always the icon rail; <1024px swaps the
  // static sidebar for the drawer.
  const isDesktop = useMediaQuery('(min-width: 1024px)');
  const canToggleCollapse = useMediaQuery('(min-width: 1280px)');
  const [userCollapsed, setUserCollapsed] = useState<boolean>(() => localStorage.getItem(SIDEBAR_COLLAPSED_KEY) === '1');
  const collapsed = isDesktop ? (canToggleCollapse ? userCollapsed : true) : false;

  // Ctrl/Cmd+B toggles the collapse (issue #947). Only meaningful where
  // the user's choice governs (≥1280px) — the forced rail (1024–1279px)
  // and drawer mode (<1024px) ignore it rather than write invisible state.
  // Skipped while typing — editable surfaces may bind the chord themselves.
  const toggleCollapsed = useCallback(() => {
    if (!canToggleCollapse) return;
    const next = !userCollapsed;
    localStorage.setItem(SIDEBAR_COLLAPSED_KEY, next ? '1' : '0');
    setUserCollapsed(next);
  }, [canToggleCollapse, userCollapsed]);
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (!(e.ctrlKey || e.metaKey) || e.key.toLowerCase() !== 'b') return;
      const target = e.target as HTMLElement | null;
      if (target && (target.tagName === 'INPUT' || target.tagName === 'TEXTAREA' || target.isContentEditable)) return;
      e.preventDefault();
      toggleCollapsed();
    };
    window.addEventListener('keydown', onKey);
    return () => window.removeEventListener('keydown', onKey);
  }, [toggleCollapsed]);

  // Route changes dismiss the drawer so a navigation never lands on a page
  // hidden behind its own scrim. State adjusted during render (React's
  // recommended pattern for "reset state when X changes") — not in an effect.
  const [lastRoute, setLastRoute] = useState(location.pathname);
  if (lastRoute !== location.pathname) {
    setLastRoute(location.pathname);
    setDrawerOpen(false);
  }

  // Escape closes the drawer (restoring focus to its trigger) or, when no
  // drawer is open, dismisses icon-rail tooltips until the pointer or
  // keyboard focus re-enters the nav (issue #947). Re-subscribed on
  // drawerOpen so the closure sees the live value without touching a ref
  // during render.
  const [tipsSuppressed, setTipsSuppressed] = useState(false);
  const drawerToggleRef = useRef<HTMLButtonElement>(null);
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (e.key !== 'Escape') return;
      if (drawerOpen) {
        setDrawerOpen(false);
        drawerToggleRef.current?.focus();
        return;
      }
      setTipsSuppressed(true);
    };
    window.addEventListener('keydown', onKey);
    return () => window.removeEventListener('keydown', onKey);
  }, [drawerOpen]);

  // Focus trap for the drawer: Tab cycles inside the dialog instead of
  // escaping into the page behind the scrim.
  const onDrawerKeyDown = (e: React.KeyboardEvent, drawerEl: HTMLElement) => {
    if (e.key !== 'Tab') return;
    const focusables = drawerEl.querySelectorAll<HTMLElement>('a[href], button:not([disabled])');
    if (focusables.length === 0) return;
    const first = focusables[0];
    const last = focusables[focusables.length - 1];
    if (e.shiftKey && document.activeElement === first) {
      e.preventDefault();
      last.focus();
    } else if (!e.shiftKey && document.activeElement === last) {
      e.preventDefault();
      first.focus();
    }
  };

  // Move focus into the drawer on open (a11y: keyboard users must not stay
  // on the hidden trigger while the nav shows). An effect keyed on
  // drawerOpen — not an inline ref callback, which React re-runs on every
  // Layout re-render and would steal focus back repeatedly.
  useEffect(() => {
    if (!drawerOpen) return;
    document.getElementById('app-sidebar')?.querySelector<HTMLElement>('.drawer-close')?.focus();
  }, [drawerOpen]);

  const handleSkipToContent = (e: React.MouseEvent<HTMLAnchorElement>) => {
    e.preventDefault();
    const main = document.getElementById('main-content');
    if (!main) return;
    main.focus({ preventScroll: false });
    main.scrollIntoView({ behavior: 'smooth' });
    const previousTitle = document.title;
    document.title = `${t('skip.toContent')} – ${previousTitle.replace(/^[^–]+ – /, '')}`;
    window.setTimeout(() => {
      document.title = previousTitle;
    }, 1200);
  };

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

  const renderNavItem = (item: NavItem) => (
    <NavLink key={item.to} to={item.to} end={item.to === '/'} className={({ isActive }) => `nav-item${isActive ? ' active' : ''}`}>
      <item.icon size={18} aria-hidden="true" /><span className="nav-label">{t(item.key)}</span>
      {/* Icon-rail tooltip: shows on hover AND keyboard focus; Escape
          suppresses until the pointer re-enters the nav (issue #947). */}
      <span className="nav-tip" role="tooltip">{t(item.key)}</span>
      {item.key === 'nav.runs' && session.state.activeRunId && (
        <span className="run-dot" title={t('layout.activeRun')} />
      )}
    </NavLink>
  );

  const renderSections = () => visibleSections.map((section) => (
    <div key={section.key} className="nav-section">
      <span className="nav-section-label">{t(section.key)}</span>
      {section.items.map(renderNavItem)}
    </div>
  ));

  return (
    <div className="app-shell">
      <a
        href="#main-content"
        className="skip-link"
        onClick={handleSkipToContent}
      >
        {t('skip.toContent')}
      </a>
      {/* Header (issue #947): deliberately nav-free — brand on the left,
          status/identity on the right. All navigation lives in the sidebar. */}
      <header className="app-header">
        <div className="header-left">
          <button
            className="drawer-toggle"
            ref={drawerToggleRef}
            onClick={() => setDrawerOpen(true)}
            aria-label={t('login.openMenu')}
            aria-controls="app-sidebar"
          >
            <Menu size={20} aria-hidden="true" />
          </button>
          {/* Repo logo (issue #907: replace the generic flask glyph with the
              project's own hexagon-metro mark; theme-switched variant). */}
          <img
            src={theme === 'dark' ? `${apiUrl('/logo.svg')}` : `${apiUrl('/logo-light.svg')}`}
            alt="" aria-hidden="true"
            style={{ width: 22, height: 25 }}
          />
          <span className="header-brand">oxo-flow</span>
        </div>
        <div className="header-right">
          <button className="btn-sm" onClick={() => setLang(lang === 'en' ? 'zh' : 'en')}
            title={lang === 'en' ? '切换到中文' : 'Switch to English'}>
            {t('lang.toggle')}
          </button>
          <button className="btn-sm" onClick={toggleTheme}
            title={theme === 'dark' ? t('theme.light') : t('theme.dark')}
            aria-label={theme === 'dark' ? t('theme.light') : t('theme.dark')}>
            {/* SVG icons instead of emoji (#907 D2): inherits currentColor,
                renders identically on every platform. */}
            {theme === 'dark' ? <Sun size={16} /> : <Moon size={16} />}
          </button>
          <span id="header-status" role="status" aria-label={STATUS_TITLES[serverStatus]} className={`status-dot ${serverStatus}`} title={STATUS_TITLES[serverStatus]} />
          {userName ? (
            <>
              <span className="header-user" title={t('nav.signedIn')}>{userName}</span>
              <button
                className="header-user"
                style={{ cursor: 'pointer', border: 'none', background: 'none' }}
                title={t('nav.signOut')}
                onClick={() => {
                  // Revoke server-side (#520) before dropping the local copy.
                  // #907 T3: land on /login with an explicit signed-out reason
                  // so the message differs from a session expiry.
                  api.logout().catch(() => {}).finally(() => {
                    localStorage.removeItem('oxo_token');
                    window.location.assign(apiUrl('/login?reason=signed-out'));
                  });
                }}
              >
                {t('nav.signOut')}
              </button>
            </>
          ) : (
            <Link to="/login" className="header-user">{t('nav.guest')}</Link>
          )}
        </div>
      </header>

      {/* Sidebar + Content */}
      <div className="app-body">
        {isDesktop ? (
          <aside
            id="app-sidebar"
            className={`sidebar${collapsed ? ' collapsed' : ''}${tipsSuppressed ? ' tips-off' : ''}`}
          >
            <nav
              className="sidebar-nav"
              aria-label={t('layout.navLabel')}
              onMouseEnter={() => setTipsSuppressed(false)}
              onFocus={() => setTipsSuppressed(false)}
            >
              {renderSections()}
            </nav>
            <button
              className="sidebar-collapse-btn"
              onClick={toggleCollapsed}
              aria-expanded={!collapsed}
              aria-controls="app-sidebar"
              aria-label={collapsed ? t('layout.expandSidebar') : t('layout.collapseSidebar')}
              title={collapsed ? t('layout.expandSidebar') : t('layout.collapseSidebar')}
              style={canToggleCollapse ? undefined : { display: 'none' }}
            >
              {collapsed ? <PanelLeftOpen size={16} aria-hidden="true" /> : <PanelLeftClose size={16} aria-hidden="true" />}
              {!collapsed && <span className="nav-label">{t('layout.collapseSidebar')}</span>}
            </button>
          </aside>
        ) : drawerOpen && (
          <>
            <div className="sidebar-scrim" onClick={() => setDrawerOpen(false)} aria-hidden="true" />
            <aside
              id="app-sidebar"
              className="sidebar drawer"
              onKeyDown={(e) => onDrawerKeyDown(e, e.currentTarget)}
              role="dialog"
              aria-modal="true"
              aria-label={t('layout.navLabel')}
            >
              <div className="drawer-head">
                <span className="header-brand">oxo-flow</span>
                <button className="drawer-close" onClick={() => { setDrawerOpen(false); drawerToggleRef.current?.focus(); }} aria-label={t('login.closeMenu')}>
                  <X size={18} aria-hidden="true" />
                </button>
              </div>
              <nav className="sidebar-nav" aria-label={t('layout.navLabel')}>
                {renderSections()}
              </nav>
            </aside>
          </>
        )}

        <main id="main-content" tabIndex={-1} className="main-content">
          <ResultNotification />
          <Outlet />
        </main>
      </div>

      {/* Footer — the ONLY place version + license render (issue #947). */}
      <footer className="app-footer">
        <span>{version ? `oxo-flow v${version}` : 'oxo-flow'} — {t('footer.tagline')}</span>
        <LicenseFooterLabel />
        <span>{t('footer.contact')}: w_shixiang@163.com</span>
      </footer>
    </div>
  );
}
