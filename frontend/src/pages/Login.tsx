import { useState } from 'react';
import { useNavigate } from 'react-router-dom';
import { KeyRound } from 'lucide-react';
import { api } from '../api/client';

export default function Login() {
  const [username, setUsername] = useState('');
  const [password, setPassword] = useState('');
  const [error, setError] = useState<string | null>(null);
  const [loading, setLoading] = useState(false);
  const navigate = useNavigate();

  const handleSubmit = async (e: React.FormEvent) => {
    e.preventDefault();
    setError(null);
    setLoading(true);
    try {
      const res = await api.login(username.trim(), password);
      localStorage.setItem('oxo_token', res.token);
      navigate('/');
    } catch (err: unknown) {
      setError(err instanceof Error ? err.message : 'Login failed');
    } finally {
      setLoading(false);
    }
  };

  return (
    <div className="page" style={{ maxWidth: 380, margin: '0 auto' }}>
      <h1 className="page-title">Sign in</h1>
      <form onSubmit={handleSubmit} className="login-form">
        <label className="inspector-field">
          <span>Username</span>
          <input
            value={username}
            onChange={(e) => setUsername(e.target.value)}
            autoFocus
            autoComplete="username"
          />
        </label>
        <label className="inspector-field">
          <span>Password</span>
          <input
            type="password"
            value={password}
            onChange={(e) => setPassword(e.target.value)}
            autoComplete="current-password"
          />
        </label>
        {error && <div className="tool-palette-hint error">{error}</div>}
        <button className="btn-run" type="submit" disabled={loading || !username.trim() || !password}>
          <KeyRound size={14} /> {loading ? 'Signing in…' : 'Sign in'}
        </button>
        <p className="run-dialog-hint">
          Team/HPC servers require a session for protected endpoints. Personal
          mode does not need a login.
        </p>
      </form>
    </div>
  );
}
