// Polling remains single-flight, fair, cancellable and isolated from the parent input channel.
import { afterEach, beforeEach, expect, it, vi } from "vitest";
vi.mock("../store", () => ({ useStore: { getState: () => ({}) } }));
vi.mock("../streams", () => ({ open: vi.fn() }));
import { observeGridOutput, outputBatch } from "../subagent-output";
import { paneOf } from "../subagents";
const ep = { port: 1234, token: "synthetic" }, source = { parent: "parent", agent: "codex" };
const detail = { items: [{ kind: "text", text: "Live" }], availability: "available", truncated: false };
beforeEach(() => vi.useFakeTimers());
afterEach(() => vi.useRealTimers());
it("reads four children together and rotates fairly beyond the eight-child batch", async () => {
  const request = vi.fn<typeof fetch>(async () => new Response(JSON.stringify({ one: detail, two: detail, three: detail, four: detail })));
  const stop = observeGridOutput("four", ["one", "two", "three", "four"], ep, source, () => true, request);
  await vi.advanceTimersByTimeAsync(0);
  expect(JSON.parse(String(request.mock.calls[0][1]?.body)).invocations).toHaveLength(4);
  expect(Object.keys(paneOf("four").outputs)).toHaveLength(4);
  const ids = Array.from({ length: 12 }, (_, n) => String(n));
  expect(new Set([...outputBatch(ids, 0), ...outputBatch(ids, 8)]).size).toBe(12);
  stop();
});
it("cleanup aborts the in-flight request and rejects its late response", async () => {
  let reply!: (value: Response) => void;
  const request = vi.fn<typeof fetch>(() => new Promise((resolve) => { reply = resolve; }));
  const stop = observeGridOutput("late-grid", ["one"], ep, source, () => true, request);
  await vi.advanceTimersByTimeAsync(5000);
  expect(request).toHaveBeenCalledOnce();
  stop();
  expect(request.mock.calls[0][1]?.signal?.aborted).toBe(true);
  reply(new Response(JSON.stringify({ one: detail })));
  await vi.advanceTimersByTimeAsync(0);
  expect(paneOf("late-grid").outputs.one).toBeUndefined();
  await vi.advanceTimersByTimeAsync(20_000); expect(request).toHaveBeenCalledOnce();
});
it("endpoint replacement rejects pending output and cannot schedule another read", async () => {
  let reply!: (value: Response) => void, current = true;
  const request = vi.fn<typeof fetch>(() => new Promise((resolve) => { reply = resolve; }));
  const stop = observeGridOutput("scope-grid", ["one"], ep, source, () => current, request);
  current = false; reply(new Response(JSON.stringify({ one: detail })));
  await vi.advanceTimersByTimeAsync(20_000);
  expect(paneOf("scope-grid").outputs.one).toBeUndefined(); expect(request).toHaveBeenCalledOnce(); stop();
});
