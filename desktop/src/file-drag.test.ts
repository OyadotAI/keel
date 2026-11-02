// Synthetic pointer contracts cover the real controller without claiming native WebView2 QA.
import { afterEach, beforeEach, expect, it, vi } from "vitest";
const fixture = vi.hoisted(() => ({ state: { lanes: { source: { project: "/repo", wt: "branch" } } as Record<string, { project: string; wt?: string }>, referenceFile: vi.fn() }, listeners: new Set<(state: unknown) => void>() }));
vi.mock("./store", () => ({ useStore: { getState: () => fixture.state, subscribe: (listener: (state: unknown) => void) => { fixture.listeners.add(listener); return () => fixture.listeners.delete(listener); } } }));
import { cancelFileDrag, draggedClick, startFileDrag, useFileDrag } from "./file-drag";

let events: EventTarget;
let element: HTMLElement;
let dropTarget: string | undefined;
function pointer(type: string, x: number, y = 0) {
  const event = Object.assign(new Event(type, { cancelable: true }), { clientX: x, clientY: y, pointerId: 1 });
  events.dispatchEvent(event);
}
function begin() { startFileDrag("source", "src/日本語 file.ts", { currentTarget: element, button: 0, isPrimary: true, pointerId: 1, clientX: 0, clientY: 0 }); }

beforeEach(() => {
  fixture.state.lanes = { source: { project: "/repo", wt: "branch" } }; fixture.state.referenceFile.mockReset();
  events = new EventTarget(); dropTarget = "destination";
  element = Object.assign(new EventTarget(), { setPointerCapture: vi.fn(), hasPointerCapture: () => true, releasePointerCapture: vi.fn() }) as unknown as HTMLElement;
  vi.stubGlobal("window", events);
  vi.stubGlobal("document", { elementFromPoint: () => dropTarget ? { closest: () => ({ dataset: { referenceTarget: dropTarget } }) } : null });
});
afterEach(() => { cancelFileDrag(); vi.unstubAllGlobals(); });

it("ordinary clicks and motion below the threshold still open files normally", () => {
  begin(); pointer("pointermove", 2); pointer("pointerup", 2);
  expect(useFileDrag.getState().drag).toBeUndefined();
  expect(draggedClick(element)).toBe(false);
  expect(fixture.state.referenceFile).not.toHaveBeenCalled();
});

it("a completed internal drag carries immutable checkout identity and suppresses only its pointer click", () => {
  begin(); pointer("pointermove", 15, 20);
  expect(useFileDrag.getState().drag).toMatchObject({ path: "src/日本語 file.ts", target: "destination" });
  pointer("pointerup", 15, 20);
  expect(fixture.state.referenceFile).toHaveBeenCalledWith("destination", { project: "/repo", wt: "branch", path: "src/日本語 file.ts" });
  expect(draggedClick(element, 0)).toBe(false);
  expect(draggedClick(element)).toBe(true);
  expect(draggedClick(element)).toBe(false);
});

it.each(["Escape", "pointercancel", "blur"])("%s cancels without a drop or accidental file-opening click", (kind) => {
  begin(); pointer("pointermove", 20);
  events.dispatchEvent(Object.assign(new Event(kind === "Escape" ? "keydown" : kind), { key: kind }));
  pointer("pointerup", 20);
  expect(useFileDrag.getState().drag).toBeUndefined();
  expect(fixture.state.referenceFile).not.toHaveBeenCalled();
  expect(draggedClick(element)).toBe(true);
  begin(); pointer("pointerup", 0);
  expect(draggedClick(element)).toBe(false);
});

it("a terminal or header has no drop target", () => {
  dropTarget = undefined; begin(); pointer("pointermove", 20); pointer("pointerup", 20);
  expect(fixture.state.referenceFile).not.toHaveBeenCalled();
});

it("removing or retargeting the source cancels the gesture", () => {
  begin(); pointer("pointermove", 20);
  fixture.state.lanes.source.wt = "changed";
  for (const listener of fixture.listeners) listener(fixture.state);
  pointer("pointerup", 20);
  expect(fixture.state.referenceFile).not.toHaveBeenCalled();
  expect(useFileDrag.getState().drag).toBeUndefined();
});
