// Finished tiles retain their DOM identity until dissolution ends, without leaking across parents.
import { useEffect, useState } from "react";
import type { Invocation } from "./model";
import { PANE_CLOSE_MS } from "./constants";

/** A closing tile remains in its original slot until its own deadline. */
export type Shown = Invocation & { leaving?: boolean };
interface Departure { child: Invocation; at: number; until?: number }
interface Presence { scope: string; children: Invocation[]; gone: Departure[]; ms: number }

/** Stable slots preserve disclosure, selection and focus while sibling tiles finish. */
export function withDeparting(children: Invocation[], gone: Departure[]): Shown[] {
  const result: Shown[] = [...children];
  for (const { child, at } of [...gone].sort((a, b) => a.at - b.at)) {
    if (!result.some((c) => c.id === child.id)) result.splice(Math.min(at, result.length), 0, { ...child, leaving: true });
  }
  return result;
}

/** Scope replacement and revival revoke departures before any old timeout can remove a tile. */
export function retainDepartures(before: Presence, scope: string, children: Invocation[], ms: number, now: number): Presence {
  if (before.scope !== scope || !ms) return { scope, children, gone: [], ms };
  const ids = new Set(children.map((child) => child.id));
  const prior = withDeparting(before.children, before.gone);
  const gone = before.gone.filter(({ child }) => !ids.has(child.id)).map((entry) => ({ ...entry, at: prior.findIndex((child) => child.id === entry.child.id) }));
  const left = prior.map((child, at) => ({ child, at, until: now + ms })).filter(({ child }) => !child.leaving && !ids.has(child.id));
  return { scope, children, gone: [...gone, ...left], ms };
}

/** Retiring an earlier slot rebases later departures so remaining tiles cannot swap places. */
export function retireDepartures(before: Presence, now: number): Presence {
  const gone = before.gone.filter((entry) => entry.until! > now);
  const retired = new Set(before.gone.filter((entry) => entry.until! <= now).map(({ child }) => child.id));
  const order = withDeparting(before.children, before.gone).filter((child) => !retired.has(child.id));
  return { ...before, gone: gone.map((entry) => ({ ...entry, at: order.findIndex((child) => child.id === entry.child.id) })) };
}

/** Reconciliation retains a departed child before commit; one bounded timer only retires it. */
export function useDeparting(children: Invocation[], scope: string, ms = PANE_CLOSE_MS): Shown[] {
  const [saved, setSaved] = useState<Presence>(() => ({ scope, children, gone: [], ms }));
  let current = saved;
  if (saved.scope !== scope || saved.ms !== ms || saved.children.length !== children.length || children.some((child, index) => child !== saved.children[index])) {
    current = retainDepartures(saved, scope, children, ms, Date.now());
    setSaved(current);
  }
  useEffect(() => {
    if (!current.gone.length) return;
    const deadline = Math.min(...current.gone.map((entry) => entry.until!));
    const timer = setTimeout(() => setSaved((before) => retireDepartures(before, Date.now())), Math.max(0, deadline - Date.now()));
    return () => clearTimeout(timer);
  }, [current]);
  return withDeparting(children, current.gone);
}
