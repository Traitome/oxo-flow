import { useEffect, useState } from 'react';
import { api } from './client';
import type { HealthResponse } from './types';

// Server-reported health, fetched once and cached across components.
// Replaces the hardcoded v0.9.2 literals (issue #79: the frontend displayed
// v0.9.2 against a 0.11.0 backend — version display must be single-sourced
// from /api/health so the two can never drift). This module is the single
// cached source: Layout's status poll, the header/sidebar/footer version,
// and ChatUI all share one in-flight request instead of racing on mount.
let cached: HealthResponse | null = null;
let inflight: Promise<HealthResponse | null> | null = null;

/**
 * Fetch /api/health, deduped and cached: concurrent callers share one
 * request, later callers reuse the cached response. `fresh` drops the cache
 * first (periodic polls that need live status, e.g. Layout's 30s check).
 */
export function fetchServerHealth(fresh = false): Promise<HealthResponse | null> {
  if (fresh) cached = null;
  if (cached) return Promise.resolve(cached);
  if (!inflight) {
    inflight = api.health()
      .then((h) => { cached = h; return h; })
      .catch(() => null)
      .finally(() => { inflight = null; });
  }
  return inflight;
}

export function useServerVersion(): string {
  const [version, setVersion] = useState(cached?.version ?? '');
  useEffect(() => {
    let active = true;
    fetchServerHealth().then((h) => { if (active && h) setVersion(h.version); });
    return () => { active = false; };
  }, []);
  return version;
}
