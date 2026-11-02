// Deferred native operations verify applied-state truth, coalescing, restoration, and recovery.
import { expect, it, vi } from "vitest";
import { createWindowZoom } from "./window-zoom";
function fixture() {
  const operations: { level: number; resolve(): void; reject(): void }[] = [];
  const persist = vi.fn(), resized = vi.fn();
  const controller = createWindowZoom({ range: { min: .5, max: 2, base: 1, step: .1 }, persist, resized, apply: (level) => new Promise<void>((resolve, reject) => operations.push({ level, resolve, reject: () => reject(Error("native failure")) })) });
  return { controller, operations, persist, resized };
}
it("publishes and saves only native-applied zoom while coalescing rapid shortcuts", async () => {
  const f = fixture(); const action = f.controller.change(1); await Promise.resolve();
  f.controller.change(1); f.controller.change(1);
  expect(f.operations.map((o) => o.level)).toEqual([1.1]);
  expect(f.controller.snapshot()).toMatchObject({ level: 1, applying: true }); expect(f.persist).not.toHaveBeenCalled();
  f.operations[0].resolve(); await Promise.resolve(); await Promise.resolve();
  expect(f.operations.map((o) => o.level)).toEqual([1.1, 1.3]);
  f.operations[1].resolve(); await action;
  expect(f.controller.snapshot()).toMatchObject({ level: 1.3, applying: false }); expect(f.persist).toHaveBeenLastCalledWith(1.3);
});
it("failure preserves the last confirmed value, and retry applies the actual failed target", async () => {
  const f = fixture(); const action = f.controller.change(-1); await Promise.resolve(); f.operations[0].reject();
  expect(await action).toBe(false); expect(f.persist).not.toHaveBeenCalled();
  expect(f.controller.snapshot()).toMatchObject({ level: 1, retry: .9, applying: false });
  const retry = f.controller.retry(); await Promise.resolve(); f.operations[1].resolve(); await retry;
  expect(f.controller.snapshot()).toMatchObject({ level: .9, error: undefined });
});
it("startup cannot overwrite a subsequent request, and reset is bounded to 100%", async () => {
  const f = fixture(); const restore = f.controller.restore(1.5); await Promise.resolve(); f.controller.change(1);
  f.operations[0].resolve(); await Promise.resolve(); await Promise.resolve(); expect(f.operations[1].level).toBe(1.6);
  f.operations[1].resolve(); await restore;
  await f.controller.restore(.5); expect(f.controller.snapshot().level).toBe(1.6);
  const reset = f.controller.change(0); await Promise.resolve(); f.operations[2].resolve(); await reset;
  expect(f.controller.snapshot().level).toBe(1);
});
it("invalid restoration and endpoint requests cannot produce non-finite or out-of-range native targets", async () => {
  const f = fixture(); await f.controller.restore(Infinity); expect(f.operations).toEqual([]);
  const restore = f.controller.restore(100); await Promise.resolve(); expect(f.operations[0].level).toBe(2); f.operations[0].resolve(); await restore;
  await f.controller.change(1); expect(f.operations).toHaveLength(1);
});
