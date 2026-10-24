import { useEffect, useState } from 'react';
import { Trash2, UserPlus } from 'lucide-react';
import { api, ApiError } from '../api/client';
import type { UserInfo } from '../api/types';
import { useI18n } from '../context/I18n';

// User management (issue #79 P1-06): the client functions existed but no
// page called them. Create users with a bcrypt-hashed password (they sign
// in via /api/auth/login), list all users, delete by id.
export default function Users() {
  const { t } = useI18n();
  const [users, setUsers] = useState<UserInfo[]>([]);
  const [error, setError] = useState<string | null>(null);
  // #907 T4: a viewer hitting the admin-only listing gets a 403, which the
  // generic empty state used to render as "No users" — a lie. Keep the empty
  // state only when the fetch actually succeeded.
  const [loadDenied, setLoadDenied] = useState(false);
  const [username, setUsername] = useState('');
  const [password, setPassword] = useState('');
  const [role, setRole] = useState('user');
  const [creating, setCreating] = useState(false);
  const [notice, setNotice] = useState<string | null>(null);
  const [currentUser, setCurrentUser] = useState<string | null>(null);
  const [currentUserLoaded, setCurrentUserLoaded] = useState(false);

  useEffect(() => {
    api
      .authMe()
      .then((me) => {
        setCurrentUser(me.authenticated ? me.username ?? null : null);
        setCurrentUserLoaded(true);
      })
      .catch(() => {
        setCurrentUser(null);
        setCurrentUserLoaded(true);
      });
  }, []);

  const reload = () => {
    api
      .listUsers()
      .then((list) => { setUsers(list); setLoadDenied(false); setError(null); })
      .catch((e: unknown) => {
        if (e instanceof ApiError && e.code === 'FORBIDDEN') {
          setLoadDenied(true);
        } else {
          setError(e instanceof Error ? e.message : t('users.loadFailed'));
        }
      });
  };
  useEffect(reload, [t]);

  const handleCreate = async (e: React.FormEvent) => {
    e.preventDefault();
    if (!username.trim() || !password) return;
    setCreating(true);
    setError(null);
    setNotice(null);
    try {
      await api.createUser(username.trim(), role, password);
      setNotice(t('users.created').replace('{{name}}', username.trim()));
      setUsername('');
      setPassword('');
      reload();
    } catch (err: unknown) {
      setError(err instanceof Error ? err.message : t('users.createFailed'));
    } finally {
      setCreating(false);
    }
  };

  const handleDelete = async (id: string, name: string) => {
    if (!window.confirm(t('users.deleteConfirm').replace('{{name}}', name))) return;
    try {
      await api.deleteUser(id);
      setNotice(t('users.deleted').replace('{{name}}', name));
      reload();
    } catch (err: unknown) {
      setError(err instanceof Error ? err.message : t('users.deleteFailed'));
    }
  };

  return (
    <div className="page">
      <h1 className="page-title">{t('users.title')}</h1>
      <p className="page-subtitle">{t('users.subtitle')}</p>

      {notice && <div className="tool-palette-hint">{notice}</div>}
      {error && <div className="tool-palette-hint error">{error}</div>}

      <form onSubmit={handleCreate} className="login-form" style={{ maxWidth: 480, margin: '0 0 1.5rem' }}>
        <div style={{ display: 'flex', gap: 8, flexWrap: 'wrap' }}>
          <label className="inspector-field" style={{ flex: 1, minWidth: 140 }}>
            <span>{t('users.username')}</span>
            <input value={username} onChange={(e) => setUsername(e.target.value)} autoComplete="off" />
          </label>
          <label className="inspector-field" style={{ flex: 1, minWidth: 140 }}>
            <span>{t('users.password')}</span>
            <input type="password" value={password} onChange={(e) => setPassword(e.target.value)} autoComplete="new-password" />
          </label>
          <label className="inspector-field" style={{ minWidth: 120 }}>
            <span>{t('users.role')}</span>
            <select value={role} onChange={(e) => setRole(e.target.value)}>
              <option value="user">{t('users.roles.user')}</option>
              <option value="viewer">{t('users.roles.viewer')}</option>
              <option value="admin">{t('users.roles.admin')}</option>
            </select>
          </label>
        </div>
        <button className="btn-run" type="submit" disabled={creating || !username.trim() || !password}>
          <UserPlus size={14} /> {creating ? t('users.creating') : t('users.create')}
        </button>
      </form>

      <div className="overflow-x">
        <table className="data-table">
          <thead>
            <tr>
              <th>{t('users.username')}</th>
              <th>{t('users.role')}</th>
              <th>{t('users.authType')}</th>
              <th>{t('users.createdAt')}</th>
              <th></th>
            </tr>
          </thead>
          <tbody>
            {users.map((u) => (
              <tr key={u.id}>
                <td>{u.username}</td>
                <td>{u.role}</td>
                <td>{u.auth_type ?? 'password'}</td>
                <td>{u.created_at.slice(0, 10)}</td>
                <td>
                  <button
                    className="icon-btn danger"
                    title={
                      currentUser === u.username
                        ? t('users.deleteSelf')
                        : t('users.deleteAria').replace('{{name}}', u.username)
                    }
                    aria-label={
                      currentUser === u.username
                        ? t('users.deleteSelf')
                        : t('users.deleteAria').replace('{{name}}', u.username)
                    }
                    disabled={!currentUserLoaded || currentUser === u.username}
                    onClick={() => handleDelete(u.id, u.username)}
                  >
                    <Trash2 size={14} />
                  </button>
                </td>
              </tr>
            ))}
            {users.length === 0 && !loadDenied && (
              <tr>
                <td colSpan={5} style={{ textAlign: 'center', opacity: 0.6 }}>
                  {t('users.noUsers')}
                </td>
              </tr>
            )}
          </tbody>
        </table>
      </div>

      {loadDenied && (
        <div className="dash-card" style={{ borderLeft: '3px solid var(--color-warning)', marginTop: '0.5rem' }}>
          <h3 style={{ fontSize: '0.95rem' }}>{t('users.denied.title')}</h3>
          <p style={{ fontSize: '0.85rem', margin: '4px 0 0' }}>{t('users.denied.desc')}</p>
        </div>
      )}
    </div>
  );
}
