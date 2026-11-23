// Navigation revalidates received evidence and bounds focus work to one explicit user action.
import { attentionSelection, type AttentionDestination, type AttentionSource } from "./attention";

const FOCUS_ATTEMPTS = 30;
/** Mirrors the shell's drawer breakpoint so explicit navigation can uncover narrow destinations. */
export const ATTENTION_DRAWER_QUERY = "(max-width: 900px)";

/** Navigation is injected so the list cannot execute work, approve requests, or own their state. */
export interface AttentionNavigation {
  read(): AttentionSource;
  select(lane: string): void;
  reveal(lane: string, destination: AttentionDestination): void;
}

/** Explicit navigation replaces covering layout state so old panels cannot obstruct the destination. */
export function attentionLayout(destination: AttentionDestination, drawer = false): { panel: boolean; conversation: boolean; expanded: false; tab: "turns"; hideSidebar: boolean } {
  const evidence = destination.kind === "checks" || destination.kind === "update";
  return { panel: evidence, conversation: !evidence, expanded: false, tab: "turns", hideSidebar: drawer };
}

/** App keeps ownership while one reveal operation consistently clears every covering surface. */
export function revealAttention(destination: AttentionDestination, actions: { panel(value: boolean): void; conversation(value: boolean): void; expanded(value: boolean): void; tab(value: "turns"): void; sidebar(value: boolean): void }, drawer: boolean): void {
  const target = attentionLayout(destination, drawer);
  actions.panel(target.panel); actions.conversation(target.conversation);
  actions.expanded(target.expanded); actions.tab(target.tab);
  if (target.hideSidebar) actions.sidebar(false);
}

/** Selection uses the current reason; disappearing rows cannot resurrect removed lanes. */
export function activateAttention(lane: string, navigation: AttentionNavigation): AttentionDestination | undefined {
  const row = attentionSelection(navigation.read(), lane);
  if (!row) return;
  navigation.select(lane);
  navigation.reveal(lane, row.destination);
  return row.destination;
}

/** A finite mount wait is cancellable and stops as soon as another action owns navigation. */
export function deferAttentionFocus(options: { current(): boolean; focus(): boolean; fallback(): void; frame(callback: () => void): number; cancel(id: number): void }): () => void {
  let attempts = 0, frame: number | undefined, cancelled = false;
  const tick = () => {
    if (cancelled || !options.current()) return;
    if (options.focus()) return;
    attempts++;
    if (attempts >= FOCUS_ATTEMPTS) { options.fallback(); return; }
    frame = options.frame(tick);
  };
  frame = options.frame(tick);
  return () => { cancelled = true; if (frame !== undefined) options.cancel(frame); };
}
