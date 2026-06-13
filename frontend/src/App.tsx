import { BrowserRouter, Routes, Route } from 'react-router-dom';
import Layout from './components/Layout';
import Dashboard from './pages/Dashboard';
import PipelineEditor from './pages/PipelineEditor';
import Pipelines from './pages/Pipelines';
import Runs from './pages/Runs';

export default function App() {
  return (
    <BrowserRouter>
      <Routes>
        <Route element={<Layout />}>
          <Route path="/" element={<Dashboard />} />
          <Route path="/editor" element={<PipelineEditor />} />
          <Route path="/pipelines" element={<Pipelines />} />
          <Route path="/runs" element={<Runs />} />
          <Route path="/runs/:id" element={<Runs />} />
        </Route>
      </Routes>
    </BrowserRouter>
  );
}
