import { useEffect, useState } from 'react';
import { api } from './client';

// Server-reported version, fetched once and cached across components.
// Replaces the hardcoded v0.9.2 literals (issue #79: the frontend displayed
// v0.9.2 against a 0.11.0 backend — version display must be single-sourced
// from /api/health so the two can never drift).
let cached: string | null = null;

export function useServerVersion(): string {
  const [version, setVersion] = useState(cached ?? '');
  useEffect(() => {
    if (cached !== null) return;
    api
      .health()
      .then((h) => {
        cached = h.version;
        setVersion(h.version);
      })
      .catch(() => setVersion(''));
  }, []);
  return version;
}
