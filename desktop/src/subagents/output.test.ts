// Output connection lifetimes cannot leak old selections or parent identities into another pane.
import { afterEach, beforeEach, expect, it, vi } from "vitest";
const api = vi.hoisted(() => ({ open: vi.fn() }));
vi.mock("../streams", () => ({ open: api.open }));
vi.mock("../store", () => ({ useStore: { getState: () => ({}) } }));
import { observeChildOutput } from "../subagent-output";
import { paneOf } from "../subagents";
const ep = { port: 1234, token: "synthetic" };
const detail = { items: [{ kind: "text", text: "Progress" }], availability: "available", truncated: false };
beforeEach(() => { vi.useFakeTimers(); api.open.mockReset(); api.open.mockReturnValue({ close: vi.fn() }); });
afterEach(() => { vi.useRealTimers(); });
it("collapse closes only the observation stream and late batches cannot update cached output", () => {
  const close = observeChildOutput("collapse", "one", ep, "parent", "codex", "checkout", () => true);
  const [request, , events] = api.open.mock.calls[0];
  expect(request).toContain("parent=parent"); expect(request).toContain("invocation=one");
  events([{ event: "output", data: detail }]); expect(paneOf("collapse").outputs.one.detail?.items[0].text).toBe("Progress");
  close(); expect(api.open.mock.results[0].value.close).toHaveBeenCalledOnce();
  events([{ event: "output", data: { ...detail, items: [] } }]); expect(paneOf("collapse").outputs.one.detail?.items).toHaveLength(1);
});
it("replaced conversation identity rejects callbacks and prevents reconnecting", () => {
  let current = true;
  const close = observeChildOutput("replaced", "one", ep, "parent", "claude", undefined, () => current);
  current = false; api.open.mock.calls[0][2]([{ event: "output", data: detail }]); api.open.mock.calls[0][3]("offline");
  vi.runOnlyPendingTimers(); expect(paneOf("replaced").outputs.one).toBeUndefined(); expect(api.open).toHaveBeenCalledOnce(); close();
});
it("disconnected output retains text and retries only while still selected", async () => {
  const close = observeChildOutput("retry", "one", ep, "parent", "antigravity", undefined, () => true);
  api.open.mock.calls[0][2]([{ event: "output", data: detail }]); api.open.mock.calls[0][3]("offline");
  expect(paneOf("retry").outputs.one).toMatchObject({ disconnected: true, detail });
  await vi.advanceTimersByTimeAsync(1000); expect(api.open).toHaveBeenCalledTimes(2);
  close(); await vi.advanceTimersByTimeAsync(15_000); expect(api.open).toHaveBeenCalledTimes(2);
});
