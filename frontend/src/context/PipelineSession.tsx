import { createContext, useCallback, useContext, useEffect, useMemo, useReducer, type Dispatch, type ReactNode } from 'react';
import type { DagJson } from '../api/types';

// ── Types ──

export type ChatContextType = 'dashboard' | 'editor' | 'monitor' | 'report';

export interface ChatMessage {
  id: string;
  role: 'user' | 'assistant' | 'agent' | 'system';
  content: string;
  actions?: ChatAction[];
  agentStatus?: string;
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

// ── localStorage key ──

const STORAGE_KEY = 'oxo_session';

function loadStoredState(): Partial<PipelineSessionState> {
  try {
    const raw = localStorage.getItem(STORAGE_KEY);
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
    // Corrupt localStorage — clear it
    localStorage.removeItem(STORAGE_KEY);
  }
  return {};
}

function saveState(state: PipelineSessionState) {
  try {
    localStorage.setItem(STORAGE_KEY, JSON.stringify({
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
  chatMessages: { dashboard: [], editor: [], monitor: [], report: [] },
};

function createInitialState(): PipelineSessionState {
  const stored = loadStoredState();
  return {
    ...INITIAL,
    ...stored,
    chatMessages: stored.chatMessages || { dashboard: [], editor: [], monitor: [], report: [] },
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
      localStorage.removeItem(STORAGE_KEY);
      return { ...INITIAL, chatMessages: { dashboard: [], editor: [], monitor: [], report: [] } };
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

export function PipelineSessionProvider({ children }: { children: ReactNode }) {
  const [state, dispatch] = useReducer(reducer, null, createInitialState);

  // Persist key state to localStorage on every change
  useEffect(() => {
    saveState(state);
  }, [state.pipelineToml, state.dagData, state.activeRunId, state.chatMessages]);

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
  const clearSession = useCallback(() => dispatch({ type: 'CLEAR_SESSION' }), []);

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

  return <SessionCtx.Provider value={ctx}>{children}</SessionCtx.Provider>;
}

export function usePipelineSession(): SessionContextValue {
  const ctx = useContext(SessionCtx);
  if (!ctx) throw new Error('usePipelineSession must be used within PipelineSessionProvider');
  return ctx;
}
