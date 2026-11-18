/**
 * Identity-scoped browser storage (issue #1144 / ledger UX-02).
 *
 * `localStorage` is shared by every account that uses the same browser
 * profile. Draft TOML, chat transcripts, and canvas node positions are
 * user state, so each persisted key carries the authenticated identity the
 * SPA already learns from the server: `POST /api/auth/login` returns
 * `username`, and `GET /api/auth/me` re-states it. The username is cached
 * in `oxo_identity` only so startup can pick the right bucket synchronously;
 * `PipelineSession` corroborates it against `/api/auth/me` before restoring
 * anything whenever a session token is present.
 *
 * Logout wipes every identity-scoped key (all identities, not just the
 * current one) and disables further writes for the rest of the page, so a
 * second account on the same browser cannot read the first account's state
 * and the unload flush cannot resurrect it.
 *
 * Deliberately NOT scoped: per-device UI preferences (theme, language,
 * sidebar collapse, guided/canvas editor mode, onboarding dismissal). They
 * describe the device, not the account.
 */

/** Bearer session token — read by the API client. Never identity-scoped. */
export const AUTH_TOKEN_KEY = 'oxo_token';

/** Cached username of the signed-in identity; `null` when anonymous. */
export const IDENTITY_HINT_KEY = 'oxo_identity';

/**
 * Shared event name for "the signed-in identity changed" (login, logout,
 * or a reconciliation against the server). Login.tsx fires it after writing
 * the hint; Layout.tsx fires it after the logout wipe.
 */
export const AUTH_CHANGED_EVENT = 'oxo:auth-changed';

/** Prefix of every identity-scoped pipeline-session bucket. */
export const SESSION_KEY_PREFIX = 'oxo_session';

/** Prefix shared by canvas-position keys (identity-scoped since #1144). */
export const CANVAS_POSITION_PREFIX = 'oxo-canvas-pos';

/** Scope used when nobody is signed in (personal mode, the login page). */
export const GUEST_SCOPE = 'guest';

/**
 * The pre-#1144 unscoped session key. It cannot be attributed to an
 * identity, so it is discarded on startup instead of migrated.
 */
export const LEGACY_SESSION_KEY = SESSION_KEY_PREFIX;

/** Module-level kill switch for persistence after logout. */
let persistenceSuppressed = false;

/** Scope token for a username; `guest` when there is no identity. */
export function identityScope(username: string | null | undefined): string {
  const name = username?.trim();
  return name ? `user:${encodeURIComponent(name)}` : GUEST_SCOPE;
}

/** Persisted bucket key for one identity scope. */
export function sessionKeyForScope(scope: string): string {
  return `${SESSION_KEY_PREFIX}::${scope}`;
}

/**
 * Persisted canvas-position key for one draft scope (pipeline id or
 * `monitor-<runId>`) owned by one identity.
 */
export function canvasPositionKey(scopeKey: string, scope: string = currentIdentityScope()): string {
  return `${CANVAS_POSITION_PREFIX}::${scope}::${scopeKey}`;
}

/** True when a session token is stored (the browser may be authenticated). */
export function hasSessionToken(): boolean {
  try {
    return !!localStorage.getItem(AUTH_TOKEN_KEY);
  } catch {
    return false;
  }
}

/** The cached identity, or `null` when nobody is signed in. */
export function readIdentityHint(): string | null {
  try {
    const value = localStorage.getItem(IDENTITY_HINT_KEY);
    return value && value.trim() !== '' ? value : null;
  } catch {
    return null;
  }
}

/** Identity scope derived from the cached hint (synchronous). */
export function currentIdentityScope(): string {
  return identityScope(readIdentityHint());
}

/** Cache (or clear) the signed-in identity. */
export function writeIdentityHint(username: string | null): void {
  try {
    if (username?.trim()) localStorage.setItem(IDENTITY_HINT_KEY, username.trim());
    else localStorage.removeItem(IDENTITY_HINT_KEY);
  } catch {
    // Storage unavailable — scoping degrades to the guest bucket.
  }
}

/**
 * True once a logout has wiped this page's state. The provider's unload
 * flush checks it so a navigation-triggered save cannot rewrite the keys
 * the logout just removed.
 */
export function isClientPersistenceSuppressed(): boolean {
  return persistenceSuppressed;
}

/** Stop all further identity-scoped writes for this page. */
export function suppressClientPersistence(): void {
  persistenceSuppressed = true;
}

/**
 * Drop the session token and the cached identity but KEEP every scoped
 * bucket — the recovery path when the server cannot confirm the claimed
 * identity (expired session, unconfirmed notice) and the user signs in
 * again, so their unsaved draft returns with the same account.
 */
export function forgetSessionIdentity(): void {
  try {
    localStorage.removeItem(AUTH_TOKEN_KEY);
    localStorage.removeItem(IDENTITY_HINT_KEY);
  } catch {
    // Storage unavailable — nothing was persisted.
  }
}

/**
 * Logout wipe (issue #1144): remove the token, the cached identity, every
 * identity-scoped session bucket (all identities, not only the current
 * one), every canvas-position key, and the unattributable legacy key.
 * Suppression happens FIRST so no unload/debounce flush can recreate them.
 */
export function clearIdentityScopedStorage(): void {
  suppressClientPersistence();
  try {
    const doomed: string[] = [];
    for (let i = 0; i < localStorage.length; i++) {
      const key = localStorage.key(i);
      if (!key) continue;
      if (
        key === SESSION_KEY_PREFIX ||
        key.startsWith(`${SESSION_KEY_PREFIX}::`) ||
        key.startsWith(CANVAS_POSITION_PREFIX)
      ) {
        doomed.push(key);
      }
    }
    for (const key of doomed) localStorage.removeItem(key);
    localStorage.removeItem(IDENTITY_HINT_KEY);
    localStorage.removeItem(AUTH_TOKEN_KEY);
  } catch {
    // Storage unavailable — nothing was persisted to leak in the first place.
  }
}

/**
 * Drop the unattributable pre-#1144 `oxo_session` blob. It was written
 * browser-wide, so it cannot be proven to belong to the identity that is
 * about to read it; every identity now has its own bucket instead.
 */
export function discardLegacyUnscopedSession(): void {
  try {
    localStorage.removeItem(LEGACY_SESSION_KEY);
  } catch {
    // Storage unavailable — nothing to discard.
  }
}
