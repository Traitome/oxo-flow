import { BrowserRouter, Routes, Route, Navigate } from 'react-router-dom';
import { lazy, Suspense, type ReactNode } from 'react';
import Layout from './components/Layout';
import RouteErrorBoundary from './components/RouteErrorBoundary';
import { useI18n } from './context/I18n';

const Dashboard = lazy(() => import('./pages/Dashboard'));
const PipelineEditor = lazy(() => import('./pages/PipelineEditor'));
const Pipelines = lazy(() => import('./pages/Pipelines'));
const Login = lazy(() => import('./pages/Login'));
const Settings = lazy(() => import('./pages/Settings'));
const ApiDocs = lazy(() => import('./pages/ApiDocs'));
const ChatUI = lazy(() => import('./components/ChatUI'));
const MonitorReport = lazy(() => import('./pages/MonitorReport'));
const Users = lazy(() => import('./pages/Users'));
const Audit = lazy(() => import('./pages/Audit'));
const Clusters = lazy(() => import('./pages/Clusters'));
const Share = lazy(() => import('./pages/Share'));

function PageFallback() {
  const { t } = useI18n();
  return <div style={{ display: 'flex', alignItems: 'center', justifyContent: 'center', height: '50vh' }}>
    <span style={{ color: 'var(--color-text-tertiary)', fontSize: '0.9rem' }}>{t('common.loading')}</span>
  </div>;
}

/** Suspense + per-route error boundary so one page's crash stays contained. */
function route(name: string, node: ReactNode): ReactNode {
  return (
    <Suspense fallback={<PageFallback />}>
      <RouteErrorBoundary name={name}>{node}</RouteErrorBoundary>
    </Suspense>
  );
}

// Mount-aware routing: the server injects window.__OXO_BASE__ into
// index.html when the app is served under a sub-path (--base-path); the
// BrowserRouter basename must match or every route 404s the SPA fallback.
declare global {
  interface Window {
    __OXO_BASE__?: string;
  }
}
const appBasename = window.__OXO_BASE__ && window.__OXO_BASE__ !== '/' ? window.__OXO_BASE__ : undefined;

export default function App() {
  return (
    <BrowserRouter basename={appBasename}>
      <Routes>
        {/* Public share landing (issue #82 P0-6): no session, no app chrome */}
        <Route path="/share/:token" element={route('share', <Share />)} />
        <Route element={<Layout />}>
          <Route path="/" element={route('dashboard', <Dashboard />)} />
          <Route path="/editor" element={route('pipelineeditor', <PipelineEditor />)} />
          <Route path="/pipelines" element={route('pipelines', <Pipelines />)} />
          <Route path="/login" element={route('login', <Login />)} />
          <Route path="/templates" element={<Navigate to="/pipelines" replace />} />
          <Route path="/runs" element={route('monitorreport', <MonitorReport />)} />
          <Route path="/runs/:id" element={route('monitorreport', <MonitorReport />)} />
          {/* /monitor merged into /runs (issue #82 P1-15) — redirect for old links */}
          <Route path="/monitor" element={<Navigate to="/runs" replace />} />
          <Route path="/monitor/:id" element={<Navigate to="/runs" replace />} />
          <Route path="/chat" element={route('chat', <ChatUI />)} />
          <Route path="/settings" element={route('settings', <Settings />)} />
          <Route path="/docs" element={route('apidocs', <ApiDocs />)} />
          <Route path="/users" element={route('users', <Users />)} />
          <Route path="/audit" element={route('audit', <Audit />)} />
          <Route path="/clusters" element={route('clusters', <Clusters />)} />
        </Route>
      </Routes>
    </BrowserRouter>
  );
}
