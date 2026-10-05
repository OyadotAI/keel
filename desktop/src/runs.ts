// Long CLI actions the person started from a panel — a pull request, a check, a plugin install —
// kept here rather than in the panel's state. A panel unmounts when its tab is switched; the run
// does not, and coming back to an idle button while `gh pr create` is still pushing invited a
// second click and a second pull request.

import { useSyncExternalStore } from "react";
import type { Arrived } from "./stream.worker";
import { open, type Stream } from "./streams";

interface Entry {
  state: unknown;
  running: boolean;
  stream?: Stream;
}

const runs = new Map<string, Entry>();
const listeners = new Set<() => void>();
const changed = () => listeners.forEach((l) => l());

/// A run's state and whether it is still going; `undefined` when nothing ran under this key.
export function useRun<T>(key: string): { state: T; running: boolean } | undefined {
  return useSyncExternalStore(
    (l) => (listeners.add(l), () => listeners.delete(l)),
    () => runs.get(key) as { state: T; running: boolean } | undefined,
  );
}

/// Start a run under `key`, unless one is already going there — the second click is a no-op,
/// not a second push. `step` folds each event into the state; `end` says how a stream that
/// stopped looks, given whether it reported its own ending.
export function start<T>(key: string, url: string, token: string, initial: T, step: (s: T, e: Arrived) => T, end: (s: T, error?: string) => T): boolean {
  if (runs.get(key)?.running) return false;
  const entry: Entry = { state: initial, running: true };
  runs.set(key, entry);
  const put = (state: T, running: boolean) => {
    // Replaced, not mutated, so `useSyncExternalStore` sees a new snapshot.
    runs.set(key, { state, running, stream: entry.stream });
    changed();
  };
  entry.stream = open(
    url,
    token,
    (events) => put(events.reduce(step, runs.get(key)!.state as T), true),
    (error) => put(end(runs.get(key)!.state as T, error), false),
  );
  changed();
  return true;
}

/// Forget a finished run, so its panel shows the form again.
export function clear(key: string) {
  if (runs.get(key)?.running) return;
  runs.delete(key);
  changed();
}

/// Lines of output, the common shape: kept to the last `max`.
export const lines = (max: number) => (s: string[], e: Arrived) => (e.event === "line" || e.event === "fatal" ? [...s, String(e.data)].slice(-max) : s);
