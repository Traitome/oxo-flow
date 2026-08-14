import { StrictMode } from 'react';
import { createRoot } from 'react-dom/client';
import App from './App';
import { PipelineSessionProvider } from './context/PipelineSession';
import { I18nProvider } from './context/I18n';
import './index.css';

createRoot(document.getElementById('root')!).render(
  <StrictMode>
    <I18nProvider>
      <PipelineSessionProvider>
        <App />
      </PipelineSessionProvider>
    </I18nProvider>
  </StrictMode>
);
