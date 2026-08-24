import { Component, type ReactNode } from 'react';

interface Props {
  children: ReactNode;
  fallback?: ReactNode;
}

interface State {
  error: Error | null;
}

/**
 * Class-based error boundary so the whole SPA does not white-screen when
 * a lazy-loaded route or a deep component throws during render.
 */
export default class ErrorBoundary extends Component<Props, State> {
  constructor(props: Props) {
    super(props);
    this.state = { error: null };
  }

  static getDerivedStateFromError(error: Error): State {
    return { error };
  }

  componentDidCatch(error: Error, info: React.ErrorInfo) {
    console.error('ErrorBoundary caught an error:', error, info.componentStack);
  }

  private reload = () => {
    window.location.reload();
  };

  private reset = () => {
    this.setState({ error: null });
  };

  render() {
    const { error } = this.state;
    if (!error) return this.props.children;
    if (this.props.fallback) return this.props.fallback;

    return (
      <div
        style={{
          display: 'flex',
          flexDirection: 'column',
          alignItems: 'center',
          justifyContent: 'center',
          minHeight: '50vh',
          padding: '2rem',
          textAlign: 'center',
          gap: '1rem',
        }}
      >
        <h2 style={{ margin: 0, color: 'var(--color-error)' }}>Something went wrong</h2>
        <p style={{ color: 'var(--color-text-secondary)', maxWidth: 480 }}>
          The application hit an unexpected error. You can reload the page or try resetting the error boundary.
        </p>
        <pre
          style={{
            maxWidth: '100%',
            overflow: 'auto',
            textAlign: 'left',
            padding: '1rem',
            borderRadius: 'var(--radius-sm)',
            background: 'var(--color-bg-secondary)',
            fontSize: '0.8rem',
            color: 'var(--color-text)',
          }}
        >
          {error.message}
        </pre>
        <div style={{ display: 'flex', gap: '0.75rem', flexWrap: 'wrap', justifyContent: 'center' }}>
          <button className="btn-run" onClick={this.reload}>Reload</button>
          <button className="btn-sm" onClick={this.reset}>Reset error boundary</button>
        </div>
      </div>
    );
  }
}
