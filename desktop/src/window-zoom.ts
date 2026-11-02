// Window zoom has one native operation in flight and reports only successfully applied values.
export interface ZoomSnapshot { level: number; applying: boolean; error?: string; retry?: number }
interface Range { min: number; max: number; step: number; base: number }
interface Dependencies { range: Range; apply(level: number): Promise<void>; persist(level: number): void; resized(): void }

/** Coalesce rapid keyboard requests while keeping the visible percentage and native geometry truthful. */
export function createWindowZoom(deps: Dependencies) {
  let snapshot: ZoomSnapshot = { level: deps.range.base, applying: false };
  let desired = snapshot.level;
  let inflight: Promise<boolean> | undefined;
  let userRequested = false;
  const listeners = new Set<() => void>();
  const clamp = (value: number) => Math.round(Math.min(deps.range.max, Math.max(deps.range.min, value)) * 100) / 100;
  function publish(patch: Partial<ZoomSnapshot>) { snapshot = { ...snapshot, ...patch }; listeners.forEach((listener) => listener()); }
  async function drain() {
    while (desired !== snapshot.level) {
      const target = desired;
      try { await deps.apply(target); }
      catch { desired = snapshot.level; publish({ applying: false, error: "Couldn’t change window zoom.", retry: target }); return false; }
      deps.persist(target); publish({ level: target, error: undefined, retry: undefined }); deps.resized();
    }
    publish({ applying: false }); return true;
  }
  function apply(target: number): Promise<boolean> {
    desired = clamp(target);
    if (inflight) return inflight;
    publish({ applying: true, error: undefined, retry: undefined });
    inflight = Promise.resolve().then(drain).finally(() => { inflight = undefined; });
    return inflight;
  }
  return {
    snapshot: () => snapshot,
    subscribe(listener: () => void) { listeners.add(listener); return () => { listeners.delete(listener); }; },
    change(direction: -1 | 0 | 1) { userRequested = true; return apply(direction ? desired + direction * deps.range.step : deps.range.base); },
    restore(target: number) { return userRequested ? Promise.resolve(true) : apply(Number.isFinite(target) ? target : deps.range.base); },
    retry() { userRequested = true; return apply(snapshot.retry ?? snapshot.level); },
    dismiss() { publish({ error: undefined, retry: undefined }); },
  };
}
