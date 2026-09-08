import { useState } from 'react';
import { useLocation, useNavigate } from 'react-router-dom';
import { KeyRound } from 'lucide-react';
import { api } from '../api/client';
import { useI18n } from '../context/I18n';

export default function Login() {
  const location = useLocation();
  const [username, setUsername] = useState('');
  const [password, setPassword] = useState('');
  // A redirect here can carry the reason (e.g. a share import that needs a
  // session) — show it instead of landing on a blank form.
  const [error, setError] = useState<string | null>(
    (location.state as { notice?: string } | null)?.notice ?? null,
  );
  const [loading, setLoading] = useState(false);
  const navigate = useNavigate();
  const { t } = useI18n();

  const handleSubmit = async (e: React.FormEvent) => {
    e.preventDefault();
    setError(null);
    setLoading(true);
    try {
      const res = await api.login(username.trim(), password);
      localStorage.setItem('oxo_token', res.token);
      // The username is the SSE stream's identity in personal-mode fallback
      // (server-side filtering needs no client state in team mode).
      localStorage.setItem('oxo_user_id', username.trim());
      navigate('/');
    } catch (err: unknown) {
      setError(err instanceof Error ? err.message : t('login.failed'));
    } finally {
      setLoading(false);
    }
  };

  return (
    <div className="page login-wrap">
      <div className="dash-card login-card">
        <h1 className="page-title">{t('login.title')}</h1>
        <form onSubmit={handleSubmit} className="login-form">
          <label className="inspector-field">
            <span>{t('login.username')}</span>
            <input
              value={username}
              onChange={(e) => setUsername(e.target.value)}
              autoFocus
              autoComplete="username"
            />
          </label>
          <label className="inspector-field">
            <span>{t('login.password')}</span>
            <input
              type="password"
              value={password}
              onChange={(e) => setPassword(e.target.value)}
              autoComplete="current-password"
            />
          </label>
          {error && <div className="tool-palette-hint error">{error}</div>}
          <button className="btn-run" type="submit" disabled={loading || !username.trim() || !password}>
            <KeyRound size={14} /> {loading ? t('login.signingIn') : t('login.submit')}
          </button>
          <p className="run-dialog-hint">
            {t('login.hint')}
          </p>
        </form>
      </div>
    </div>
  );
}
