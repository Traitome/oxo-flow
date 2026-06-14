import { useRef, useEffect } from 'react';
import { EditorView, basicSetup } from 'codemirror';
import { EditorState } from '@codemirror/state';
import { StreamLanguage } from '@codemirror/language';

// Simple TOML-like syntax highlighting using StreamLanguage
const tomlLanguage = StreamLanguage.define<unknown>({
  token(stream) {
    // Comments
    if (stream.match(/^#.*/)) return 'comment';
    // Strings
    if (stream.match(/^"[^"]*"/)) return 'string';
    if (stream.match(/^'[^']*'/)) return 'string';
    // Keys
    if (stream.match(/^[a-zA-Z_][a-zA-Z0-9_-]*\s*(?==)/)) return 'atom';
    if (stream.match(/^\[\[?[^\]]*\]\]?/)) return 'heading';
    // Table headers
    if (stream.match(/^\[[^\]]*\]/)) return 'heading';
    // Numbers
    if (stream.match(/^\d+(\.\d+)?([eE][+-]?\d+)?/)) return 'number';
    if (stream.match(/^true|false|yes|no|on|off/i)) return 'bool';
    // Skip whitespace
    if (stream.match(/^\s+/)) return null;
    // Operators and other
    stream.next();
    return null;
  },
});

interface TomlEditorProps {
  value: string;
  onChange?: (value: string) => void;
  readOnly?: boolean;
}

export default function TomlEditor({ value, onChange, readOnly }: TomlEditorProps) {
  const containerRef = useRef<HTMLDivElement>(null);
  const viewRef = useRef<EditorView | null>(null);

  useEffect(() => {
    if (!containerRef.current) return;

    const updateListener = EditorView.updateListener.of((update) => {
      if (update.docChanged) {
        onChange?.(update.state.doc.toString());
      }
    });

    const state = EditorState.create({
      doc: value,
      extensions: [
        basicSetup,
        tomlLanguage,
        updateListener,
        EditorView.theme({
          '&': { height: '100%', fontSize: '13px', fontFamily: '"Cascadia Code", "SF Mono", "Fira Code", monospace' },
          '.cm-scroller': { overflow: 'auto' },
          '.cm-content': { padding: '12px' },
          '.cm-gutters': { borderRight: '1px solid #E2E8F0', backgroundColor: '#F8FAFC' },
        }),
        EditorView.lineWrapping,
        readOnly ? EditorState.readOnly.of(true) : [],
      ],
    });

    viewRef.current = new EditorView({
      state,
      parent: containerRef.current,
    });

    return () => {
      viewRef.current?.destroy();
      viewRef.current = null;
    };
  }, []);

  // Update content when value changes externally
  useEffect(() => {
    const view = viewRef.current;
    if (!view || !onChange) return;
    const currentContent = view.state.doc.toString();
    if (currentContent !== value) {
      view.dispatch({
        changes: { from: 0, to: currentContent.length, insert: value },
      });
    }
  }, [value, onChange]);

  return (
    <div
      ref={containerRef}
      style={{ height: '100%', border: '1px solid var(--color-border)', borderRadius: 'var(--radius-sm)', overflow: 'hidden' }}
    />
  );
}
