import { StrictMode } from 'react';
import { createRoot } from 'react-dom/client';
import App from './App';
import ErrorBoundary from './components/ErrorBoundary';
import { PipelineSessionProvider } from './context/PipelineSession';
import { I18nProvider } from './context/I18n';
import './index.css';
// WCAG contrast corrections from the axe gate (issue #1151); must load after
// index.css so the equal-specificity overrides win.
import './styles/a11y.css';

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
        <ErrorBoundary>
          <App />
        </ErrorBoundary>
      </PipelineSessionProvider>
    </I18nProvider>
  </StrictMode>
);
