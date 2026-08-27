import { Component, type ReactNode } from 'react';
import { Link } from 'react-router-dom';

interface Props {
  /** Human-readable page name shown in the fallback UI and console logs. */
  name: string;
  children: ReactNode;
}

interface State {
  error: Error | null;
}

/**
 * Per-route boundary (issue #208): without it any render error inside a lazy
 * page bubbled past <Suspense> to the single global boundary, tearing down
 * the whole SPA. Scoping one boundary per route keeps other pages alive.
 */
export default class RouteErrorBoundary extends Component<Props, State> {
  state: State = { error: null };

  static getDerivedStateFromError(error: Error): State {
    return { error };
  }

  componentDidCatch(error: Error, info: React.ErrorInfo) {
    console.error(
      `[RouteErrorBoundary:${this.props.name}] render error:`,
      error,
      info.componentStack,
    );
  }

  private reset = () => this.setState({ error: null });

  render() {
    if (!this.state.error) return this.props.children;
    return (
      <div style={{
        display: 'flex',
        flexDirection: 'column',
        alignItems: 'center',
        justifyContent: 'center',
        gap: 12,
        height: '60vh',
      }}>
        <span style={{ fontSize: '2rem' }} role="img" aria-label="error">💥</span>
        <p style={{ color: 'var(--color-text-secondary)', margin: 0 }}>
          This page hit an unexpected error ({this.props.name}). Other pages are unaffected.
        </p>
        <div style={{ display: 'flex', gap: 8 }}>
          <button className="btn-run" onClick={this.reset}>Try again</button>
          <Link to="/" className="btn-run btn-secondary">Go home</Link>
        </div>
      </div>
    );
  }
}
