// Pointer ownership tests exercise cleanup without substituting a browser for native UI QA.
import { afterEach, beforeEach, expect, it, vi } from "vitest";
import { startCardResize } from "./git-card-resize";
import { cardLayout } from "./git-card-layout";
let windowEvents: EventTarget, handle: HTMLElement;
let frames: Map<number, FrameRequestCallback>;
let body: { userSelect: string; cursor: string };
const preview = vi.fn(), commit = vi.fn();
function pointer(type: string, y: number) { windowEvents.dispatchEvent(Object.assign(new Event(type, { cancelable: true }), { pointerId: 1, clientY: y })); }
function start() {
  return startCardResize({ button: 0, isPrimary: true, pointerId: 1, clientY: 594, currentTarget: handle, preventDefault: vi.fn() }, { region: () => ({ top: 0 }), layout: () => cardLayout(1000, .6, 34, 34, true, true), preview, commit });
}
beforeEach(() => {
  vi.clearAllMocks(); frames = new Map(); let number = 0; windowEvents = new EventTarget(); body = { userSelect: "text", cursor: "auto" };
  let captured = false;
  handle = Object.assign(new EventTarget(), { setPointerCapture: () => { captured = true; }, hasPointerCapture: () => captured, releasePointerCapture: () => { captured = false; handle.dispatchEvent(new Event("lostpointercapture")); } }) as unknown as HTMLElement;
  vi.stubGlobal("window", windowEvents); vi.stubGlobal("document", { body: { style: body } });
  vi.stubGlobal("requestAnimationFrame", (callback: FrameRequestCallback) => { frames.set(++number, callback); return number; });
  vi.stubGlobal("cancelAnimationFrame", (id: number) => frames.delete(id));
});
afterEach(() => vi.unstubAllGlobals());
it.each(["pointerup", "pointercancel", "lostpointercapture", "unmount", "blur"])("%s commits the last valid split and releases every gesture resource", (reason) => {
  const cleanup = start(); pointer("pointermove", 700); pointer("pointermove", 720);
  expect(frames.size).toBe(1); expect(body.userSelect).toBe("none");
  if (reason === "unmount") cleanup();
  else if (reason === "lostpointercapture") handle.dispatchEvent(new Event(reason));
  else if (reason === "blur") windowEvents.dispatchEvent(new Event(reason));
  else pointer(reason, 720);
  expect(commit).toHaveBeenCalledOnce(); expect(commit.mock.calls[0][0]).toBeCloseTo(720 / 990);
  expect(body).toEqual({ userSelect: "text", cursor: "auto" }); expect(frames.size).toBe(0);
  cleanup(); pointer("pointermove", 800); pointer("pointerup", 800);
  expect(commit).toHaveBeenCalledOnce(); expect(preview).toHaveBeenLastCalledWith(undefined);
});
