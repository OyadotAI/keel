// Only confirmed endings remove live terminal panes; observation loss retains them.
import { useEffect, useLayoutEffect, useMemo, useRef, useState, type RefObject } from "react";
import type { ChildState, Invocation } from "./model";
import { GRID_MIN_WIDTH, GRID_MAX_COLUMNS, PANE_CLOSE_MS, PANE_MOVE_MS } from "./constants";
const ended = new Set(["done", "failed", "cancelled", "interrupted"]);
/** Missing evidence cannot become fabricated completion. */
export function liveInvocations(before: Invocation[], snapshot?: ChildState): Invocation[] {
  const current = new Map((snapshot?.invocations ?? []).map((child) => [child.id, child]));
  const result: Invocation[] = [];
  for (const prior of before) {
    const next = current.get(prior.id);
    current.delete(prior.id);
    if (next && ended.has(next.status)) continue;
    result.push(next ? { ...next, status: snapshot?.connected ? next.status : "unknown" } : { ...prior, status: "unknown" });
  }
  for (const child of current.values()) {
    if (!ended.has(child.status)) result.push({ ...child, status: snapshot?.connected ? child.status : "unknown" });
  }
  return result;
}
/** A replacement conversation cannot inherit another parent's children. */
export function useLiveInvocations(scope: string, snapshot?: ChildState): Invocation[] {
  const [previous, setPrevious] = useState(() => ({ scope, children: liveInvocations([], snapshot) }));
  useEffect(() => setPrevious((before) => ({ scope, children: liveInvocations(before.scope === scope ? before.children : [], snapshot) })), [scope, snapshot]);
  return useMemo(() => liveInvocations(previous.scope === scope ? previous.children : [], snapshot), [previous, scope, snapshot]);
}
/** Columns grow only while each terminal remains readable. */
export function gridColumns(width: number, count: number): number {
  return Math.max(1, Math.min(Math.floor(width / GRID_MIN_WIDTH), count <= 4 ? 2 : GRID_MAX_COLUMNS, count));
}
/** Offscreen panes keep lifecycle while detailed output reads pause. */
export function useVisibleChildren(root: RefObject<HTMLDivElement | null>, ids: string[], hidden: boolean): string[] {
  const key = JSON.stringify(ids);
  const [visible, setVisible] = useState<string[]>([]);
  useEffect(() => {
    const element = root.current;
    if (!element || hidden) { setVisible([]); return; }
    const active = new Set<string>();
    const observer = new IntersectionObserver((entries) => {
      for (const entry of entries) {
        const id = (entry.target as HTMLElement).dataset.childInvocation!;
        if (entry.isIntersecting) active.add(id); else active.delete(id);
      }
      setVisible([...active]);
    }, { root: element });
    element.querySelectorAll("[data-child-invocation]").forEach((node) => observer.observe(node));
    return () => observer.disconnect();
  }, [root, key, hidden]);
  return hidden ? [] : visible.filter((id) => ids.includes(id));
}
/** A pane on its way out, still at the slot it held. */
export type Shown = Invocation & { leaving?: boolean };
interface Departure { child: Invocation; at: number }
/** Departed panes are put back at their old index so nothing else moves until they are gone. */
export function withDeparting(children: Invocation[], gone: Departure[]): Shown[] {
  const result: Shown[] = [...children];
  for (const { child, at } of [...gone].sort((a, b) => a.at - b.at)) {
    if (!result.some((c) => c.id === child.id)) result.splice(Math.min(at, result.length), 0, { ...child, leaving: true });
  }
  return result;
}
/** A finished pane lingers for its close animation; a grid that reflows first is a pane nobody saw end. */
export function useDeparting(children: Invocation[], ms = PANE_CLOSE_MS): Shown[] {
  const [gone, setGone] = useState<Departure[]>([]);
  const seen = useRef(children), timers = useRef(new Set<ReturnType<typeof setTimeout>>());
  useEffect(() => {
    const ids = new Set(children.map((child) => child.id));
    const left = seen.current.map((child, at) => ({ child, at })).filter(({ child }) => !ids.has(child.id));
    seen.current = children;
    if (!left.length) return;
    setGone((before) => [...before.filter(({ child }) => !left.some((l) => l.child.id === child.id)), ...left]);
    const timer = setTimeout(() => { timers.current.delete(timer); setGone((before) => before.filter((entry) => !left.includes(entry))); }, ms);
    timers.current.add(timer);
  }, [children, ms]);
  useEffect(() => { const pending = timers.current; return () => pending.forEach(clearTimeout); }, []);
  return useMemo(() => withDeparting(children, gone), [children, gone]);
}
type Box = Pick<DOMRect, "left" | "top" | "width" | "height">;
/** The transform that puts a pane back in the box it had, or nothing when it has not moved. */
export function flipFrom(was: Box, now: Box): string | undefined {
  if (!now.width || !now.height) return;
  const dx = was.left - now.left, dy = was.top - now.top, sx = was.width / now.width, sy = was.height / now.height;
  if (!dx && !dy && sx === 1 && sy === 1) return;
  return `translate(${dx}px, ${dy}px) scale(${sx}, ${sy})`;
}
const stillness = () => typeof matchMedia === "function" && matchMedia("(prefers-reduced-motion: reduce)").matches;
/** The grid retracks in one frame; each pane then plays the move from where it was, so a split
 * is seen happening rather than found done. Boxes are re-measured on resize so a window dragged
 * between two splits does not replay a stale box. */
export function useFlip(root: RefObject<HTMLElement | null>, key: string): void {
  const boxes = useRef(new Map<string, Box>());
  const measure = (element: HTMLElement) => new Map([...element.querySelectorAll<HTMLElement>("[data-pane]")].map((pane) => [pane.dataset.pane!, pane.getBoundingClientRect()]));
  useLayoutEffect(() => {
    const element = root.current; if (!element) return;
    const was = boxes.current, now = measure(element);
    boxes.current = now;
    if (stillness()) return;
    for (const pane of element.querySelectorAll<HTMLElement>("[data-pane]")) {
      const before = was.get(pane.dataset.pane!), after = now.get(pane.dataset.pane!);
      const transform = before && after && flipFrom(before, after);
      if (transform) pane.animate([{ transformOrigin: "0 0", transform }, { transformOrigin: "0 0", transform: "none" }], { duration: PANE_MOVE_MS, easing: "cubic-bezier(.2, .8, .2, 1)" });
    }
  }, [root, key]);
  useEffect(() => {
    const element = root.current; if (!element) return;
    const observer = new ResizeObserver(() => { boxes.current = measure(element); });
    observer.observe(element); return () => observer.disconnect();
  }, [root]);
}
