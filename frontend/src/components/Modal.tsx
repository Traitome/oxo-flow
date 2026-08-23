import { useEffect, useRef } from 'react';

const FOCUSABLE = 'button, [href], input, select, textarea, [tabindex]:not([tabindex="-1"])';

export interface ModalProps {
  /** Called on Escape or backdrop click. */
  onClose: () => void;
  /** id of the element that labels the dialog (aria-labelledby). */
  labelledBy: string;
  /** Extra class on the dialog panel (e.g. 'inspector-dialog'). */
  className?: string;
  children: React.ReactNode;
}

/**
 * Shared modal overlay: renders `.modal-overlay` > `.modal-dialog` (backdrop
 * click closes, matching the previous per-dialog behavior) and adds the
 * accessibility behavior the hand-rolled dialogs lacked — Escape closes,
 * focus moves into the dialog on open, Tab/Shift+Tab cycle within it, and
 * focus returns to the previously focused element on close.
 */
export default function Modal({ onClose, labelledBy, className, children }: ModalProps) {
  const dialogRef = useRef<HTMLDivElement>(null);
  // Keep the latest onClose without re-running the focus effect (the parent
  // passes an inline closure whose identity changes on every render).
  const onCloseRef = useRef(onClose);
  useEffect(() => {
    onCloseRef.current = onClose;
  });

  useEffect(() => {
    const dialog = dialogRef.current;
    if (!dialog) return;
    const previouslyFocused = document.activeElement instanceof HTMLElement ? document.activeElement : null;
    const focusable = () =>
      Array.from(dialog.querySelectorAll<HTMLElement>(FOCUSABLE)).filter((el) => !el.hasAttribute('disabled'));

    // Initial focus goes to the first focusable control in the dialog.
    focusable()[0]?.focus();

    const onKeyDown = (e: KeyboardEvent) => {
      if (e.key === 'Escape') {
        e.stopPropagation();
        onCloseRef.current();
        return;
      }
      if (e.key !== 'Tab') return;
      const items = focusable();
      if (items.length === 0) {
        e.preventDefault();
        return;
      }
      const first = items[0];
      const last = items[items.length - 1];
      const active = document.activeElement;
      if (e.shiftKey) {
        if (active === first || !dialog.contains(active)) {
          e.preventDefault();
          last.focus();
        }
      } else if (active === last || !dialog.contains(active)) {
        e.preventDefault();
        first.focus();
      }
    };
    document.addEventListener('keydown', onKeyDown);
    return () => {
      document.removeEventListener('keydown', onKeyDown);
      previouslyFocused?.focus();
    };
  }, []);

  return (
    <div className="modal-overlay" onClick={() => onCloseRef.current()}>
      <div
        ref={dialogRef}
        className={className ? `modal-dialog ${className}` : 'modal-dialog'}
        role="dialog"
        aria-modal="true"
        aria-labelledby={labelledBy}
        onClick={(e) => e.stopPropagation()}
      >
        {children}
      </div>
    </div>
  );
}
