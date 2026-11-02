// Window geometry follows native acknowledgements while terminal text keeps its separate range.
import { afterEach, describe, expect, it, vi } from "vitest";
const native = vi.hoisted(() => ({ setZoom: vi.fn(() => Promise.resolve()) }));
vi.mock("@tauri-apps/api/webview", () => ({ getCurrentWebview: () => native }));
import { inTerminal, next, onTerminalFont, RANGES, zoom } from "./zoom";
afterEach(() => { vi.unstubAllGlobals(); vi.resetModules(); native.setZoom.mockReset().mockResolvedValue(); });

describe("zoom", () => {
  it("steps, clamps and resets", () => {
    expect(next(RANGES.FONT, 13, 1)).toBe(14);
    expect(next(RANGES.FONT, 28, 1)).toBe(28);
    expect(next(RANGES.FONT, 9, -1)).toBe(9);
    expect(next(RANGES.APP, 1.1, 1)).toBe(1.2);
    expect(next(RANGES.APP, 0.5, -1)).toBe(0.5);
    expect(next(RANGES.FONT, 20, 0)).toBe(13);
  });

  it("sizes the terminal font when a terminal has focus, and only then", () => {
    // No DOM here: an element is what `closest` says about it.
    const el = (inXterm: boolean) => ({ closest: (sel: string) => (inXterm && sel === ".xterm" ? {} : null) }) as unknown as Element;
    const inside = el(true);
    const outside = el(false);
    expect(inTerminal(inside)).toBe(true);
    expect(inTerminal(outside)).toBe(false);

    const sizes: number[] = [];
    const off = onTerminalFont((s) => sizes.push(s));
    zoom(1, inside);
    zoom(1, outside);
    zoom(0, inside);
    off();
    expect(sizes).toEqual([14, 13]);
  });
});

it("restores saved zoom only after native success and uses applied coordinates for child webviews", async () => {
  const saved = new Map([["keel.zoom", "1.5"]]);
  vi.stubGlobal("localStorage", { getItem: (key: string) => saved.get(key), setItem: (key: string, value: string) => saved.set(key, value) });
  vi.stubGlobal("window", { innerHeight: 500, dispatchEvent: vi.fn() });
  let finish!: () => void;
  native.setZoom.mockImplementationOnce(() => new Promise<void>((resolve) => { finish = resolve; }));
  const api = await import("./zoom"); api.restoreZoom(); await Promise.resolve();
  expect(api.appZoom()).toBe(1);
  const rectangle = { left: 10, top: 20, width: 100, height: 50 } as DOMRect;
  expect(api.toWindow(rectangle).width).toBe(100);
  finish(); await vi.waitFor(() => expect(api.appZoom()).toBe(1.5));
  expect(api.toWindow(rectangle)).toMatchObject({ x: 15, width: 150, viewport: 750 });
  const calls = native.setZoom.mock.calls.length;
  api.zoom(1, { closest: () => ({}) } as unknown as Element);
  expect(native.setZoom.mock.calls.length).toBe(calls); expect(api.appZoom()).toBe(1.5);
  await api.zoomWindow(-1); expect(api.appZoom()).toBe(1.4);
});

it.each(["NaN", "Infinity", "bad", ""])("invalid saved window zoom %s cannot reach the native API", async (value) => {
  vi.stubGlobal("localStorage", { getItem: () => value, setItem: vi.fn() });
  const api = await import("./zoom"); api.restoreZoom(); await Promise.resolve(); await Promise.resolve();
  expect(api.appZoom()).toBe(1); expect(native.setZoom).not.toHaveBeenCalled();
});
