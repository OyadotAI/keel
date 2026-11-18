// A finished tile keeps its slot until the whole burst ends, so the grid re-tracks once, not per child.
import { useEffect, useState } from "react";
import type { Invocation } from "./model";
import { PANE_CLOSE_MS, SPLIT_LINGER_MS } from "./constants";

/** A tile's `slot` is its key: it outlives a re-keyed child, and a new child in a held slot gets a new one. */
export type Shown = Invocation & { slot: string; held?: boolean; leaving?: boolean };
/** `quiet` is when the last child ended; `closing` is when every tile began to dissolve together. */
export interface Presence { scope: string; slots: Shown[]; live?: Invocation[]; idle?: boolean; quiet?: number; closing?: number }
/** How long an all-finished split is held, and how long its tiles take to dissolve. */
export interface Timing { linger: number; close: number }

const fresh = (child: Invocation): Shown => ({ ...child, slot: child.id });
/** New children take the earliest finished slot, under their own key, before the grid grows. */
function refill(slots: Shown[], arrivals: Invocation[]): Shown[] {
  const queue = [...arrivals];
  const filled = slots.map((slot) => (slot.held && queue.length ? fresh(queue.shift()!) : slot));
  return [...filled, ...queue.map(fresh)];
}
/** A slot follows its own id first; a child the daemon re-keyed (Antigravity's `:pending`) keeps its slot.
 *  An id the daemon still reports was not re-keyed: a reused Codex child's next turn is a new tile. */
function successors(slots: Shown[], live: Invocation[], known: Map<string, Invocation>): Map<Shown, Invocation> {
  const left = new Set(live), found = new Map<Shown, Invocation>();
  const claim = (same: (slot: Shown, child: Invocation) => boolean) => {
    for (const slot of slots) {
      const next = !found.has(slot) && live.find((child) => left.has(child) && same(slot, child));
      if (next) { found.set(slot, next); left.delete(next); }
    }
  };
  claim((slot, child) => child.id === slot.id);
  claim((slot, child) => !known.has(slot.id) && !!child.child && child.child === slot.child);
  return found;
}

/** Only a child that stopped being live is held, with the end status the daemon reported for it. */
export function holdSlots(before: Presence, scope: string, live: Invocation[], reported: Invocation[] = [], now: number): Presence {
  const prior: Presence = before.scope === scope ? before : { scope, slots: [] };
  const ended = new Map(reported.map((child) => [child.id, child]));
  const next = successors(prior.slots, live, ended), taken = new Set(next.values()), busy = live.length > 0;
  // Re-keyed and ended in one snapshot: the end is reported under the new id, for the same child.
  const end = (slot: Shown) => ended.get(slot.id) ?? (slot.child ? reported.filter((child) => child.child === slot.child).at(-1) : undefined);
  const kept = prior.slots.map((slot): Shown => {
    const child = next.get(slot);
    return child ? { ...child, slot: slot.slot } : { ...slot, ...end(slot), slot: slot.slot, held: true, leaving: busy ? undefined : slot.leaving };
  });
  const slots = refill(kept, live.filter((child) => !taken.has(child)));
  return { scope, slots, live, quiet: busy || !slots.length ? undefined : prior.quiet ?? now, closing: busy ? undefined : prior.closing };
}

/** The hold ends at the linger, or at once when the lane is idle; the tiles then leave together. */
export function expire(state: Presence, now: number, idle: boolean, { linger, close }: Timing): Presence {
  if (state.quiet === undefined) return state;
  const closing = state.closing ?? (idle || now >= state.quiet + linger ? now : undefined);
  if (closing === undefined) return state;
  if (now >= closing + close) return { ...state, slots: [], quiet: undefined, closing: undefined };
  return state.closing === closing ? state : { ...state, closing, slots: state.slots.map((slot) => ({ ...slot, leaving: true })) };
}

/** The one moment the hold next changes by itself, or nothing while a child still works. */
export function deadline(state: Presence, idle: boolean, { linger, close }: Timing): number | undefined {
  if (state.closing !== undefined) return state.closing + close;
  if (state.quiet === undefined) return undefined;
  return idle ? state.quiet : state.quiet + linger;
}

/** Reconciliation holds a finished child before commit; one bounded timer ends the hold. */
export function useHeldSlots(live: Invocation[], reported: Invocation[] | undefined, scope: string, idle: boolean, linger = SPLIT_LINGER_MS, close = PANE_CLOSE_MS): Shown[] {
  const [saved, setSaved] = useState<Presence>(() => ({ scope, slots: [] }));
  let current = saved;
  if (saved.scope !== scope || saved.live !== live || saved.idle !== idle) {
    const now = Date.now();
    current = { ...expire(holdSlots(saved, scope, live, reported, now), now, idle, { linger, close }), idle };
    setSaved(current);
  }
  const at = deadline(current, idle, { linger, close });
  useEffect(() => {
    if (at === undefined) return;
    // A timer may fire a millisecond early; expiring at the deadline itself means it never has to be asked twice.
    const timer = setTimeout(() => setSaved((before) => expire(before, Math.max(Date.now(), at), idle, { linger, close })), Math.max(0, at - Date.now()));
    return () => clearTimeout(timer);
  }, [at, idle, linger, close]);
  return current.slots;
}
