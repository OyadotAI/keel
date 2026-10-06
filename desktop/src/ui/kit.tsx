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
/// The visible lane's terminal — where typing goes when nothing else is asking for it.
export function focusTerminal() {
  document.querySelector<HTMLElement>(".agents .terminal:not(.hidden) textarea")?.focus();
}

/// Give focus back to what had it, if that is still on screen; otherwise to the visible terminal.
/// A lane switch or a close can leave `back` hidden or gone, and focus on a hidden xterm types
/// into a lane nobody is looking at.
/// Only when focus was actually lost: whatever the close opened (a dialog, the rename field) or the
/// place the person clicked keeps it.
export function restoreFocus(back: Element | null) {
  const now = document.activeElement;
  if (now && now !== document.body && !now.closest(".hidden")) return;
  const el = back as HTMLElement | null;
  if (el?.isConnected && (el.checkVisibility?.() ?? true) && !el.closest(".hidden")) el.focus();
  else focusTerminal();
}

/// ↑↓ between the files in a panel list (anything marked `data-file`), Esc back to the terminal.
/// Plain keys, safe because they only arrive while focus is inside the panel.
export function walkFiles(e: KeyboardEvent<HTMLElement>) {
  if (e.key === "Escape" && (e.target as HTMLElement).closest("[data-file]")) return (e.preventDefault(), focusTerminal());
  if (e.key !== "ArrowDown" && e.key !== "ArrowUp") return;
  const all = [...e.currentTarget.querySelectorAll<HTMLElement>("[data-file]:not(:disabled)")];
  const at = all.indexOf(document.activeElement as HTMLElement);
  if (at < 0 && document.activeElement !== e.currentTarget) return;
  e.preventDefault();
  const next = all[Math.max(0, Math.min(all.length - 1, at + (e.key === "ArrowDown" ? 1 : -1)))];
  next?.focus();
  next?.scrollIntoView({ block: "nearest" });
}

export function Dialog({ title, children, onClose, tone }: { title: string; children: ReactNode; onClose: () => void; tone?: "danger" }) {
  const ref = useRef<HTMLDivElement>(null);
  // Callers pass a fresh arrow every render; the effect below must run once per dialog, or a
  // parent that re-renders (the status bar ticks twice a second) pulls focus out of the modal.
  const closeRef = useRef(onClose);
  closeRef.current = onClose;
  useEffect(() => cover(), []);
  useEffect(() => {
    const key = (e: globalThis.KeyboardEvent) => e.key === "Escape" && closeRef.current();
    window.addEventListener("keydown", key);
    const back = document.activeElement;
    ref.current?.querySelector<HTMLElement>(".actions button")?.focus();
    return () => {
      window.removeEventListener("keydown", key);
      // After the close's own state change has rendered, so a lane it removed is gone.
      requestAnimationFrame(() => restoreFocus(back));
    };
  }, []);
  return (
    <div className="dialog-backdrop" onMouseDown={(e) => e.target === e.currentTarget && onClose()}>
      <div ref={ref} className={`dialog${tone ? ` ${tone}` : ""}`} role="dialog" aria-modal="true" aria-label={title}>
        <h2>{title}</h2>
        {children}
      </div>
    </div>
  );
}
