import { StrictMode } from 'react';
import { createRoot } from 'react-dom/client';
import App from './App';
import { PipelineSessionProvider } from './context/PipelineSession';
import { I18nProvider } from './context/I18n';
import './index.css';

// Dark mode (issue #82 P2-1): explicit choice stored in localStorage;
// otherwise the OS preference governs via the prefers-color-scheme rules.
const savedTheme = localStorage.getItem('oxo_theme');
if (savedTheme === 'dark' || savedTheme === 'light') {
  document.documentElement.dataset.theme = savedTheme;
}

createRoot(document.getElementById('root')!).render(
  <StrictMode>
    <I18nProvider>
      <PipelineSessionProvider>
        <App />
      </PipelineSessionProvider>
    </I18nProvider>
  </StrictMode>
);
