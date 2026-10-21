import { describe, expect, it, vi } from "vitest";

const posted: unknown[] = [];
vi.mock("./api", () => ({ post: vi.fn(async (_ep: unknown, _path: string, body: unknown) => void posted.push(body)) }));
const invoke = vi.fn();
vi.mock("@tauri-apps/api/core", () => ({ invoke: (...a: unknown[]) => invoke(...a) }));
vi.mock("@tauri-apps/api/event", () => ({ listen: vi.fn() }));

import { activity, address, onBrowser, shownTool } from "./browser";

const ep = { port: 1, token: "t" };

describe("the lane's Browser tab", () => {
  it("says what the agent does, and never what it typed", () => {
    const said = activity({ op: "type", target: 3, text: "hunter2", submit: true });
    expect(said).toBe("typing 7 characters into element 3, then submitting");
    expect(said).not.toContain("hunter2");
    expect(activity({ op: "open", url: "https://example.com" })).toBe("opening example.com");
  });

  it("answers at once for a lane this window does not have", async () => {
    posted.length = 0;
    await onBrowser("/p", ep, () => false, { id: "b-1", lane: "gone", op: { op: "read" } });
    expect(invoke).not.toHaveBeenCalled();
    expect(posted).toEqual([{ id: "b-1", ok: false, text: "That lane isn't open in Keel, so it has no Browser tab." }]);
  });

  it("runs the op in the lane's webview and posts what it said, or why not", async () => {
    posted.length = 0;
    invoke.mockResolvedValueOnce("# Example");
    await onBrowser("/p", ep, () => true, { id: "b-2", lane: "l", op: { op: "read" } });
    expect(invoke).toHaveBeenCalledWith("browser_act", { lane: "l", project: "/p", op: { op: "read" } });
    invoke.mockRejectedValueOnce("No element 9 on the page now.");
    await onBrowser("/p", ep, () => true, { id: "b-3", lane: "l", op: { op: "click", target: 9 } });
    expect(posted).toEqual([
      { id: "b-2", ok: true, text: "# Example" },
      { id: "b-3", ok: false, text: "No element 9 on the page now." },
    ]);
  });

  it("shows the tab when a run starts or a page opens, not on every action", async () => {
    const shown: string[] = [];
    invoke.mockResolvedValue("ok");
    // The first action of a run shows the tab; the rest of the run does not pull the person back.
    await onBrowser("/p", ep, () => true, { id: "b-4", lane: "fresh", op: { op: "read" } }, (l) => shown.push(l));
    await onBrowser("/p", ep, () => true, { id: "b-5", lane: "fresh", op: { op: "scroll", direction: "down" } }, (l) => shown.push(l));
    await onBrowser("/p", ep, () => true, { id: "b-6", lane: "fresh", op: { op: "open", url: "https://example.com" } }, (l) => shown.push(l));
    expect(shown).toEqual(["fresh", "fresh"]);
  });

  it("names its calls as what they are, and takes an address as typed", () => {
    expect(shownTool("mcp__keel__browser_click")).toBe("Browser · click");
    expect(shownTool("Bash")).toBe("Bash");
    expect(address("example.com/a")).toBe("https://example.com/a");
    expect(address(" http://localhost:3000 ")).toBe("http://localhost:3000");
    expect(address("localhost:3000/a")).toBe("http://localhost:3000/a");
    expect(address("127.0.0.1:5173")).toBe("http://127.0.0.1:5173");
    expect(address("localhost.example.com")).toBe("https://localhost.example.com");
  });
});
