import { StrictMode } from 'react';
import { createRoot } from 'react-dom/client';
import App from './App';
import { PipelineSessionProvider } from './context/PipelineSession';
import './index.css';

createRoot(document.getElementById('root')!).render(
  <StrictMode>
    <PipelineSessionProvider>
      <App />
    </PipelineSessionProvider>
  </StrictMode>
);
