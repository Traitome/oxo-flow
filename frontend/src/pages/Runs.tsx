import { useEffect, useState } from 'react';
import { api, createEventSource } from '../api/client';
import type { RunDetail, SseEvent } from '../api/types';

export default function Runs() {
  const [runs, setRuns] = useState<RunDetail[]>([]);
  const [events, setEvents] = useState<SseEvent[]>([]);
  const [selectedRun, setSelectedRun] = useState<RunDetail | null>(null);

  useEffect(() => {
    api.listRuns().then(setRuns).catch(() => {});
  }, []);

  useEffect(() => {
    const es = createEventSource();
    es.onmessage = (e) => {
      try {
        const ev: SseEvent = JSON.parse(e.data);
        setEvents((prev) => [ev, ...prev].slice(0, 50));
        // Refresh run list when run events occur
        if (ev.type.startsWith('run_')) {
          api.listRuns().then(setRuns).catch(() => {});
        }
      } catch { /* ignore parse errors */ }
    };
    return () => es.close();
  }, []);

  return (
    <div className="page">
      <h1 className="page-title">Runs</h1>

      <div className="section">
        <h2 className="section-title">Run History</h2>
        {runs.length === 0 ? (
          <div className="empty-state">No runs yet.</div>
        ) : (
          <table className="run-table">
            <thead>
              <tr>
                <th>ID</th>
                <th>Workflow</th>
                <th>Status</th>
                <th>Started</th>
                <th>Finished</th>
                <th></th>
              </tr>
            </thead>
            <tbody>
              {runs.map((r) => (
                <tr key={r.id}>
                  <td className="mono">{r.id.slice(0, 8)}</td>
                  <td>{r.workflow_name}</td>
                  <td><span className={`status-badge ${r.status}`}>{r.status}</span></td>
                  <td>{r.started_at ? new Date(r.started_at).toLocaleString() : '-'}</td>
                  <td>{r.finished_at ? new Date(r.finished_at).toLocaleString() : '-'}</td>
                  <td>
                    <button
                      className="btn-sm"
                      onClick={() => api.getRun(r.id).then(setSelectedRun)}
                    >
                      Detail
                    </button>
                  </td>
                </tr>
              ))}
            </tbody>
          </table>
        )}
      </div>

      {selectedRun && (
        <div className="modal-overlay" onClick={() => setSelectedRun(null)}>
          <div className="modal" onClick={(e) => e.stopPropagation()}>
            <h2>Run Detail: {selectedRun.id.slice(0, 8)}</h2>
            <div className="detail-grid">
              <div><strong>Workflow:</strong> {selectedRun.workflow_name}</div>
              <div><strong>Status:</strong> <span className={`status-badge ${selectedRun.status}`}>{selectedRun.status}</span></div>
              <div><strong>Started:</strong> {selectedRun.started_at ? new Date(selectedRun.started_at).toLocaleString() : '-'}</div>
              <div><strong>Finished:</strong> {selectedRun.finished_at ? new Date(selectedRun.finished_at).toLocaleString() : '-'}</div>
            </div>
            {selectedRun.log_tail && (
              <div className="log-block">
                <strong>Log Output:</strong>
                <pre>{selectedRun.log_tail}</pre>
              </div>
            )}
            {selectedRun.output_files.length > 0 && (
              <div>
                <strong>Output Files:</strong>
                <ul className="file-list">
                  {selectedRun.output_files.map((f) => (
                    <li key={f} className="mono">{f}</li>
                  ))}
                </ul>
              </div>
            )}
            <button className="btn-sm" onClick={() => setSelectedRun(null)}>Close</button>
          </div>
        </div>
      )}

      <div className="section">
        <h2 className="section-title">Live Events (SSE)</h2>
        <div className="event-log">
          {events.map((ev, i) => (
            <div key={i} className={`event-entry ${ev.type}`}>
              <span className="event-type">{ev.type}</span>
              <span className="event-time">{new Date(ev.time).toLocaleTimeString()}</span>
            </div>
          ))}
          {events.length === 0 && <div className="empty-state">Waiting for events...</div>}
        </div>
      </div>
    </div>
  );
}
