import { beforeEach, describe, expect, it, vi } from "vitest";

beforeEach(() => {
  vi.resetModules();
  const saved = new Map<string, string>();
  vi.stubGlobal("localStorage", { getItem: (k: string) => saved.get(k) ?? null, setItem: (k: string, v: string) => saved.set(k, v) });
  // No DOM in these tests: the window's appearance needs a media query and a root element.
  vi.stubGlobal("window", { matchMedia: () => ({ matches: false, addEventListener: vi.fn() }), addEventListener: vi.fn() });
  vi.stubGlobal("document", { documentElement: { dataset: {}, style: {} } });
});

describe("terminal colours", () => {
  it("follow the window until a choice is made, and the choice is kept and announced", async () => {
    const appearance = await import("./appearance");
    const t = await import("./terminal-theme");
    expect(t.terminalTheme()).toBe("window");
    appearance.setAppearance("dark");
    expect(t.terminalDark()).toBe(true);
    const heard = vi.fn();
    t.onTerminalTheme(heard);
    t.setTerminalTheme("light");
    expect(t.terminalDark()).toBe(false);
    expect(heard).toHaveBeenCalled();
    expect(localStorage.getItem("keel:terminal-theme")).toBe("light");
  });
});
