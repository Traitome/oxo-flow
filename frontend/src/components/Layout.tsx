import { useState } from 'react';
import { NavLink, Outlet } from 'react-router-dom';
import { LayoutDashboard, GitBranch, PlayCircle, Library, Settings, BookOpen, FlaskConical, Menu, X } from 'lucide-react';
import Toast from './Toast';

const nav = [
  { to: '/', icon: LayoutDashboard, label: 'Dashboard' },
  { to: '/editor', icon: GitBranch, label: 'Pipeline Editor' },
  { to: '/pipelines', icon: Library, label: 'Pipelines' },
  { to: '/runs', icon: PlayCircle, label: 'Runs' },
  { to: '/monitor', icon: PlayCircle, label: 'Monitor' },
  { to: '/docs', icon: BookOpen, label: 'API Docs' },
  { to: '/settings', icon: Settings, label: 'Settings' },
];

export default function Layout() {
  const [menuOpen, setMenuOpen] = useState(false);

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
          <span className="header-ver">v0.8</span>
        </div>
        <nav className={`header-nav${menuOpen ? ' open' : ''}`}>
          {nav.map(({ to, label }) => (
            <NavLink key={to} to={to} end={to === '/'} onClick={() => setMenuOpen(false)} className={({ isActive }) => `header-link${isActive ? ' active' : ''}`}>
              {label}
            </NavLink>
          ))}
        </nav>
        <div className="header-right">
          <span id="header-status" className="status-dot ok" title="Server connected" />
          <span className="header-user">Guest</span>
        </div>
      </header>

      {/* Sidebar + Content */}
      <div className="app-body">
        <aside className="sidebar">
          <nav className="sidebar-nav">
            {nav.map(({ to, icon: Icon, label }) => (
              <NavLink key={to} to={to} end={to === '/'} className={({ isActive }) => `nav-item${isActive ? ' active' : ''}`}>
                <Icon size={18} /><span>{label}</span>
              </NavLink>
            ))}
          </nav>
          <div className="sidebar-footer">
            <span>oxo-flow v0.8.0</span>
            <span>Academic License</span>
          </div>
        </aside>

        <main className="main-content">
          <Outlet />
        </main>
      </div>

      {/* Footer */}
      <footer className="app-footer">
        <span>oxo-flow v0.8.0 — Academic License. Free for academic use. Commercial use requires authorization.</span>
        <span>Contact: w_shixiang@163.com</span>
      </footer>

      {/* Toast notifications */}
      <Toast />
    </div>
  );
}
