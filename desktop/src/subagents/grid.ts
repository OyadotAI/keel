// Only confirmed endings remove live terminal panes; observation loss retains them.
import { useEffect, useLayoutEffect, useMemo, useRef, useState, type RefObject } from "react";
import type { ChildState, Invocation } from "./model";
import { GRID_MIN_WIDTH, GRID_MAX_COLUMNS } from "./constants";
import { createGridMotion } from "./grid-motion";
export { flipFrom } from "./grid-motion";
export { useDeparting, withDeparting, type Shown } from "./presence";
const ended = new Set(["done", "failed", "cancelled", "interrupted"]);
/** Missing evidence cannot become fabricated completion. */
export function liveInvocations(before: Invocation[], snapshot?: ChildState): Invocation[] {
  const current = new Map((snapshot?.invocations ?? []).map((child) => [child.id, child]));
  const result: Invocation[] = [];
  // A child the daemon now reports under another id — Antigravity's `:pending` placeholder becomes
  // `:<run>` once its database opens — is the same child, not a lost one left "unconfirmed".
  const reported = new Set((snapshot?.invocations ?? []).map((child) => child.child).filter(Boolean));
  for (const prior of before) {
    const next = current.get(prior.id);
    current.delete(prior.id);
    if (!next && prior.child && reported.has(prior.child)) continue;
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
/** Separate layout transforms from each tile's visual reveal; resize discards stale targets. */
export function useFlip(root: RefObject<HTMLElement | null>, key: string, reduced = false): void {
  const motion = useRef<ReturnType<typeof createGridMotion> | null>(null);
  motion.current ??= createGridMotion();
  useLayoutEffect(() => { if (root.current) motion.current!.update(root.current, reduced); }, [root, key, reduced]);
  useEffect(() => {
    const element = root.current; if (!element) return;
    const owner = motion.current!;
    let width = element.clientWidth, height = element.clientHeight;
    const observer = new ResizeObserver(() => {
      if (width === element.clientWidth && height === element.clientHeight) return;
      width = element.clientWidth; height = element.clientHeight; owner.reset(element);
    });
    observer.observe(element);
    return () => { observer.disconnect(); owner.cancel(); };
  }, [root]);
}
