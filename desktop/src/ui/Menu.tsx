import { cover } from "../cover";
import { createPortal } from "react-dom";
import { useEffect, useLayoutEffect, useRef, useState, type ReactNode } from "react";

/// A small menu or popover anchored under its button. Esc or a click outside closes it.
export function Floating({ anchor, onClose, children, kind = "menu", width }: { anchor: DOMRect; onClose: () => void; children: ReactNode; kind?: "menu" | "popover"; width?: number }) {
  const ref = useRef<HTMLDivElement>(null);
  const [position, setPosition] = useState({ left: 8, top: 8 });
  const w = Math.min(width ?? (kind === "menu" ? 240 : 340), window.innerWidth - 16);
  useLayoutEffect(() => {
    const place = () => {
      const el = ref.current;
      if (!el) return;
      const point = anchor.width === 0 && anchor.height === 0;
      const left = Math.max(8, Math.min(point ? anchor.left : anchor.right - el.offsetWidth, window.innerWidth - el.offsetWidth - 8));
      const below = anchor.bottom + (point ? 0 : 4);
      const above = anchor.top - el.offsetHeight - (point ? 0 : 4);
      const preferred = below + el.offsetHeight <= window.innerHeight - 8 ? below : above;
      const top = Math.max(8, Math.min(preferred, window.innerHeight - el.offsetHeight - 8));
      setPosition((old) => old.left === left && old.top === top ? old : { left, top });
    };
    place();
    const observer = new ResizeObserver(place);
    if (ref.current) observer.observe(ref.current);
    window.addEventListener("resize", place);
    return () => { observer.disconnect(); window.removeEventListener("resize", place); };
  }, [anchor, w]);
  useEffect(() => cover(), []);
  useEffect(() => {
    const key = (e: KeyboardEvent) => e.key === "Escape" && onClose();
    window.addEventListener("keydown", key);
    ref.current?.querySelector<HTMLElement>("button:not(:disabled)")?.focus();
    return () => window.removeEventListener("keydown", key);
  }, [onClose]);
  return createPortal(
    <>
      <div className="scrim" onClick={onClose} />
      <div ref={ref} className={kind} role={kind === "menu" ? "menu" : "dialog"} style={{ ...position, width: w, minWidth: 0, maxWidth: "calc(100vw - 16px)", maxHeight: "calc(100dvh - 16px)", overflowY: "auto" }} onKeyDown={(e) => {
        if (kind !== "menu" || !["ArrowDown", "ArrowUp", "Home", "End"].includes(e.key)) return;
        const items = [...e.currentTarget.querySelectorAll<HTMLElement>("button:not(:disabled)")];
        const at = items.indexOf(document.activeElement as HTMLElement);
        const next = e.key === "Home" ? 0 : e.key === "End" ? items.length - 1 : (at + (e.key === "ArrowDown" ? 1 : -1) + items.length) % items.length;
        items[next]?.focus();
        e.preventDefault();
      }}>
        {children}
      </div>
    </>, document.body,
  );
}
