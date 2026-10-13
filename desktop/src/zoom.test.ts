import { describe, expect, it, vi } from "vitest";
vi.mock("@tauri-apps/api/webview", () => ({ getCurrentWebview: () => ({ setZoom: vi.fn(() => Promise.resolve()) }) }));
import { inTerminal, next, onTerminalFont, RANGES, zoom } from "./zoom";

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
