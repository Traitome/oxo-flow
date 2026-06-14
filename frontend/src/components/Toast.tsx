import { useEffect, useState, useCallback } from 'react';

interface ToastItem { id: number; message: string; type: 'success' | 'error' | 'info'; }
let toastId = 0;
let addToastFn: ((msg: string, type: 'success' | 'error' | 'info') => void) | null = null;

export function showToast(message: string, type: 'success' | 'error' | 'info' = 'info') {
  addToastFn?.(message, type);
}

export default function Toast() {
  const [toasts, setToasts] = useState<ToastItem[]>([]);

  const add = useCallback((message: string, type: 'success' | 'error' | 'info') => {
    const id = ++toastId;
    setToasts((prev) => [...prev, { id, message, type }]);
    setTimeout(() => setToasts((prev) => prev.filter((t) => t.id !== id)), 4000);
  }, []);

  useEffect(() => { addToastFn = add; return () => { addToastFn = null; }; }, [add]);

  if (toasts.length === 0) return null;

  return (
    <div className="toast-container">
      {toasts.map((t) => (
        <div key={t.id} className={`toast toast-${t.type}`} onClick={() => setToasts((prev) => prev.filter((x) => x.id !== t.id))}>
          <span>{t.type === 'error' ? '✕' : t.type === 'success' ? '✓' : 'ℹ'}</span>
          <span>{t.message}</span>
        </div>
      ))}
    </div>
  );
}
