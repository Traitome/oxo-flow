import { createContext, useCallback, useContext, useEffect, useMemo, useReducer, useRef, useState, type Dispatch, type ReactNode } from 'react';
import type { DagJson } from '../api/types';
import { api, apiUrl } from '../api/client';
import {
  AUTH_CHANGED_EVENT,
  AUTH_TOKEN_KEY,
  GUEST_SCOPE,
  IDENTITY_HINT_KEY,
  currentIdentityScope,
  discardLegacyUnscopedSession,
  forgetSessionIdentity,
  hasSessionToken,
  identityScope,
  isClientPersistenceSuppressed,
  readIdentityHint,
  sessionKeyForScope,
  suppressClientPersistence,
  writeIdentityHint,
} from '../utils/identityStorage';

// ── Types ──

export type ChatContextType = 'dashboard' | 'editor' | 'monitor' | 'report';

export interface ChatMessageToolCall {
  id: string;
  name: string;
  args: string;
  summary?: string;
}

/** Structured round progress from the `progress` SSE event. */
export interface ChatProgress {
  round: number;
  max_rounds: number;
  step: string;
  tool?: string | null;
  elapsed_ms: number;
  prompt_tokens: number;
  completion_tokens: number;
}

export interface ChatMessage {
  id: string;
  role: 'user' | 'assistant' | 'agent' | 'system';
  content: string;
  actions?: ChatAction[];
  agentStatus?: string;
  /** Latest round-progress frame (agent loop observability, issue #1040 B2). */
  progress?: ChatProgress;
  /** Grounded tool calls the agent made (rendered as collapsible cards). */
  toolCalls?: ChatMessageToolCall[];
  /** How the server routed this turn (issue #1040 P2-4): a conversational
   * text reply versus a generated pipeline. Chat turns never grow pipeline
   * cards. */
  mode?: 'chat' | 'pipeline';
}

export interface ChatAction {
  type: string;
  label: string;
  action: string;
  data?: unknown;
}

export interface RunResult {
  runId?: string;
  message: string;
  type: 'success' | 'error' | 'info';
}

export interface PipelineSessionState {
  pipelineToml: string;
  dagData: DagJson | null;
  activeRunId: string | null;
  lastRunResult: RunResult | null;
  chatContext: ChatContextType;
  chatMessages: Record<ChatContextType, ChatMessage[]>;
}

// ── Actions ──

type SessionAction =
  | { type: 'SET_PIPELINE_TOML'; payload: string }
  | { type: 'SET_DAG_DATA'; payload: DagJson | null }
  | { type: 'SET_ACTIVE_RUN_ID'; payload: string | null }
  | { type: 'SET_RUN_RESULT'; payload: RunResult | null }
  | { type: 'SET_CHAT_CONTEXT'; payload: ChatContextType }
  | { type: 'SET_CHAT_MESSAGES'; payload: { context: ChatContextType; messages: ChatMessage[] } }
  | { type: 'CLEAR_SESSION' }
  | { type: 'RESTORE_STATE'; payload: Partial<PipelineSessionState> };

// ── localStorage buckets ──
//
// #1144 (ledger UX-02): drafts and chat transcripts used to live in ONE
// browser-wide `oxo_session` blob — a second account on the same browser
// read the first account's state after a logout/login. Every bucket is now
// keyed by the identity the SPA learns from the server (`oxo_identity`,
// written and cleared in the same operations as `oxo_token`), with a
// separate `guest` bucket for anonymous use. The logout path wipes them all
// (see `clearIdentityScopedStorage`).

// #547: two tabs used to last-writer-wins one whole blob — tab B's older
// snapshot resurrected over tab A's newer draft on reload. Each write now
// carries a monotonic stamp; a flush never overwrites a NEWER foreign
// write, and other tabs pull the newer state via the `storage` event.
let writeStamp = 0;

function readStamp(scope: string): number {
  try {
    const raw = localStorage.getItem(sessionKeyForScope(scope));
    return raw ? (JSON.parse(raw)._stamp as number ?? 0) : 0;
  } catch {
    return 0;
  }
}

function emptyChatMessages(): Record<ChatContextType, ChatMessage[]> {
  return { dashboard: [], editor: [], monitor: [], report: [] };
}

function loadStoredState(scope: string): Partial<PipelineSessionState> {
  const key = sessionKeyForScope(scope);
  try {
    const raw = localStorage.getItem(key);
    if (raw) {
      const parsed = JSON.parse(raw);
      // Only restore specific fields (not transient ones like lastRunResult)
      return {
        pipelineToml: parsed.pipelineToml || '',
        dagData: parsed.dagData || null,
        activeRunId: parsed.activeRunId || null,
        chatMessages: parsed.chatMessages || undefined,
      };
    }
  } catch {
    // Corrupt localStorage — clear this identity's bucket only
    try {
      localStorage.removeItem(key);
    } catch {
      // Storage unavailable — nothing to clear.
    }
  }
  return {};
}

/** True when the session holds nothing worth persisting. */
function isPristine(state: PipelineSessionState): boolean {
  return (
    state.pipelineToml === '' &&
    state.dagData === null &&
    state.activeRunId === null &&
    Object.values(state.chatMessages).every((messages) => messages.length === 0)
  );
}

function saveState(state: PipelineSessionState, scope: string) {
  // After logout the page is on its way to /login; an unload flush must not
  // recreate the keys the logout wiped (#1144).
  if (isClientPersistenceSuppressed()) return;
  // #1144 D1: only ever write to the bucket the browser currently claims to
  // be. A logout — in this tab or another — removes the cached identity, so
  // an in-flight debounce or unload flush from a background tab can no
  // longer resurrect the wiped bucket. The guest bucket has no identity to
  // protect and stays writable.
  if (scope !== GUEST_SCOPE && scope !== currentIdentityScope()) return;
  try {
    const key = sessionKeyForScope(scope);
    if (isPristine(state)) {
      // An emptied session is stored as "absent", not as a stale blob — a
      // cleared draft must not come back on reload.
      localStorage.removeItem(key);
      return;
    }
    writeStamp = Math.max(writeStamp, readStamp(scope)) + 1;
    localStorage.setItem(key, JSON.stringify({
      _stamp: writeStamp,
      pipelineToml: state.pipelineToml,
      dagData: state.dagData,
      activeRunId: state.activeRunId,
      chatMessages: state.chatMessages,
    }));
  } catch {
    // localStorage full or unavailable — silently degrade
  }
}

// ── Initial state ──

const INITIAL: PipelineSessionState = {
  pipelineToml: '',
  dagData: null,
  activeRunId: null,
  lastRunResult: null,
  chatContext: 'dashboard',
  chatMessages: emptyChatMessages(),
};

function createEmptyState(): PipelineSessionState {
  return { ...INITIAL, chatMessages: emptyChatMessages() };
}

function createStateFromStorage(scope: string): PipelineSessionState {
  const stored = loadStoredState(scope);
  return {
    ...INITIAL,
    ...stored,
    chatMessages: stored.chatMessages || emptyChatMessages(),
  };
}

function reducer(state: PipelineSessionState, action: SessionAction): PipelineSessionState {
  switch (action.type) {
    case 'SET_PIPELINE_TOML':
      return { ...state, pipelineToml: action.payload };
    case 'SET_DAG_DATA':
      return { ...state, dagData: action.payload };
    case 'SET_ACTIVE_RUN_ID':
      return { ...state, activeRunId: action.payload };
    case 'SET_RUN_RESULT':
      return { ...state, lastRunResult: action.payload };
    case 'SET_CHAT_CONTEXT':
      return { ...state, chatContext: action.payload };
    case 'SET_CHAT_MESSAGES': {
      const { context, messages } = action.payload;
      // Cap at 50 messages per context to avoid unbounded growth
      const capped = messages.length > 50 ? messages.slice(messages.length - 50) : messages;
      return {
        ...state,
        chatMessages: { ...state.chatMessages, [context]: capped },
      };
    }
    case 'RESTORE_STATE':
      return { ...state, ...action.payload };
    case 'CLEAR_SESSION':
      // Storage removal happens in the `clearSession` callback (it needs the
      // active identity scope); the reducer stays pure.
      return { ...INITIAL, chatMessages: emptyChatMessages() };
  }
}

// ── Context ──

interface SessionContextValue {
  state: PipelineSessionState;
  dispatch: Dispatch<SessionAction>;
  setPipelineToml: (toml: string) => void;
  setDagData: (dag: DagJson | null) => void;
  setActiveRunId: (id: string | null) => void;
  setRunResult: (result: RunResult | null) => void;
  setChatContext: (ctx: ChatContextType) => void;
  setChatMessages: (ctx: ChatContextType, msgs: ChatMessage[]) => void;
  clearSession: () => void;
}

const SessionCtx = createContext<SessionContextValue | null>(null);

/**
 * Cap on the `/api/auth/me` confirmation that gates the first paint when the
 * browser claims a signed-in identity (#1144 D3). Past it the app fails
 * closed with the "Session not confirmed" notice — it never renders from
 * the unverified cached identity — and a late answer still adopts the
 * confirmed owner.
 */
const IDENTITY_CONFIRM_TIMEOUT_MS = 5000;

/**
 * Gate in front of the app (#1144 D3). `unconfirmed` is a terminal failure
 * state until a late confirmation arrives or the user retries/signs in:
 * no bucket is restored and no draft is rendered.
 */
type IdentityStatus = 'confirming' | 'ready' | 'unconfirmed';

export function PipelineSessionProvider({ children }: { children: ReactNode }) {
  // #1144: the bucket is chosen from `oxo_identity` — the cached username
  // written by Login.tsx alongside the token and cleared by logout / a
  // rejected session. A browser that holds a token, or claims an identity,
  // is NEVER trusted for the first paint (D3): the provider renders nothing
  // until `GET /api/auth/me` confirms who the token belongs to. Only a
  // CONFIRMED match restores a bucket — a stale or forged hint cannot paint
  // another account's draft, and if confirmation fails or times out the app
  // fails closed (no drafts, an unconfirmed notice) instead of falling back
  // to the cached identity. A fully anonymous browser has nothing to confirm
  // and restores the guest bucket synchronously.
  const [boot] = useState(() => {
    // Pre-#1144 the blob was browser-wide and cannot be attributed to any
    // identity; discard it rather than hand it to whoever loads next.
    discardLegacyUnscopedSession();
    const mustConfirm = hasSessionToken() || readIdentityHint() !== null;
    // Until a scope is confirmed the session belongs to nobody: start on the
    // guest bucket (only read when no confirmation is needed).
    return { scope: GUEST_SCOPE, mustConfirm };
  });
  const scopeRef = useRef<string>(boot.scope);
  const sessionKeyRef = useRef<string>(sessionKeyForScope(boot.scope));
  // While confirming, NOTHING is read from storage — not even the guest
  // bucket — so a forged hint cannot surface foreign content on first paint.
  const [state, dispatch] = useReducer(reducer, undefined, () =>
    boot.mustConfirm ? createEmptyState() : createStateFromStorage(boot.scope));
  /**
   * 'confirming' → nothing renders yet; 'ready' → the app renders the
   * confirmed bucket; 'unconfirmed' → the server could not say who is signed
   * in, so NO bucket is restored and an explicit notice is shown (fail
   * closed). A late confirmation promotes 'unconfirmed' to 'ready'.
   */
  const [status, setStatus] = useState<IdentityStatus>(boot.mustConfirm ? 'confirming' : 'ready');
  // Snapshot used by the unload flush; kept in sync with every render and
  // updated synchronously by `applyScope` before a re-key's dispatch lands.
  const stateRef = useRef(state);

  /** Load one bucket into the live session (no identity change involved). */
  const restoreFromBucket = useCallback((scope: string) => {
    writeStamp = readStamp(scope);
    const restored = createStateFromStorage(scope);
    stateRef.current = restored;
    dispatch({ type: 'RESTORE_STATE', payload: restored });
  }, []);

  /**
   * Move the session to another identity bucket (login, logout, a cross-tab
   * identity swap, or the startup reconciliation). The outgoing state is
   * NOT flushed into the new bucket: `stateRef` is updated synchronously so
   * even an unload in between can only ever write the restored state.
   */
  const applyScope = useCallback((nextScope: string) => {
    const previous = scopeRef.current;
    if (nextScope === previous) return;
    // Replacing or removing a SIGNED-IN identity invalidates everything the
    // mounted pages hold in memory (an open editor keeps its own copy of the
    // draft and would push it into the new bucket). Re-initialize the app
    // from the new bucket instead. Two exceptions:
    //  - the login page owns the guest→user transition; the routed pages are
    //    not mounted there and remount after its navigate('/'),
    //  - logout suppresses persistence and navigates to /login itself.
    const onLoginRoute = window.location.pathname.replace(/\/+$/, '').endsWith('/login');
    if (previous !== GUEST_SCOPE && !onLoginRoute) {
      // #1144 D1 (review finding): a cross-tab logout reaches this branch,
      // and the reload's `beforeunload` flush used to rewrite the bucket the
      // other tab had just wiped. Suppress ALL writes first, then re-point
      // the refs synchronously, so nothing this tab emits can resurrect the
      // old identity's state — whichever tab signed out.
      suppressClientPersistence();
      scopeRef.current = nextScope;
      sessionKeyRef.current = sessionKeyForScope(nextScope);
      writeStamp = readStamp(nextScope);
      const restored = createStateFromStorage(nextScope);
      stateRef.current = restored;
      window.location.reload();
      return;
    }
    scopeRef.current = nextScope;
    sessionKeyRef.current = sessionKeyForScope(nextScope);
    restoreFromBucket(nextScope);
  }, [restoreFromBucket]);

  // Persist key state to localStorage — trailing 500ms debounce: streamed
  // chat chunks dispatch SET_CHAT_MESSAGES many times per second, and
  // stringifying the whole history per chunk janked the UI. The latest
  // state is flushed on unmount/page unload so the tail isn't lost. Each
  // scheduled write carries the scope it was scheduled under, so a re-key
  // can never redirect an older write into the new identity's bucket.
  useEffect(() => {
    stateRef.current = state;
    // While the identity is unconfirmed (D3) nothing is loaded, so nothing
    // may be written — a pristine save would DELETE a bucket we never read.
    if (status !== 'ready') return;
    const scope = scopeRef.current;
    const timer = setTimeout(() => saveState(state, scope), 500);
    return () => clearTimeout(timer);
  }, [state, status]);
  useEffect(() => {
    const flush = () => {
      // Same rule as the debounce: an unconfirmed session owns no bucket.
      if (status !== 'ready') return;
      const scope = scopeRef.current;
      // #547: never flush-on-unmount over a NEWER write from another tab —
      // that resurrection was the whole last-writer-wins bug. The
      // saveState stamp takes max(existing)+1 for OUR writes; a foreign
      // newer write means our state is older and must be dropped.
      if (readStamp(scope) > writeStamp) {
        writeStamp = readStamp(scope);
        return;
      }
      saveState(stateRef.current, scope);
    };
    window.addEventListener('beforeunload', flush);
    // Live cross-tab sync: a newer write from another tab pulls that state
    // in instead of leaving this tab stale. Identity changes are followed
    // too — localStorage is browser-wide, so a login or logout in another
    // tab has already swapped the token this tab would send.
    const onStorage = (e: StorageEvent) => {
      if (e.key === IDENTITY_HINT_KEY || e.key === AUTH_TOKEN_KEY) {
        applyScope(currentIdentityScope());
        return;
      }
      if (e.key === null) {
        // localStorage.clear() from another tab.
        dispatch({ type: 'RESTORE_STATE', payload: createStateFromStorage(scopeRef.current) });
        return;
      }
      if (e.key === sessionKeyRef.current && e.newValue) {
        try {
          const parsed = JSON.parse(e.newValue);
          const foreign = parsed._stamp as number ?? 0;
          if (foreign > writeStamp) {
            writeStamp = foreign;
            dispatch({ type: 'RESTORE_STATE', payload: {
              pipelineToml: parsed.pipelineToml || '',
              dagData: parsed.dagData || null,
              activeRunId: parsed.activeRunId || null,
              chatMessages: parsed.chatMessages || undefined,
            } });
          }
        } catch { /* corrupt foreign write — ignore */ }
      }
    };
    window.addEventListener('storage', onStorage);
    return () => {
      window.removeEventListener('beforeunload', flush);
      window.removeEventListener('storage', onStorage);
      flush();
    };
  }, [applyScope, status]);

  // Re-key on an in-page login/logout, and corroborate the cached identity
  // with the server's answer once per page load. A mismatch means the hint
  // is stale (an older client, a crashed logout, a revoked session): adopt
  // the server's identity and drop the mismatched bucket's in-memory state.
  //
  // #1144 D3 (review round 2): FAIL CLOSED. If the server cannot say who is
  // signed in (timeout or network error), NO bucket is restored — not even
  // the cached identity's — and an explicit unconfirmed notice replaces the
  // app, because the cached identity is exactly what an attacker or a stale
  // client could have forged. A late answer is still adopted: the request is
  // not abandoned on the cap, it promotes the notice to the confirmed app
  // (or, for "nobody", to the guest session) whenever it lands.
  const confirmSeq = useRef(0);
  // Starts the request and schedules the cap; it never calls setState
  // synchronously, so an effect may start it without a render loop.
  const runConfirmation = useCallback(() => {
    const seq = ++confirmSeq.current;
    const cap = setTimeout(() => {
      if (seq === confirmSeq.current) setStatus('unconfirmed');
    }, IDENTITY_CONFIRM_TIMEOUT_MS);
    api.authMe()
      .then((me) => {
        if (seq !== confirmSeq.current) return;
        clearTimeout(cap);
        const serverIdentity = me.authenticated ? (me.username ?? null) : null;
        writeIdentityHint(serverIdentity);
        const nextScope = identityScope(serverIdentity);
        if (nextScope !== scopeRef.current) {
          applyScope(nextScope);
        } else {
          // Same scope, now confirmed: load it only at this point.
          restoreFromBucket(nextScope);
        }
        setStatus('ready');
      })
      .catch(() => {
        if (seq !== confirmSeq.current) return;
        clearTimeout(cap);
        // Server unreachable: keep listening for a late answer (the request
        // may still resolve) but render no drafts in the meantime.
        setStatus('unconfirmed');
      });
  }, [applyScope, restoreFromBucket]);

  /** Retry the confirmation from the unconfirmed notice. */
  const retryConfirmation = useCallback(() => {
    setStatus('confirming');
    runConfirmation();
  }, [runConfirmation]);

  useEffect(() => {
    const onAuthChanged = () => applyScope(currentIdentityScope());
    window.addEventListener(AUTH_CHANGED_EVENT, onAuthChanged);
    // An anonymous browser has nothing to confirm: `/api/auth/me` can only
    // answer "nobody", which the guest scope already reflects, and the
    // status already starts as 'ready'. Skipping the roundtrip keeps
    // personal mode (the default) free of an extra request per load.
    if (hasSessionToken() || readIdentityHint() !== null) runConfirmation();
    return () => {
      // Abandon any in-flight confirmation for this mount.
      confirmSeq.current += 1;
      window.removeEventListener(AUTH_CHANGED_EVENT, onAuthChanged);
    };
  }, [applyScope, runConfirmation]);

  /** Fail-closed recovery: drop the unconfirmed session, keep every bucket. */
  const signInAgain = useCallback(() => {
    forgetSessionIdentity();
    window.location.assign(apiUrl('/login'));
  }, []);

  // Stable setter identities: consumers can list them in effect deps
  // without retriggering on unrelated context changes.
  const setPipelineToml = useCallback(
    (toml: string) => dispatch({ type: 'SET_PIPELINE_TOML', payload: toml }),
    [],
  );
  const setDagData = useCallback(
    (dag: DagJson | null) => dispatch({ type: 'SET_DAG_DATA', payload: dag }),
    [],
  );
  const setActiveRunId = useCallback(
    (id: string | null) => dispatch({ type: 'SET_ACTIVE_RUN_ID', payload: id }),
    [],
  );
  const setRunResult = useCallback(
    (result: RunResult | null) => dispatch({ type: 'SET_RUN_RESULT', payload: result }),
    [],
  );
  const setChatContext = useCallback(
    (c: ChatContextType) => dispatch({ type: 'SET_CHAT_CONTEXT', payload: c }),
    [],
  );
  const setChatMessages = useCallback(
    (ctx: ChatContextType, msgs: ChatMessage[]) =>
      dispatch({ type: 'SET_CHAT_MESSAGES', payload: { context: ctx, messages: msgs } }),
    [],
  );
  const clearSession = useCallback(() => {
    try {
      localStorage.removeItem(sessionKeyForScope(scopeRef.current));
    } catch {
      // Storage unavailable — the in-memory reset below still applies.
    }
    dispatch({ type: 'CLEAR_SESSION' });
  }, []);

  const ctx: SessionContextValue = useMemo(
    () => ({
      state,
      dispatch,
      setPipelineToml,
      setDagData,
      setActiveRunId,
      setRunResult,
      setChatContext,
      setChatMessages,
      clearSession,
    }),
    [state, dispatch, setPipelineToml, setDagData, setActiveRunId, setRunResult, setChatContext, setChatMessages, clearSession],
  );

  // #1144 D3: hold the first paint while a claimed identity is being
  // confirmed, so the hinted bucket's content is never rendered, not even
  // for one `/api/auth/me` roundtrip. Guests skip the gate entirely.
  if (status === 'confirming') return null;
  // Fail closed: the server could not name the signed-in user, so NO bucket
  // was restored. Show an explicit notice with no drafts rather than the
  // cached identity's — that is exactly what a forged hint would name.
  if (status === 'unconfirmed') {
    return (
      <div
        role="alert"
        data-testid="identity-unconfirmed"
        style={{
          maxWidth: 520,
          margin: '18vh auto',
          padding: '1.5rem',
          fontFamily: 'system-ui, sans-serif',
          textAlign: 'center',
        }}
      >
        <h1 style={{ fontSize: '1.25rem', marginBottom: '0.75rem' }}>Session not confirmed</h1>
        <p style={{ marginBottom: '0.5rem' }}>
          We could not confirm who is signed in, so no drafts are shown.
        </p>
        <p style={{ marginBottom: '1rem', opacity: 0.8 }}>
          Your saved work is kept — it returns as soon as the server confirms your account.
        </p>
        <div style={{ display: 'flex', gap: '0.5rem', justifyContent: 'center' }}>
          <button type="button" onClick={retryConfirmation}>Retry</button>
          <button type="button" onClick={signInAgain}>Sign in again</button>
        </div>
      </div>
    );
  }

  return <SessionCtx.Provider value={ctx}>{children}</SessionCtx.Provider>;
}

// eslint-disable-next-line react-refresh/only-export-components
export function usePipelineSession(): SessionContextValue {
  const ctx = useContext(SessionCtx);
  if (!ctx) throw new Error('usePipelineSession must be used within PipelineSessionProvider');
  return ctx;
}
