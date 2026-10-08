import { expect, it, vi } from "vitest";
import { task } from "./runs";
vi.mock("./streams", () => ({ open: vi.fn() }));
it("locks a multi-step action immediately and releases it after completion", async () => {
  let finish!: () => void;
  const work = vi.fn(() => new Promise<void>((resolve) => { finish = resolve; }));
  const failure = vi.fn(() => "failed");
  const first = task("setup:project", "Connecting…", work, failure);
  await task("setup:project", "Connecting…", work, failure);
  expect(work).toHaveBeenCalledTimes(1);
  finish(); await first;
  await task("setup:project", "Connecting…", async () => {}, failure);
  expect(failure).not.toHaveBeenCalled();
});
it("reports a failed task and allows retry", async () => {
  const failure = vi.fn(() => "Readable error");
  await task("setup:failed", "Starting…", async () => { throw new Error("offline"); }, failure);
  expect(failure).toHaveBeenCalledWith(expect.objectContaining({ message: "offline" }));
  const retry = vi.fn(async () => {});
  await task("setup:failed", "Starting…", retry, failure);
  expect(retry).toHaveBeenCalledOnce();
});
