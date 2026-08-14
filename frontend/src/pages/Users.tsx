import { useEffect, useState } from 'react';
import { Trash2, UserPlus } from 'lucide-react';
import { api } from '../api/client';
import type { UserInfo } from '../api/types';

// User management (issue #79 P1-06): the client functions existed but no
// page called them. Create users with a bcrypt-hashed password (they sign
// in via /api/auth/login), list all users, delete by id.
export default function Users() {
  const [users, setUsers] = useState<UserInfo[]>([]);
  const [error, setError] = useState<string | null>(null);
  const [username, setUsername] = useState('');
  const [password, setPassword] = useState('');
  const [role, setRole] = useState('user');
  const [creating, setCreating] = useState(false);
  const [notice, setNotice] = useState<string | null>(null);

  const reload = () => {
    api
      .listUsers()
      .then(setUsers)
      .catch((e: unknown) => setError(e instanceof Error ? e.message : 'Failed to load users'));
  };
  useEffect(reload, []);

  const handleCreate = async (e: React.FormEvent) => {
    e.preventDefault();
    if (!username.trim() || !password) return;
    setCreating(true);
    setError(null);
    setNotice(null);
    try {
      await api.createUser(username.trim(), role, password);
      setNotice(`User ${username.trim()} created.`);
      setUsername('');
      setPassword('');
      reload();
    } catch (err: unknown) {
      setError(err instanceof Error ? err.message : 'Failed to create user');
    } finally {
      setCreating(false);
    }
  };

  const handleDelete = async (id: string, name: string) => {
    if (!window.confirm(`Delete user ${name}?`)) return;
    try {
      await api.deleteUser(id);
      setNotice(`User ${name} deleted.`);
      reload();
    } catch (err: unknown) {
      setError(err instanceof Error ? err.message : 'Failed to delete user');
    }
  };

  return (
    <div className="page">
      <h1 className="page-title">Users</h1>
      <p className="page-subtitle">
        Accounts created here sign in with their password (bcrypt-hashed).
        Env-var credentials (OXO_FLOW_ADMIN_PASSWORD / _USER_ / _VIEWER_)
        continue to work alongside them.
      </p>

      {notice && <div className="tool-palette-hint">{notice}</div>}
      {error && <div className="tool-palette-hint error">{error}</div>}

      <form onSubmit={handleCreate} className="login-form" style={{ maxWidth: 480, margin: '0 0 1.5rem' }}>
        <div style={{ display: 'flex', gap: 8, flexWrap: 'wrap' }}>
          <label className="inspector-field" style={{ flex: 1, minWidth: 140 }}>
            <span>Username</span>
            <input value={username} onChange={(e) => setUsername(e.target.value)} autoComplete="off" />
          </label>
          <label className="inspector-field" style={{ flex: 1, minWidth: 140 }}>
            <span>Password</span>
            <input type="password" value={password} onChange={(e) => setPassword(e.target.value)} autoComplete="new-password" />
          </label>
          <label className="inspector-field" style={{ minWidth: 120 }}>
            <span>Role</span>
            <select value={role} onChange={(e) => setRole(e.target.value)}>
              <option value="user">user</option>
              <option value="viewer">viewer</option>
              <option value="admin">admin</option>
            </select>
          </label>
        </div>
        <button className="btn-run" type="submit" disabled={creating || !username.trim() || !password}>
          <UserPlus size={14} /> {creating ? 'Creating…' : 'Create user'}
        </button>
      </form>

      <div className="overflow-x">
        <table className="data-table">
          <thead>
            <tr>
              <th>Username</th>
              <th>Role</th>
              <th>Auth type</th>
              <th>Created</th>
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
                    title={`Delete ${u.username}`}
                    onClick={() => handleDelete(u.id, u.username)}
                  >
                    <Trash2 size={14} />
                  </button>
                </td>
              </tr>
            ))}
            {users.length === 0 && (
              <tr>
                <td colSpan={5} style={{ textAlign: 'center', opacity: 0.6 }}>
                  No users
                </td>
              </tr>
            )}
          </tbody>
        </table>
      </div>
    </div>
  );
}
