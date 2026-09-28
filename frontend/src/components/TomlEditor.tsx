import { useRef, useEffect, useState } from 'react';
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
  // #580: CodeMirror catches plugin crashes internally (logException →
  // window.onerror → console.error) before React's error boundaries ever
  // see them, leaving a silently-dead editor. exceptionSink hands those
  // crashes back to us so they can surface as visible UI degradation.
  const [crashCount, setCrashCount] = useState(0);

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
        EditorView.exceptionSink.of((error) => {
          console.error('[TomlEditor] CodeMirror plugin crashed:', error);
          setCrashCount((c) => c + 1);
        }),
        EditorView.theme({
          '&': { height: '100%', fontSize: '13px', fontFamily: '"Cascadia Code", "SF Mono", "Fira Code", monospace', backgroundColor: 'var(--color-bg)', color: 'var(--color-text)' },
          '.cm-scroller': { overflow: 'auto' },
          '.cm-content': { padding: '12px', caretColor: 'var(--color-text)' },
          '.cm-gutters': {
            borderRight: '1px solid var(--color-border)',
            backgroundColor: 'var(--color-bg-secondary)',
            color: 'var(--color-text-tertiary)',
          },
          '.cm-activeLineGutter': { backgroundColor: 'var(--color-bg-tertiary)' },
          '.cm-activeLine': { backgroundColor: 'var(--color-bg-tertiary)' },
          '.cm-selectionBackground': { backgroundColor: 'var(--color-primary-light)' },
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
    // Editor is intentionally initialized once per mount; recreating it on
    // prop changes would destroy cursor state and is handled by effects below.
    // eslint-disable-next-line react-hooks/exhaustive-deps
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

  if (crashCount > 0) {
    return (
      <div>
        <div className="result-bar error" role="alert">
          <span>
            Editor extension crashed ({crashCount} {crashCount === 1 ? 'time' : 'times'}). Editing still works, but highlighting or autocompletion may be degraded — switch modes or reload to fully reset.
          </span>
        </div>
        <div
          ref={containerRef}
          style={{ height: '100%', border: '1px solid var(--color-border)', borderRadius: 'var(--radius-sm)', overflow: 'hidden' }}
        />
      </div>
    );
  }

  return (
    <div
      ref={containerRef}
      style={{ height: '100%', border: '1px solid var(--color-border)', borderRadius: 'var(--radius-sm)', overflow: 'hidden' }}
    />
  );
}
