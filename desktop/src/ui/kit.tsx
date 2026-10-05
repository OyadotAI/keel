// The shell's primitives, after agentchrome's `ui/`: one Tabs, one empty state, one banner. Each
// of these was hand-written in four places and had drifted — four tablists, none with arrow keys.

import { cover } from "../cover";
import { useEffect, useRef, type KeyboardEvent, type ReactNode } from "react";

/// A tab row with roving focus: only the selected tab is in the Tab order, ←/→ move between them.
export function Tabs<T extends string>({ tabs, value, onChange, children, className }: { tabs: readonly { id: T; label: ReactNode; title?: string }[]; value: T; onChange: (id: T) => void; children?: ReactNode; className?: string }) {
  const move = (e: KeyboardEvent<HTMLDivElement>) => {
    const step = e.key === "ArrowRight" ? 1 : e.key === "ArrowLeft" ? -1 : 0;
    if (!step) return;
    e.preventDefault();
    const i = tabs.findIndex((t) => t.id === value);
    const next = tabs[(i + step + tabs.length) % tabs.length];
    onChange(next.id);
    e.currentTarget.querySelector<HTMLElement>(`[data-tab="${next.id}"]`)?.focus();
  };
  return (
    <div className={`tabs${className ? ` ${className}` : ""}`} role="tablist" onKeyDown={move}>
      {tabs.map((t) => (
        <button key={t.id} data-tab={t.id} role="tab" aria-selected={t.id === value} tabIndex={t.id === value ? 0 : -1} title={t.title} onClick={() => onChange(t.id)}>
          {t.label}
        </button>
      ))}
      {children}
    </div>
  );
}

/// What a pane shows when it has nothing: which of the reasons it is, and the way out.
export function Empty({ title, children, action }: { title: string; children?: ReactNode; action?: ReactNode }) {
  return (
    <div className="empty-state">
      <strong>{title}</strong>
      {children && <p>{children}</p>}
      {action}
    </div>
  );
}

/// A line that reports how something went, read out by assistive technology when it changes.
export function Banner({ tone = "note", children }: { tone?: "note" | "error" | "warn"; children: ReactNode }) {
  return (
    <p className={`banner-line ${tone}`} role={tone === "error" ? "alert" : "status"} aria-live="polite">
      {children}
    </p>
  );
}

/// A count beside a tab's name.
export const Count = ({ n }: { n?: number }) => (n ? <span className="tab-count">{n}</span> : null);

/// A modal question in the middle of the window: dimmed behind, Esc or the backdrop cancels, and
/// focus lands on the first button. For decisions — closing, merging, discarding — that should not
/// be a small box beside whatever was clicked.
export function Dialog({ title, children, onClose, tone }: { title: string; children: ReactNode; onClose: () => void; tone?: "danger" }) {
  const ref = useRef<HTMLDivElement>(null);
  useEffect(() => cover(), []);
  useEffect(() => {
    const key = (e: globalThis.KeyboardEvent) => e.key === "Escape" && onClose();
    window.addEventListener("keydown", key);
    const back = document.activeElement as HTMLElement | null;
    ref.current?.querySelector<HTMLElement>(".actions button")?.focus();
    return () => {
      window.removeEventListener("keydown", key);
      back?.focus?.();
    };
  }, [onClose]);
  return (
    <div className="dialog-backdrop" onMouseDown={(e) => e.target === e.currentTarget && onClose()}>
      <div ref={ref} className={`dialog${tone ? ` ${tone}` : ""}`} role="dialog" aria-modal="true" aria-label={title}>
        <h2>{title}</h2>
        {children}
      </div>
    </div>
  );
}
