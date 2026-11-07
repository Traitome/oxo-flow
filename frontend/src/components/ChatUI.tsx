import { useState, useRef, useEffect } from 'react';
import { Send, Square, Bot, User, Loader2, Check, Wrench } from 'lucide-react';
import { Link, useNavigate } from 'react-router-dom';
import { usePipelineSession, type ChatContextType, type ChatMessage, type ChatAction, type ChatProgress } from '../context/PipelineSession';
import { useServerVersion } from '../api/version';
import { api, ApiError } from '../api/client';
import { useI18n } from '../context/I18n';

/** Fallback inactivity budget before the server's `ready` event arrives.
 * The server advertises the real value (3× its keep-alive cadence); if the
 * connection dies silently, this is what turns an endless "planning" spinner
 * into a visible error with a retry (issue #1040 B1). */
const DEFAULT_IDLE_TIMEOUT_MS = 45_000;
const MIN_IDLE_TIMEOUT_MS = 10_000;
const MAX_IDLE_TIMEOUT_MS = 300_000;

class ChatStreamIdleError extends Error {
  constructor(seconds: number) {
    super(`No data from the server for ${seconds}s — the connection appears to be dead.`);
    this.name = 'ChatStreamIdleError';
  }
}

/**
 * Read one SSE chunk, rejecting when the stream stays silent past
 * `timeoutMs`. Keep-alive comment frames count as traffic (they arrive as
 * bytes), so a slow-but-healthy generation is never cut off.
 */
function readWithIdleTimeout(
  reader: ReadableStreamDefaultReader<Uint8Array>,
  timeoutMs: number,
): Promise<ReadableStreamReadResult<Uint8Array>> {
  let timer: number | undefined;
  const timeout = new Promise<never>((_, reject) => {
    timer = window.setTimeout(
      () => reject(new ChatStreamIdleError(Math.round(timeoutMs / 1000))),
      timeoutMs,
    );
  });
  return Promise.race([reader.read(), timeout]).finally(() => {
    if (timer !== undefined) window.clearTimeout(timer);
  });
}

function formatElapsed(ms: number): string {
  const totalSecs = Math.max(0, Math.round(ms / 1000));
  if (totalSecs < 60) return `${totalSecs}s`;
  return `${Math.floor(totalSecs / 60)}m ${totalSecs % 60}s`;
}

function useContextLabels(t: (key: string) => string): Record<ChatContextType, string> {
  return {
    dashboard: t('chat.context.dashboard'),
    editor: t('chat.context.editor'),
    monitor: t('chat.context.monitor'),
    report: t('chat.context.report'),
  };
}

function usePlaceholders(t: (key: string) => string): Record<ChatContextType, string> {
  return {
    dashboard: t('chat.placeholder.dashboard'),
    editor: t('chat.placeholder.editor'),
    monitor: t('chat.placeholder.monitor'),
    report: t('chat.placeholder.report'),
  };
}

interface ChatUIProps {
  context?: ChatContextType;
  onPipelineReady?: (data: { toml_content?: string; validation?: unknown; pipeline_id?: string }) => void;
}

export default function ChatUI({ context = 'dashboard', onPipelineReady }: ChatUIProps) {
  const session = usePipelineSession();
  const version = useServerVersion();
  const navigate = useNavigate();
  const { t } = useI18n();
  const contextLabels = useContextLabels(t);
  const placeholders = usePlaceholders(t);
  const [messages, setMessages] = useState<ChatMessage[]>(() => session.state.chatMessages[context] || []);
  const [input, setInput] = useState('');
  const [loading, setLoading] = useState(false);
  const [aiConfigured, setAiConfigured] = useState(true);
  const chatRef = useRef<HTMLDivElement>(null);
  const inputRef = useRef<HTMLTextAreaElement>(null);

  // AbortController for the in-flight chat stream (issue #214): without it a
  // navigation away mid-stream leaked the fetch connection and kept the
  // backend burning provider tokens for UI nobody was reading.
  const abortRef = useRef<AbortController | null>(null);
  useEffect(() => () => abortRef.current?.abort(), []);

  // Sync messages to session context whenever they change. The setter
  // identities are stable (useCallback in PipelineSession), so depending on
  // them — instead of the whole `session` object — keeps the effect from
  // re-running on every unrelated state update (that re-ran the dispatch and
  // looped: the reducer always returns a new state object).
  const { setChatMessages, setChatContext } = session;
  useEffect(() => {
    setChatMessages(context, messages);
  }, [messages, context, setChatMessages]);

  // Set chat context on mount
  useEffect(() => {
    setChatContext(context);
  }, [context, setChatContext]);

  // Detect whether the server has a working AI provider so we can surface a
  // friendly fallback instead of a raw env-var error.
  useEffect(() => {
    api.aiConfig().then((c) => setAiConfigured(c.is_configured)).catch(() => setAiConfigured(false));
  }, []);

  // #550: crypto.randomUUID exists only in secure contexts — plain-HTTP
  // LAN/HPC deployments threw on Send and the chat silently broke.
  const uuid = (): string => {
    if (typeof crypto !== 'undefined' && typeof crypto.randomUUID === 'function') {
      return crypto.randomUUID();
    }
    const bytes = new Uint8Array(16);
    crypto.getRandomValues(bytes);
    return Array.from(bytes, (b) => b.toString(16).padStart(2, '0')).join('');
  };

  useEffect(() => { chatRef.current?.scrollTo(0, chatRef.current.scrollHeight); }, [messages]);

  const lastPrompt = useRef('');

  // Round-progress label: the server sends structured frames every phase
  // (plan/generate/tool/validate) so a long generation shows motion instead
  // of one static word (issue #1040 B2).
  const progressLabel = (p: ChatProgress): string => {
    const round = t('chat.progress.round')
      .replace('{{round}}', String(p.round))
      .replace('{{max}}', String(p.max_rounds));
    switch (p.step) {
      case 'plan':
        return t('chat.progress.plan');
      case 'generate':
        return p.round > 0 ? `${round} · ${t('chat.progress.generate')}` : t('chat.progress.generate');
      case 'tool':
        return p.round > 0
          ? `${round} · ${t('chat.progress.tool').replace('{{tool}}', p.tool ?? '…')}`
          : t('chat.progress.tool').replace('{{tool}}', p.tool ?? '…');
      case 'validate':
        return p.round > 0 ? `${round} · ${t('chat.progress.validate')}` : t('chat.progress.validate');
      default:
        return round;
    }
  };

  const sendMessage = async (override?: string) => {
    const text = (override ?? input).trim();
    if (!text || loading) return;
    lastPrompt.current = text;

    const userMsg: ChatMessage = { id: uuid(), role: 'user', content: text };
    setMessages(prev => [...prev, userMsg]);
    setInput('');
    setLoading(true);

    // Add assistant placeholder
    const assistantId = uuid();
    setMessages(prev => [...prev, { id: assistantId, role: 'assistant', content: '', agentStatus: t('chat.thinking') }]);

    // Abort any stream still running (issue #214): navigation away and the
    // stop button go through this controller too.
    abortRef.current?.abort();
    const ac = new AbortController();
    abortRef.current = ac;

    try {
      // Goes through the API client so the base-path prefix and the
      // Authorization header apply (team/HPC mode, --base-path deploys).
      const resp = await api.chatSendStream(text, { intent: context }, { signal: ac.signal });
      if (!resp.body) throw new Error("No response body");
      const reader = resp.body.getReader();
      const decoder = new TextDecoder();
      let buffer = '';
      let doneReading = false;
      let idleTimeoutMs = DEFAULT_IDLE_TIMEOUT_MS;
      let finalPipelineData: { toml_content?: string; validation?: unknown; pipeline_id?: string } | null = null;
      // Server-side intent routing (issue #1040 P2-4): `ready` announces the
      // decision up front so a chat turn never grows pipeline cards.
      let streamMode: 'chat' | 'pipeline' | null = null;

      while (!doneReading) {
        // Bound every wait: a silently dropped connection (proxy, network
        // change) never fires an error, so `reader.read()` would hang
        // forever and leave the UI "planning" indefinitely.
        const { value, done } = await readWithIdleTimeout(reader, idleTimeoutMs);
        if (done) break;
        buffer += decoder.decode(value, { stream: true });
        let nextIndex;
        while ((nextIndex = buffer.indexOf('\n\n')) !== -1) {
          const eventString = buffer.substring(0, nextIndex);
          buffer = buffer.substring(nextIndex + 2);
          
          const lines = eventString.split('\n');
          let currentEvent = '';
          // #550: SSE data fields may span multiple `data:` lines — they
          // join with \n per the SSE spec. Last-line-wins truncated any
          // multi-line payload.
          const dataLines: string[] = [];
          for (const line of lines) {
            if (line.startsWith('event:')) currentEvent = line.substring(6).trim();
            else if (line.startsWith('data:')) dataLines.push(line.substring(5).replace(/^ /, ''));
          }
          const currentData = dataLines.join('\n');
          
          if (currentEvent && currentData) {
            // #550: one bad frame (or an interleaved keepalive) must skip
            // that frame, not abort the whole stream into the ❌ path.
            let payload: Record<string, unknown>;
            try {
              payload = JSON.parse(currentData);
            } catch {
              console.warn('chat: skipping unparseable SSE frame', currentEvent, currentData.slice(0, 120));
              continue;
            }
            if (currentEvent === 'ready') {
              // Server-declared stream cadence: derive the client watchdog
              // from server guidance instead of hard-coding one.
              const secs = Number(payload.idle_timeout_secs);
              if (Number.isFinite(secs) && secs > 0) {
                idleTimeoutMs = Math.min(Math.max(secs * 1000, MIN_IDLE_TIMEOUT_MS), MAX_IDLE_TIMEOUT_MS);
              }
              if (payload.mode === 'chat' || payload.mode === 'pipeline') {
                streamMode = payload.mode;
                setMessages(prev => prev.map(m => m.id === assistantId ? { ...m, mode: payload.mode as 'chat' | 'pipeline' } : m));
              }
            } else if (currentEvent === 'progress') {
              const progress: ChatProgress = {
                round: Number(payload.round ?? 0),
                max_rounds: Number(payload.max_rounds ?? 0),
                step: String(payload.step ?? ''),
                tool: typeof payload.tool === 'string' ? payload.tool : null,
                elapsed_ms: Number(payload.elapsed_ms ?? 0),
                prompt_tokens: Number(payload.prompt_tokens ?? 0),
                completion_tokens: Number(payload.completion_tokens ?? 0),
              };
              setMessages(prev => prev.map(m => m.id === assistantId ? { ...m, progress } : m));
            } else if (currentEvent === 'status') {
              setMessages(prev => prev.map(m => m.id === assistantId ? { ...m, agentStatus: String(payload.message ?? '') } : m));
            } else if (currentEvent === 'tool_call') {
              setMessages(prev => prev.map(m => m.id === assistantId ? {
                ...m,
                toolCalls: [...(m.toolCalls ?? []), { id: `${String(payload.name)}-${uuid()}`, name: String(payload.name), args: typeof payload.args === 'string' ? payload.args : JSON.stringify(payload.args) }],
              } : m));
            } else if (currentEvent === 'tool_result') {
              setMessages(prev => prev.map(m => m.id === assistantId ? {
                ...m,
                toolCalls: (m.toolCalls ?? []).map((tc, i, arr) =>
                  i === arr.length - 1 && tc.name === String(payload.name) ? { ...tc, summary: String(payload.summary ?? '') } : tc
                ),
              } : m));
            } else if (currentEvent === 'text') {
              setMessages(prev => prev.map(m => m.id === assistantId ? { ...m, content: m.content + String(payload.chunk ?? '') } : m));
            } else if (currentEvent === 'action') {
              // A chat-routed turn must never grow pipeline cards, even if
              // a stray `action` frame arrives.
              if (payload.action_type === 'pipeline_ready' && streamMode !== 'chat') {
                finalPipelineData = payload.data as { toml_content?: string; validation?: unknown; pipeline_id?: string } | null;
              }
            } else if (currentEvent === 'done') {
              if (payload.mode === 'chat' || payload.mode === 'pipeline') {
                streamMode = payload.mode;
                setMessages(prev => prev.map(m => m.id === assistantId ? { ...m, mode: payload.mode as 'chat' | 'pipeline' } : m));
              }
              // The terminal payload repeats the conversational reply so a
              // lost `text` frame cannot leave the bubble empty.
              if (streamMode === 'chat' && typeof payload.message === 'string') {
                setMessages(prev => prev.map(m => m.id === assistantId ? { ...m, content: m.content || String(payload.message) } : m));
              }
              doneReading = true;
            } else if (currentEvent === 'error') {
              throw new Error(String(payload.message ?? JSON.stringify(payload)));
            }
          }
        }
      }

      if (finalPipelineData) {
        const tomlPreview = (finalPipelineData.toml_content as string || '').split('\n').slice(0, 6).join('\n');
        setMessages(prev => prev.map(m =>
          m.id === assistantId ? {
            ...m,
            content: m.content + `\n\n✅ ${t('chat.generated')}\n\n\`\`\`toml\n${tomlPreview}\n...\n\`\`\``,
            agentStatus: undefined,
            progress: undefined,
            actions: [
              { type: 'primary', label: `✅ ${t('chat.accept')}`, action: 'accept', data: finalPipelineData },
              { type: 'secondary', label: `✏️ ${t('chat.edit')}`, action: 'edit', data: finalPipelineData },
              { type: 'ghost', label: `🔄 ${t('chat.regenerate')}`, action: 'regenerate' },
            ],
          } : m
        ));
        onPipelineReady?.(finalPipelineData);
      } else {
        setMessages(prev => prev.map(m => m.id === assistantId ? { ...m, agentStatus: undefined, progress: undefined } : m));
      }
    } catch (e: unknown) {
      // Aborted streams (stop button, navigation away, re-entry) are not
      // errors — leave whatever streamed content is already on screen.
      if (e instanceof DOMException && e.name === 'AbortError') {
        setMessages(prev => prev.map(m => m.id === assistantId ? { ...m, agentStatus: undefined, progress: undefined } : m));
      } else {
        if (e instanceof ApiError && e.code === 'AI_NOT_CONFIGURED') {
          setAiConfigured(false);
        }
        if (e instanceof ChatStreamIdleError) {
          // Release the dead connection and surface a retry entry instead
          // of leaving the send button disabled forever.
          ac.abort();
        }
        const errMsg = e instanceof Error ? e.message : 'Connection error.';
        setMessages(prev => prev.map(m =>
          m.id === assistantId ? {
            ...m,
            content: m.content + `\n❌ ${errMsg}`,
            agentStatus: undefined,
            progress: undefined,
            // The failure archive lives on the server; the browser keeps a
            // one-click retry so a dropped stream is recoverable.
            actions: [{ type: 'secondary', label: `🔄 ${t('chat.retry')}`, action: 'retry' }],
          } : m
        ));
      }
    } finally {
      // Only clear if we still own the slot — a newer request may have
      // replaced the controller (re-entry case).
      if (abortRef.current === ac) abortRef.current = null;
    }
    setLoading(false);
  };

  const handleAction = async (action: ChatAction) => {
    if (action.action === 'accept' && action.data) {
      // Issue #79 P1-10: Accept claimed "saved" without saving anything.
      // The pipeline is persisted for real, gated by the validation the
      // backend attached to the pipeline_ready payload.
      const data = action.data as { toml_content?: string; validation?: { valid?: boolean } | null };
      if (data.validation && data.validation.valid === false) {
        setMessages(prev => [...prev, { id: uuid(), role: 'system', content: `❌ ${t('chat.validationFailed')}` }]);
        return;
      }
      try {
        const toml = data.toml_content ?? '';
        const name = toml.match(/name\s*=\s*"([^"]+)"/)?.[1] || 'ai-generated-pipeline';
        await api.createPipeline({ name, toml_content: toml });
        session.setPipelineToml(toml);
        onPipelineReady?.(action.data);
        setMessages(prev => [...prev, { id: uuid(), role: 'system', content: `✅ ${t('chat.saved').replace('{{name}}', name)}` }]);
        navigate('/editor');
      } catch (err: unknown) {
        setMessages(prev => [...prev, { id: uuid(), role: 'system', content: `❌ ${t('chat.saveFailed').replace('{{error}}', err instanceof Error ? err.message : 'unknown error')}` }]);
      }
    } else if (action.action === 'regenerate' || action.action === 'retry') {
      // #550: input was cleared on the first send — the button read empty
      // input and did nothing. Re-send the retained last prompt.
      void sendMessage(lastPrompt.current || undefined);
    } else if (action.action === 'edit' && action.data) {
      onPipelineReady?.(action.data);
    }
  };

  return (
    <div className="chat-container">
      {/* Header */}
      <div className="chat-header">
        <Bot size={18} color="var(--color-primary)" />
        <span className="chat-title">{t('chat.title')}</span>
        <span className="chat-context-tag">{contextLabels[context]}</span>
        <span className="chat-version">{version ? `v${version}` : ''}</span>
      </div>

      {/* Messages */}
      <div ref={chatRef} aria-live="polite" aria-label={t('chat.messages')} className="chat-messages">
        {!aiConfigured && (
          <div className="chat-empty" style={{ textAlign: 'center' }}>
            <Bot size={32} style={{ opacity: 0.5 }} />
            <p style={{ fontSize: '0.9rem', maxWidth: 360, margin: '0 auto 1rem' }}>
              {t('chat.disabled.message')}
            </p>
            <div style={{ display: 'flex', gap: '0.5rem', justifyContent: 'center', flexWrap: 'wrap' }}>
              <Link to="/pipelines" className="btn-run" style={{ textDecoration: 'none' }}>
                {t('chat.disabled.templates')}
              </Link>
              {/* In the editor context "Open editor" links to the page the
                  user is already on (#907 P3-15) — show it everywhere else. */}
              {context !== 'editor' && (
                <Link to="/editor" className="btn-sm" style={{ textDecoration: 'none' }}>
                  {t('chat.disabled.editor')}
                </Link>
              )}
            </div>
          </div>
        )}
        {messages.length === 0 && aiConfigured && (
          <div className="chat-empty">
            <Bot size={32} style={{ opacity: 0.5 }} />
            <p style={{ fontSize: '0.9rem' }}>{placeholders[context]}</p>
            <div style={{ display: 'flex', gap: '6px', flexWrap: 'wrap', justifyContent: 'center', marginTop: 8 }}>
              {[
                'RNA-seq paired-end hg38: fastp + STAR + featureCounts + MultiQC',
                'WGS germline variant calling: bwa-mem2 + samtools + GATK4 HaplotypeCaller',
                'scRNA-seq CellRanger + Seurat QC pipeline',
              ].map((prompt) => (
                <button
                  key={prompt}
                  type="button"
                  className="pill-chip"
                  onClick={() => setInput(prompt)}
                >
                  {prompt}
                </button>
              ))}
            </div>
          </div>
        )}

        {messages.map(msg => (
          <div key={msg.id} className="chat-msg">
            <div className={`chat-avatar${msg.role === 'user' ? ' user' : ''}`}>
              {msg.role === 'user' ? <User size={14} /> : msg.role === 'system' ? <Check size={14} /> : <Bot size={14} />}
            </div>
            <div className="chat-msg-body">
              <div className="chat-msg-author">
                {msg.role === 'user' ? t('chat.user') : msg.role === 'system' ? t('chat.system') : t('chat.ai')}
              </div>
              {msg.agentStatus && (
                <div className="chat-status">
                  <Loader2 size={12} className="spin" /> {msg.agentStatus}
                </div>
              )}
              {msg.progress && (
                <div className="chat-progress" role="status">
                  <Loader2 size={12} className="spin" aria-hidden="true" />
                  <span>{progressLabel(msg.progress)}</span>
                  {msg.progress.elapsed_ms > 0 && (
                    <span className="chat-progress-meta">{formatElapsed(msg.progress.elapsed_ms)}</span>
                  )}
                  {(msg.progress.prompt_tokens > 0 || msg.progress.completion_tokens > 0) && (
                    <span className="chat-progress-meta">
                      {msg.progress.prompt_tokens.toLocaleString()} + {msg.progress.completion_tokens.toLocaleString()} tok
                    </span>
                  )}
                </div>
              )}
              {msg.toolCalls && msg.toolCalls.length > 0 && (
                <div className="chat-tool-cards">
                  {msg.toolCalls.map((tc) => (
                    <details className="chat-tool-card" key={tc.id}>
                      <summary>
                        <Wrench size={12} />
                        <span className="chat-tool-name">{tc.name}</span>
                        {tc.summary ? (
                          <span className="chat-tool-done">✓</span>
                        ) : (
                          <span className="chat-tool-pending"><Loader2 size={11} className="spin" /></span>
                        )}
                      </summary>
                      <div className="chat-tool-body">
                        <div className="chat-tool-args">{tc.args}</div>
                        {tc.summary && <div className="chat-tool-summary">{tc.summary}</div>}
                      </div>
                    </details>
                  ))}
                </div>
              )}
              {msg.content && (
                <div className="chat-content">{msg.content}</div>
              )}
              {/* Action buttons */}
              {msg.actions && msg.actions.length > 0 && (
                <div className="chat-actions">
                  {msg.actions.map((act, i) => (
                    <button
                      key={i}
                      onClick={() => handleAction(act)}
                      className={act.type === 'primary' ? 'btn-run' : 'btn-sm'}
                    >
                      {act.label}
                    </button>
                  ))}
                </div>
              )}
            </div>
          </div>
        ))}
      </div>

      {/* Input */}
      <div className="chat-input-row">
        <textarea
          ref={inputRef}
          value={input}
          onChange={e => setInput(e.target.value)}
          onKeyDown={e => { if (e.key === 'Enter' && !e.shiftKey) { e.preventDefault(); sendMessage(); }}}
          placeholder={aiConfigured ? t('chat.inputPlaceholder') : t('chat.disabled.message')}
          disabled={loading || !aiConfigured}
          rows={2}
          className="search-input intent-input"
          style={{ flex: 1, minWidth: 0 }}
        />
        <button
          onClick={() => void sendMessage()}
          disabled={loading || !input.trim()}
          className="btn-run chat-send"
          aria-label={t('chat.send')}
        >
          {loading ? <Loader2 size={16} className="spin" /> : <Send size={16} />}
        </button>
        {loading && (
          <button
            onClick={() => abortRef.current?.abort()}
            className="btn-run chat-send"
            aria-label={t('chat.stop')}
            title={t('chat.stop')}
          >
            <Square size={16} />
          </button>
        )}
      </div>
    </div>
  );
}
