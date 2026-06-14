import { useRef, useEffect } from 'react';
import { EditorView, basicSetup } from 'codemirror';
import { EditorState } from '@codemirror/state';

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
