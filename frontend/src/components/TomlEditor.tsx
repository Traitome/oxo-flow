import { useRef, useEffect } from 'react';
import { EditorView, basicSetup } from 'codemirror';
import { EditorState } from '@codemirror/state';
import { StreamLanguage } from '@codemirror/language';
import { toml } from '@codemirror/legacy-modes/mode/toml';

interface TomlEditorProps {
  value: string;
  onChange?: (value: string) => void;
  readOnly?: boolean;
  /** 1-based line to scroll to and select (validation errors, P2-8). */
  highlightLine?: number | null;
}

export default function TomlEditor({ value, onChange, readOnly, highlightLine }: TomlEditorProps) {
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
        // TOML syntax highlighting (issue #82 P2-2): previously the core
        // editor rendered as one flat color.
        StreamLanguage.define(toml),
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

  // Jump to the failing line when a validation error is clicked (issue
  // #82 P2-8: errors previously carried no way to locate the problem).
  useEffect(() => {
    const view = viewRef.current;
    if (!view || highlightLine == null) return;
    const line = view.state.doc.line(Math.min(highlightLine, view.state.doc.lines));
    view.dispatch({
      selection: { anchor: line.from },
      effects: EditorView.scrollIntoView(line.from, { y: 'center' }),
    });
    view.focus();
  }, [highlightLine]);

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
