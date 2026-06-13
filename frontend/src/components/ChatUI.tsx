import { useState, useRef, useEffect } from 'react';
import { Send, Bot, User, Loader2, Check } from 'lucide-react';

interface ChatMessage {
  id: string;
  role: 'user' | 'assistant' | 'agent' | 'system';
  content: string;
  actions?: ChatAction[];
  agentStatus?: string;
}

interface ChatAction {
  type: string;
  label: string;
  action: string;
  data?: any;
}

interface ChatUIProps {
  onPipelineReady?: (data: any) => void;
  onDataReport?: (report: any) => void;
}

export default function ChatUI({ onPipelineReady }: ChatUIProps) {
  const [messages, setMessages] = useState<ChatMessage[]>([]);
  const [input, setInput] = useState('');
  const [loading, setLoading] = useState(false);
  const [agents, setAgents] = useState<Record<string, string>>({});
  const chatRef = useRef<HTMLDivElement>(null);
  const inputRef = useRef<HTMLInputElement>(null);

  useEffect(() => { chatRef.current?.scrollTo(0, chatRef.current.scrollHeight); }, [messages, agents]);

  const sendMessage = async () => {
    const text = input.trim();
    if (!text || loading) return;

    const userMsg: ChatMessage = { id: Date.now().toString(), role: 'user', content: text };
    setMessages(prev => [...prev, userMsg]);
    setInput('');
    setLoading(true);

    // Add assistant placeholder
    const assistantId = (Date.now() + 1).toString();
    setMessages(prev => [...prev, { id: assistantId, role: 'assistant', content: '', agentStatus: 'Thinking...' }]);

    try {
      const resp = await fetch('/api/chat/send/json', {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({ message: text }),
      });
      const data = await resp.json();

      if (data.pipeline_id) {
        // Update assistant message with result
        const tomlPreview = (data.toml_content as string || '').split('\n').slice(0, 6).join('\n');
        setMessages(prev => prev.map(m =>
          m.id === assistantId ? {
            ...m,
            content: `✅ Pipeline generated!\n\n\`\`\`toml\n${tomlPreview}\n...\n\`\`\``,
            agentStatus: undefined,
            actions: [
              { type: 'primary', label: '✅ Accept', action: 'accept', data },
              { type: 'secondary', label: '✏️ Edit', action: 'edit', data },
              { type: 'ghost', label: '🔄 Regenerate', action: 'regenerate' },
            ],
          } : m
        ));
        onPipelineReady?.(data);
        setAgents({});
      } else if (data.code) {
        setMessages(prev => prev.map(m =>
          m.id === assistantId ? { ...m, content: `❌ ${data.message || 'Error generating pipeline'}`, agentStatus: undefined } : m
        ));
      }
    } catch (e) {
      setMessages(prev => prev.map(m =>
        m.id === assistantId ? { ...m, content: '❌ Connection error. Please try again.', agentStatus: undefined } : m
      ));
    }
    setLoading(false);
  };

  const handleAction = (action: ChatAction) => {
    if (action.action === 'accept' && action.data) {
      onPipelineReady?.(action.data);
      setMessages(prev => [...prev, { id: Date.now().toString(), role: 'system', content: 'Pipeline saved and ready to run.' }]);
    } else if (action.action === 'regenerate') {
      sendMessage();
    } else if (action.action === 'edit' && action.data) {
      onPipelineReady?.(action.data);
      window.location.href = '/editor';
    }
  };

  return (
    <div className="chat-container" style={{ display: 'flex', flexDirection: 'column', height: '100%', background: 'var(--color-bg)', borderRadius: 'var(--radius-md)', border: '1px solid var(--color-border)' }}>
      {/* Header */}
      <div style={{ padding: '12px 16px', borderBottom: '1px solid var(--color-border)', display: 'flex', alignItems: 'center', gap: '8px' }}>
        <Bot size={18} color="var(--color-primary)" />
        <span style={{ fontWeight: 600, fontSize: '0.9rem' }}>AI Companion</span>
        <span style={{ fontSize: '0.7rem', color: 'var(--color-text-tertiary)', marginLeft: 'auto' }}>v0.9</span>
      </div>

      {/* Messages */}
      <div ref={chatRef} style={{ flex: 1, overflow: 'auto', padding: '12px 16px', display: 'flex', flexDirection: 'column', gap: '12px' }}>
        {messages.length === 0 && (
          <div style={{ textAlign: 'center', color: 'var(--color-text-tertiary)', padding: '2rem 1rem' }}>
            <Bot size={32} style={{ marginBottom: '8px', opacity: 0.5 }} />
            <p style={{ fontSize: '0.9rem', marginBottom: '4px' }}>Describe your analysis and I'll generate a pipeline.</p>
            <p style={{ fontSize: '0.75rem' }}>Try: "RNA-seq paired-end, hg38, STAR + featureCounts"</p>
          </div>
        )}

        {messages.map(msg => (
          <div key={msg.id} style={{ display: 'flex', gap: '8px', alignItems: 'flex-start' }}>
            <div style={{ width: 28, height: 28, borderRadius: '50%', display: 'flex', alignItems: 'center', justifyContent: 'center', flexShrink: 0, background: msg.role === 'user' ? 'var(--color-primary-light)' : 'var(--color-bg-tertiary)', color: msg.role === 'user' ? 'var(--color-primary)' : 'var(--color-text-secondary)' }}>
              {msg.role === 'user' ? <User size={14} /> : msg.role === 'system' ? <Check size={14} /> : <Bot size={14} />}
            </div>
            <div style={{ flex: 1 }}>
              <div style={{ fontSize: '0.8rem', fontWeight: 600, color: 'var(--color-text-secondary)', marginBottom: '2px' }}>
                {msg.role === 'user' ? 'You' : msg.role === 'system' ? 'System' : 'AI'}
              </div>
              {msg.agentStatus && (
                <div style={{ display: 'flex', alignItems: 'center', gap: '6px', padding: '6px 0', color: 'var(--color-text-secondary)', fontSize: '0.8rem' }}>
                  <Loader2 size={12} className="spin" /> {msg.agentStatus}
                </div>
              )}
              {msg.content && (
                <div style={{ fontSize: '0.85rem', lineHeight: 1.6, whiteSpace: 'pre-wrap', wordBreak: 'break-word' }}>{msg.content}</div>
              )}
              {/* Action buttons */}
              {msg.actions && msg.actions.length > 0 && (
                <div style={{ display: 'flex', gap: '6px', marginTop: '8px', flexWrap: 'wrap' }}>
                  {msg.actions.map((act, i) => (
                    <button
                      key={i}
                      onClick={() => handleAction(act)}
                      className={act.type === 'primary' ? 'btn-run' : act.type === 'secondary' ? 'btn-sm' : 'btn-sm'}
                      style={act.type === 'primary' ? {} : { background: 'transparent', border: '1px solid var(--color-border)' }}
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
          <div style={{ background: 'var(--color-bg-secondary)', borderRadius: 'var(--radius-sm)', padding: '8px 12px', fontSize: '0.75rem' }}>
            {Object.entries(agents).map(([agent, status]) => (
              <div key={agent} style={{ display: 'flex', alignItems: 'center', gap: '6px', padding: '2px 0' }}>
                <div style={{ width: 6, height: 6, borderRadius: '50%', background: status === 'done' ? 'var(--color-success)' : 'var(--color-primary)', animation: status !== 'done' ? 'pulse 1.5s infinite' : 'none' }} />
                <span style={{ fontWeight: 500 }}>{agent}</span>
                <span style={{ color: 'var(--color-text-tertiary)' }}>{status}</span>
              </div>
            ))}
          </div>
        )}
      </div>

      {/* Input */}
      <div style={{ padding: '12px 16px', borderTop: '1px solid var(--color-border)', display: 'flex', gap: '8px' }}>
        <input
          ref={inputRef}
          type="text"
          value={input}
          onChange={e => setInput(e.target.value)}
          onKeyDown={e => e.key === 'Enter' && sendMessage()}
          placeholder="Describe your analysis..."
          disabled={loading}
          className="intent-input"
          style={{ flex: 1 }}
        />
        <button onClick={sendMessage} disabled={loading || !input.trim()} className="btn-run" style={{ width: 40, height: 40, padding: 0, display: 'flex', alignItems: 'center', justifyContent: 'center' }}>
          {loading ? <Loader2 size={16} className="spin" /> : <Send size={16} />}
        </button>
      </div>
    </div>
  );
}
