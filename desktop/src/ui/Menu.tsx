import { cover } from "../cover";
import { useEffect, useRef, type ReactNode } from "react";

/// A small menu or popover anchored under its button. Esc or a click outside closes it.
export function Floating({ anchor, onClose, children, kind = "menu", width }: { anchor: DOMRect; onClose: () => void; children: ReactNode; kind?: "menu" | "popover"; width?: number }) {
  const ref = useRef<HTMLDivElement>(null);
  useEffect(() => cover(), []);
  useEffect(() => {
    const key = (e: KeyboardEvent) => e.key === "Escape" && onClose();
    window.addEventListener("keydown", key);
    ref.current?.querySelector<HTMLElement>("button:not(:disabled)")?.focus();
    return () => window.removeEventListener("keydown", key);
  }, [onClose]);
  const w = width ?? (kind === "menu" ? 240 : 340);
  // A zero-size anchor is a pointer (a right-click): open at it rather than under a button.
  const point = anchor.width === 0 && anchor.height === 0;
  const left = Math.max(8, Math.min(point ? anchor.left : anchor.right - w, window.innerWidth - w - 8));
  const top = Math.min(anchor.bottom + (point ? 0 : 4), window.innerHeight - 260);
  return (
    <>
      <div className="scrim" onClick={onClose} />
      <div ref={ref} className={kind} role={kind === "menu" ? "menu" : "dialog"} style={{ top, left, width: w }} onKeyDown={(e) => {
        if (kind !== "menu" || !["ArrowDown", "ArrowUp", "Home", "End"].includes(e.key)) return;
        const items = [...e.currentTarget.querySelectorAll<HTMLElement>("button:not(:disabled)")];
        const at = items.indexOf(document.activeElement as HTMLElement);
        const next = e.key === "Home" ? 0 : e.key === "End" ? items.length - 1 : (at + (e.key === "ArrowDown" ? 1 : -1) + items.length) % items.length;
        items[next]?.focus();
        e.preventDefault();
      }}>
        {children}
      </div>
    </>
  );
}
