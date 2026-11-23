// App owns destination reveal; this hook cancels stale focus instead of changing source lifetimes.
import { useCallback, useEffect, useRef, useState } from "react";
import { useStore } from "./store";
import { terminalLease } from "./typers";
import { activateAttention, deferAttentionFocus } from "./attention-navigation";
import type { AttentionDestination } from "./attention";

function laneRoot(lane: string): HTMLElement | null {
  return document.querySelector(`[data-attention-lane="${CSS.escape(lane)}"]`);
}

function focusElement(element?: HTMLElement | null): boolean {
  if (!element || !(element.checkVisibility?.() ?? true)) return false;
  element.scrollIntoView({ block: "nearest" });
  element.focus();
  return document.activeElement === element;
}

function focusDestination(lane: string, destination: AttentionDestination): boolean {
  if (destination.kind === "terminal") {
    const lease = terminalLease(lane);
    if (!lease) return false;
    lease.focus();
    return true;
  }
  const root = laneRoot(lane);
  const selectors: Record<string, string> = { request: `[data-attention-request="${CSS.escape(destination.kind === "request" ? destination.request : "")}"]`, error: "[data-attention-error]" };
  if (destination.kind === "checks" || destination.kind === "update") return focusElement(document.querySelector<HTMLElement>(`[data-attention-turns="${CSS.escape(lane)}"]`));
  const target = root?.querySelector<HTMLElement>(selectors[destination.kind]);
  if (destination.kind === "request" || destination.kind === "error") return focusElement(target?.querySelector<HTMLElement>("button:not(:disabled), textarea:not(:disabled), input:not(:disabled)") ?? target);
  return focusElement(target);
}

/** Revalidation and selection are shared across layouts; no row starts an agent or sends an answer. */
export function useAttentionNavigation(reveal: (lane: string, destination: AttentionDestination) => void) {
  const [open, setOpen] = useState(false);
  const cancel = useRef<(() => void) | undefined>(undefined);
  const cancelFocus = useCallback(() => { cancel.current?.(); cancel.current = undefined; }, []);
  useEffect(() => {
    window.addEventListener("pointerdown", cancelFocus);
    window.addEventListener("keydown", cancelFocus);
    return () => { cancelFocus(); window.removeEventListener("pointerdown", cancelFocus); window.removeEventListener("keydown", cancelFocus); };
  }, [cancelFocus]);
  const show = useCallback(() => { cancelFocus(); setOpen(true); }, [cancelFocus]);
  function activate(lane: string) {
    cancelFocus();
    const destination = activateAttention(lane, { read: useStore.getState, select: (id) => useStore.getState().select(id), reveal });
    if (!destination) return;
    setOpen(false);
    cancel.current = deferAttentionFocus({
      current: () => useStore.getState().active === lane && !!useStore.getState().lanes[lane] && !document.querySelector('[role="dialog"], [role="menu"], .palette'),
      focus: () => focusDestination(lane, destination),
      fallback: () => { focusElement(laneRoot(lane)?.querySelector<HTMLElement>(".lane-name")); },
      frame: requestAnimationFrame, cancel: cancelAnimationFrame,
    });
  }
  return { open, show, close: () => setOpen(false), activate };
}
