// Stable card slots resize without remounting drafts, branch searches, or the synchronized graph.
import { useEffect, useLayoutEffect, useRef, useState, type ReactNode } from "react";
import { useGitCards } from "../git-cards";
import { cardLayout, CARD_HEADER_MIN, CARD_SPLIT_DEFAULT } from "../git-card-layout";
import { startCardResize, cardResizeKey } from "../git-card-resize";

/** Measure the real available region and header geometry after banners, wrapping, and zoom changes. */
export function GitCardSplitter({ upper, lower, title }: { upper: ReactNode; lower: ReactNode; title: string }) {
  const region = useRef<HTMLDivElement>(null);
  const stop = useRef<() => void>(() => {});
  const share = useGitCards((s) => s.share);
  const upperOpen = useGitCards((s) => s.changes), lowerOpen = useGitCards((s) => s.graph);
  const [temporary, setTemporary] = useState<number>();
  const [size, setSize] = useState({ height: 0, upper: CARD_HEADER_MIN, lower: CARD_HEADER_MIN });
  const layout = cardLayout(size.height, temporary ?? share, size.upper, size.lower, upperOpen, lowerOpen);
  const latest = useRef(layout); latest.current = layout;
  useLayoutEffect(() => {
    const node = region.current;
    if (!node) return;
    const first = node.querySelector<HTMLElement>(".coding-section-toggle"), second = node.querySelector<HTMLElement>(".graph-heading");
    const header = (element: HTMLElement | null) => {
      const card = element?.parentElement;
      const style = card && getComputedStyle(card);
      return (element?.getBoundingClientRect().height ?? CARD_HEADER_MIN) + (style ? parseFloat(style.borderTopWidth) + parseFloat(style.borderBottomWidth) : 0);
    };
    const measure = () => setSize({ height: node.getBoundingClientRect().height, upper: header(first), lower: header(second) });
    const observer = new ResizeObserver(measure); observer.observe(node);
    if (first) observer.observe(first); if (second) observer.observe(second);
    measure(); return () => observer.disconnect();
  }, []);
  useEffect(() => () => stop.current(), [upperOpen, lowerOpen]);
  function start(event: React.PointerEvent<HTMLDivElement>) {
    stop.current();
    stop.current = startCardResize(event, { region: () => region.current?.getBoundingClientRect(), layout: () => latest.current, preview: setTemporary, commit: (value) => useGitCards.getState().setShare(value) });
  }
  function keys(event: React.KeyboardEvent<HTMLDivElement>) {
    const next = cardResizeKey(layout, event.key, event.shiftKey);
    if (next !== undefined) { event.preventDefault(); useGitCards.getState().setShare(next); }
    else if (["ArrowUp", "ArrowDown", "Home"].includes(event.key)) event.preventDefault();
  }
  const percentage = Math.round(layout.upper / layout.usable * 100);
  return <div ref={region} className="git-card-split">
    <div key="upper" className="git-card-slot" style={{ height: layout.upper }}>{upper}</div>
    {upperOpen && lowerOpen ? <div key="separator" className="git-card-handle" role="separator" tabIndex={0} aria-orientation="horizontal" aria-label="Resize Source Control and Graph" aria-valuenow={percentage} aria-valuemin={Math.round(layout.minimum * 100)} aria-valuemax={Math.round(layout.maximum * 100)} aria-valuetext={`${title} ${percentage}%, Graph ${100 - percentage}%`} title="Drag to resize. ↑/↓ adjusts; Shift adjusts faster. Double-click or Home resets." onPointerDown={start} onKeyDown={keys} onDoubleClick={() => useGitCards.getState().setShare(CARD_SPLIT_DEFAULT)} /> : <div key="separator" style={{ height: layout.separator, flex: "none" }} />}
    <div key="lower" className="git-card-slot" style={{ height: layout.lower }}>{lower}</div>
  </div>;
}
