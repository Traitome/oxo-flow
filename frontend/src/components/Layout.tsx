import { NavLink, Outlet } from 'react-router-dom';
import {
  LayoutDashboard, GitBranch, PlayCircle, Library, FlaskConical,
} from 'lucide-react';

const nav = [
  { to: '/', icon: LayoutDashboard, label: 'Dashboard' },
  { to: '/editor', icon: GitBranch, label: 'Pipeline Editor' },
  { to: '/pipelines', icon: Library, label: 'Pipelines' },
  { to: '/runs', icon: PlayCircle, label: 'Runs' },
];

export default function Layout() {
  return (
    <div className="layout">
      <aside className="sidebar">
        <div className="sidebar-header">
          <FlaskConical size={22} />
          <span className="sidebar-title">oxo-flow</span>
        </div>
        <nav className="sidebar-nav">
          {nav.map(({ to, icon: Icon, label }) => (
            <NavLink
              key={to}
              to={to}
              end={to === '/'}
              className={({ isActive }) => `nav-item${isActive ? ' active' : ''}`}
            >
              <Icon size={18} />
              <span>{label}</span>
            </NavLink>
          ))}
        </nav>
      </aside>
      <main className="main-content">
        <Outlet />
      </main>
    </div>
  );
}
