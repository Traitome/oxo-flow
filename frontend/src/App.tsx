import { BrowserRouter, Routes, Route } from 'react-router-dom';
import { lazy, Suspense } from 'react';
import Layout from './components/Layout';

const Dashboard = lazy(() => import('./pages/Dashboard'));
const PipelineEditor = lazy(() => import('./pages/PipelineEditor'));
const Pipelines = lazy(() => import('./pages/Pipelines'));
const Settings = lazy(() => import('./pages/Settings'));
const ApiDocs = lazy(() => import('./pages/ApiDocs'));
const ChatUI = lazy(() => import('./components/ChatUI'));
const MonitorReport = lazy(() => import('./pages/MonitorReport'));

function PageFallback() {
  return <div style={{ display: 'flex', alignItems: 'center', justifyContent: 'center', height: '50vh' }}>
    <span style={{ color: 'var(--color-text-tertiary)', fontSize: '0.9rem' }}>Loading...</span>
  </div>;
}

export default function App() {
  return (
    <BrowserRouter>
      <Routes>
        <Route element={<Layout />}>
          <Route path="/" element={<Suspense fallback={<PageFallback />}><Dashboard /></Suspense>} />
          <Route path="/editor" element={<Suspense fallback={<PageFallback />}><PipelineEditor /></Suspense>} />
          <Route path="/pipelines" element={<Suspense fallback={<PageFallback />}><Pipelines /></Suspense>} />
          <Route path="/runs" element={<Suspense fallback={<PageFallback />}><MonitorReport /></Suspense>} />
          <Route path="/runs/:id" element={<Suspense fallback={<PageFallback />}><MonitorReport /></Suspense>} />
          <Route path="/monitor" element={<Suspense fallback={<PageFallback />}><MonitorReport /></Suspense>} />
          <Route path="/chat" element={<Suspense fallback={<PageFallback />}><ChatUI /></Suspense>} />
          <Route path="/settings" element={<Suspense fallback={<PageFallback />}><Settings /></Suspense>} />
          <Route path="/docs" element={<Suspense fallback={<PageFallback />}><ApiDocs /></Suspense>} />
        </Route>
      </Routes>
    </BrowserRouter>
  );
}
