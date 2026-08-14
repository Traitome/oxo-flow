import { useEffect, useState } from 'react';
import { api } from '../api/client';
import type { AuditLogResponse } from '../api/types';

// Audit trail (issue #79 P1-05): every state-changing request is recorded
// server-side; this page renders the trail. Previously no page called the
// audit client function, so the trail was invisible even when written.
export default function Audit() {
  const [data, setData] = useState<AuditLogResponse | null>(null);
  const [days, setDays] = useState(7);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    api
      .audit(days)
      .then(setData)
      .catch((e: unknown) => setError(e instanceof Error ? e.message : 'Failed to load audit log'));
  }, [days]);

  return (
    <div className="page">
      <h1 className="page-title">Audit Trail</h1>
      <p className="page-subtitle">
        All mutations (create/update/delete/run actions) with the acting user
        and outcome. Read-only requests are not recorded.
      </p>

      <label className="inspector-field" style={{ maxWidth: 200 }}>
        <span>Lookback (days)</span>
        <select value={days} onChange={(e) => setDays(Number(e.target.value))}>
          {[1, 3, 7, 14, 30].map((d) => (
            <option key={d} value={d}>
              {d}
            </option>
          ))}
        </select>
      </label>

      {error && <div className="tool-palette-hint error">{error}</div>}

      <div className="overflow-x">
        <table className="data-table">
          <thead>
            <tr>
              <th>Time</th>
              <th>User</th>
              <th>Action</th>
              <th>Resource</th>
              <th>Result</th>
            </tr>
          </thead>
          <tbody>
            {(data?.entries ?? []).map((e, i) => (
              <tr key={i}>
                <td>{e.timestamp.replace('T', ' ').slice(0, 19)}</td>
                <td>{e.user}</td>
                <td>{e.action}</td>
                <td>{e.resource}</td>
                <td>
                  <span className={`status-badge ${e.result === 'success' ? 'completed' : 'failed'}`}>
                    {e.result}
                  </span>
                </td>
              </tr>
            ))}
            {(data?.entries ?? []).length === 0 && !error && (
              <tr>
                <td colSpan={5} style={{ textAlign: 'center', opacity: 0.6 }}>
                  No audit entries in the last {days} days
                </td>
              </tr>
            )}
          </tbody>
        </table>
      </div>
    </div>
  );
}
