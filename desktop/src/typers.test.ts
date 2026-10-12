import { afterEach, expect, it, vi } from "vitest";
import { exitTerminal, register, terminalConnected } from "./typers";

afterEach(() => vi.useRealTimers());
it("waits for terminal exit after the CLI's normal double-interrupt gesture", async () => {
  vi.useFakeTimers();
  const write = vi.fn();
  const unregister = register("handoff", write);
  const finished = vi.fn();
  const wait = exitTerminal("handoff").then(finished);
  expect(write.mock.calls).toEqual([["\x03"]]);
  await vi.advanceTimersByTimeAsync(200);
  expect(write.mock.calls).toEqual([["\x03"], ["\x03"]]);
  expect(finished).not.toHaveBeenCalled();
  unregister(); await wait;
  expect(finished).toHaveBeenCalledOnce();
  expect(terminalConnected("handoff")).toBe(false);
  expect(vi.getTimerCount()).toBe(0);
});
it("does not send another interrupt if the first one exits", async () => {
  vi.useFakeTimers();
  const write = vi.fn();
  const unregister = register("first-exit", write);
  const wait = exitTerminal("first-exit");
  unregister(); await wait;
  await vi.advanceTimersByTimeAsync(500);
  expect(write).toHaveBeenCalledOnce();
});
it("times out without disconnecting or force-killing a terminal", async () => {
  vi.useFakeTimers();
  const unregister = register("timeout", vi.fn());
  const wait = expect(exitTerminal("timeout")).rejects.toThrow("did not exit");
  await vi.advanceTimersByTimeAsync(10_000); await wait;
  expect(terminalConnected("timeout")).toBe(true);
  unregister();
});
it("already exited terminals need no shutdown", async () => {
  await expect(exitTerminal("not-connected")).resolves.toBeUndefined();
});
