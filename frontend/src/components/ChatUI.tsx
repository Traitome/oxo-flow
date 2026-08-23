import { useState, useRef, useEffect } from 'react';
import { Send, Bot, User, Loader2, Check, Wrench } from 'lucide-react';
import { Link, useNavigate } from 'react-router-dom';
import { usePipelineSession, type ChatContextType, type ChatMessage, type ChatAction } from '../context/PipelineSession';
import { useServerVersion } from '../api/version';
import { api, ApiError } from '../api/client';
import { useI18n } from '../context/I18n';

const CONTEXT_LABELS: Record<ChatContextType, string> = {
  dashboard: 'Pipeline Generation',
  editor: 'Pipeline Refinement',
  monitor: 'Run Diagnosis',
  report: 'Results Interpretation',
};

const PLACEHOLDERS: Record<ChatContextType, string> = {
  dashboard: 'Describe your analysis and I\'ll generate a pipeline. Try: "RNA-seq paired-end, hg38, STAR + featureCounts"',
  editor: 'Ask me to refine this pipeline — add rules, change parameters, or fix validation issues.',
  monitor: 'Ask me about the running pipeline — status, errors, or predictions.',
  report: 'Ask me about the results — findings, comparisons, or next steps.',
};

interface ChatUIProps {
  context?: ChatContextType;
  onPipelineReady?: (data: { toml_content?: string; validation?: unknown; pipeline_id?: string }) => void;
  onDataReport?: (report: Record<string, unknown>) => void;
}

export default function ChatUI({ context = 'dashboard', onPipelineReady }: ChatUIProps) {
  const session = usePipelineSession();
  const version = useServerVersion();
  const navigate = useNavigate();
  const { t } = useI18n();
  const [messages, setMessages] = useState<ChatMessage[]>(() => session.state.chatMessages[context] || []);
  const [input, setInput] = useState('');
  const [loading, setLoading] = useState(false);
  const [agents, setAgents] = useState<Record<string, string>>({});
  const [aiConfigured, setAiConfigured] = useState(true);
  const chatRef = useRef<HTMLDivElement>(null);
  const inputRef = useRef<HTMLTextAreaElement>(null);

  // Sync messages to session context whenever they change
  useEffect(() => {
    session.setChatMessages(context, messages);
  }, [messages, context]);

  // Set chat context on mount
  useEffect(() => {
    session.setChatContext(context);
  }, [context]);

  // Detect whether the server has a working AI provider so we can surface a
  // friendly fallback instead of a raw env-var error.
  useEffect(() => {
    api.aiConfig().then((c) => setAiConfigured(c.is_configured)).catch(() => setAiConfigured(false));
  }, []);

  useEffect(() => { chatRef.current?.scrollTo(0, chatRef.current.scrollHeight); }, [messages, agents]);

  const sendMessage = async () => {
    const text = input.trim();
    if (!text || loading) return;

    const userMsg: ChatMessage = { id: crypto.randomUUID(), role: 'user', content: text };
    setMessages(prev => [...prev, userMsg]);
    setInput('');
    setLoading(true);

    // Add assistant placeholder
    const assistantId = crypto.randomUUID();
    setMessages(prev => [...prev, { id: assistantId, role: 'assistant', content: '', agentStatus: 'Thinking...' }]);

    try {
      // Goes through the API client so the base-path prefix and the
      // Authorization header apply (team/HPC mode, --base-path deploys).
      const resp = await api.chatSendStream(text, { intent: context });
      if (!resp.body) throw new Error("No response body");
      const reader = resp.body.getReader();
      const decoder = new TextDecoder();
      let buffer = '';
      let doneReading = false;
      let finalPipelineData: { toml_content?: string; validation?: unknown; pipeline_id?: string } | null = null;

      while (!doneReading) {
        const { value, done } = await reader.read();
        if (done) break;
        buffer += decoder.decode(value, { stream: true });
        let nextIndex;
        while ((nextIndex = buffer.indexOf('\n\n')) !== -1) {
          const eventString = buffer.substring(0, nextIndex);
          buffer = buffer.substring(nextIndex + 2);
          
          const lines = eventString.split('\n');
          let currentEvent = '';
          let currentData = '';
          for (const line of lines) {
            if (line.startsWith('event:')) currentEvent = line.substring(6).trim();
            else if (line.startsWith('data:')) currentData = line.substring(5).trim();
          }
          
          if (currentEvent && currentData) {
            const payload = JSON.parse(currentData);
            if (currentEvent === 'status') {
              setMessages(prev => prev.map(m => m.id === assistantId ? { ...m, agentStatus: payload.message } : m));
            } else if (currentEvent === 'tool_call') {
              setMessages(prev => prev.map(m => m.id === assistantId ? {
                ...m,
                toolCalls: [...(m.toolCalls ?? []), { id: `${payload.name}-${crypto.randomUUID()}`, name: payload.name, args: typeof payload.args === 'string' ? payload.args : JSON.stringify(payload.args) }],
              } : m));
            } else if (currentEvent === 'tool_result') {
              setMessages(prev => prev.map(m => m.id === assistantId ? {
                ...m,
                toolCalls: (m.toolCalls ?? []).map((tc, i, arr) =>
                  i === arr.length - 1 && tc.name === payload.name ? { ...tc, summary: payload.summary } : tc
                ),
              } : m));
            } else if (currentEvent === 'text') {
              setMessages(prev => prev.map(m => m.id === assistantId ? { ...m, content: m.content + payload.chunk } : m));
            } else if (currentEvent === 'action') {
              if (payload.action_type === 'pipeline_ready') {
                finalPipelineData = payload.data;
              }
            } else if (currentEvent === 'done') {
              doneReading = true;
            } else if (currentEvent === 'error') {
              throw new Error(payload.message || JSON.stringify(payload));
            }
          }
        }
      }

      if (finalPipelineData) {
        const tomlPreview = (finalPipelineData.toml_content as string || '').split('\n').slice(0, 6).join('\n');
        setMessages(prev => prev.map(m =>
          m.id === assistantId ? {
            ...m,
            content: m.content + `\n\n✅ Pipeline generated!\n\n\`\`\`toml\n${tomlPreview}\n...\n\`\`\``,
            agentStatus: undefined,
            actions: [
              { type: 'primary', label: '✅ Accept', action: 'accept', data: finalPipelineData },
              { type: 'secondary', label: '✏️ Edit', action: 'edit', data: finalPipelineData },
              { type: 'ghost', label: '🔄 Regenerate', action: 'regenerate' },
            ],
          } : m
        ));
        onPipelineReady?.(finalPipelineData);
      } else {
        setMessages(prev => prev.map(m => m.id === assistantId ? { ...m, agentStatus: undefined } : m));
      }
      setAgents({});
    } catch (e: unknown) {
      if (e instanceof ApiError && e.code === 'AI_NOT_CONFIGURED') {
        setAiConfigured(false);
      }
      const errMsg = e instanceof Error ? e.message : 'Connection error.';
      setMessages(prev => prev.map(m =>
        m.id === assistantId ? { ...m, content: m.content + `\n❌ ${errMsg}`, agentStatus: undefined } : m
      ));
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
        setMessages(prev => [...prev, { id: crypto.randomUUID(), role: 'system', content: '❌ The generated pipeline did not pass validation — use ✏️ Edit to review it in the editor.' }]);
        return;
      }
      try {
        const toml = data.toml_content ?? '';
        const name = toml.match(/name\s*=\s*"([^"]+)"/)?.[1] || 'ai-generated-pipeline';
        await api.createPipeline({ name, toml_content: toml });
        session.setPipelineToml(toml);
        onPipelineReady?.(action.data);
        setMessages(prev => [...prev, { id: crypto.randomUUID(), role: 'system', content: `✅ Pipeline "${name}" saved and opened in the editor.` }]);
        navigate('/editor');
      } catch (err: unknown) {
        setMessages(prev => [...prev, { id: crypto.randomUUID(), role: 'system', content: `❌ Save failed: ${err instanceof Error ? err.message : 'unknown error'}` }]);
      }
    } else if (action.action === 'regenerate') {
      sendMessage();
    } else if (action.action === 'edit' && action.data) {
      onPipelineReady?.(action.data);
    }
  };

  return (
    <div className="chat-container">
      {/* Header */}
      <div className="chat-header">
        <Bot size={18} color="var(--color-primary)" />
        <span className="chat-title">AI Companion</span>
        <span className="chat-context-tag">{CONTEXT_LABELS[context]}</span>
        <span className="chat-version">{version ? `v${version}` : ''}</span>
      </div>

      {/* Messages */}
      <div ref={chatRef} aria-live="polite" aria-label="Chat messages" className="chat-messages">
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
              <Link to="/editor" className="btn-sm" style={{ textDecoration: 'none' }}>
                {t('chat.disabled.editor')}
              </Link>
            </div>
          </div>
        )}
        {messages.length === 0 && aiConfigured && (
          <div className="chat-empty">
            <Bot size={32} style={{ opacity: 0.5 }} />
            <p style={{ fontSize: '0.9rem' }}>{PLACEHOLDERS[context]}</p>
          </div>
        )}

        {messages.map(msg => (
          <div key={msg.id} className="chat-msg">
            <div className={`chat-avatar${msg.role === 'user' ? ' user' : ''}`}>
              {msg.role === 'user' ? <User size={14} /> : msg.role === 'system' ? <Check size={14} /> : <Bot size={14} />}
            </div>
            <div className="chat-msg-body">
              <div className="chat-msg-author">
                {msg.role === 'user' ? 'You' : msg.role === 'system' ? 'System' : 'AI'}
              </div>
              {msg.agentStatus && (
                <div className="chat-status">
                  <Loader2 size={12} className="spin" /> {msg.agentStatus}
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

        {/* Agent status bar */}
        {Object.keys(agents).length > 0 && (
          <div className="chat-agents">
            {Object.entries(agents).map(([agent, status]) => (
              <div key={agent} className="chat-agent-row">
                <div className="chat-agent-dot" style={{ background: status === 'done' ? 'var(--color-success)' : 'var(--color-primary)', animation: status !== 'done' ? 'pulse 1.5s infinite' : 'none' }} />
                <span style={{ fontWeight: 500 }}>{agent}</span>
                <span style={{ color: 'var(--color-text-tertiary)' }}>{status}</span>
              </div>
            ))}
          </div>
        )}
      </div>

      {/* Input */}
      <div className="chat-input-row">
        <textarea
          ref={inputRef}
          value={input}
          onChange={e => setInput(e.target.value)}
          onKeyDown={e => { if (e.key === 'Enter' && !e.shiftKey) { e.preventDefault(); sendMessage(); }}}
          placeholder={aiConfigured ? 'Describe your analysis... (Shift+Enter for newline)' : t('chat.disabled.message')}
          disabled={loading || !aiConfigured}
          rows={2}
          className="search-input intent-input"
          style={{ flex: 1, minWidth: 0 }}
        />
        <button onClick={sendMessage} disabled={loading || !input.trim()} className="btn-run chat-send" aria-label="Send message">
          {loading ? <Loader2 size={16} className="spin" /> : <Send size={16} />}
        </button>
      </div>
    </div>
  );
}
