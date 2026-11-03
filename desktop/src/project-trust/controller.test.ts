// Consent tests exercise public controller operations with delayed, failing and closed-project seams.
import { expect, it, vi } from "vitest";
import { createTrustController, type TrustRequest } from "./index";

const endpoint = { port: 12345, token: "synthetic" };
function deferred<T>() {
  let resolve!: (value: T) => void;
  let reject!: (reason: Error) => void;
  const promise = new Promise<T>((yes, no) => { resolve = yes; reject = no; });
  return { promise, resolve, reject };
}
function fixture() {
  let request: TrustRequest | null = null;
  const deps = {
    read: () => request,
    put: (value: TrustRequest | null) => { request = value; },
    exists: vi.fn(() => true),
    ensure: vi.fn(async (_project: string) => endpoint),
    grant: vi.fn(async (_ep: typeof endpoint): Promise<unknown> => true),
    confirmed: vi.fn(),
  };
  return { deps, trust: createTrustController(deps) };
}

it("opening and cancelling prepare a connection but never write trust", async () => {
  const { deps, trust } = fixture();
  trust.open("/original");
  expect(deps.read()?.phase).toBe("connecting");
  await Promise.resolve();
  expect(deps.read()?.phase).toBe("ready");
  expect(deps.grant).not.toHaveBeenCalled();
  trust.dismiss();
  await trust.confirm();
  expect(deps.read()).toBeNull();
  expect(deps.grant).not.toHaveBeenCalled();
});

it("cannot confirm until the captured project's connection is ready", async () => {
  const { deps, trust } = fixture();
  const connection = deferred<typeof endpoint>();
  deps.ensure.mockReturnValue(connection.promise);
  trust.open("/original");
  await trust.confirm();
  expect(deps.grant).not.toHaveBeenCalled();
  trust.dismiss();
  connection.resolve(endpoint);
  await Promise.resolve();
  expect(deps.read()).toBeNull();
});

it("only the most recently opened decision can receive a late connection result", async () => {
  const { deps, trust } = fixture();
  const old = deferred<typeof endpoint>();
  deps.ensure.mockReturnValueOnce(old.promise);
  trust.open("/old");
  trust.open("/new");
  await Promise.resolve();
  old.resolve({ ...endpoint, port: 12346 });
  await Promise.resolve();
  expect(deps.read()).toMatchObject({ project: "/new", endpoint });
});

it("submits once and keeps a pending decision visible until the write resolves", async () => {
  const { deps, trust } = fixture();
  const saved = deferred<unknown>();
  deps.grant.mockReturnValue(saved.promise);
  trust.open("/original");
  await Promise.resolve();
  const first = trust.confirm();
  await trust.confirm();
  trust.dismiss();
  trust.open("/other");
  expect(deps.read()).toMatchObject({ project: "/original", phase: "saving" });
  expect(deps.grant).toHaveBeenCalledExactlyOnceWith(endpoint);
  saved.resolve(true);
  await first;
  expect(deps.confirmed).toHaveBeenCalledExactlyOnceWith("/original");
  expect(deps.read()).toBeNull();
});

it("keeps a failed write open and permits an explicit retry for the same project", async () => {
  const { deps, trust } = fixture();
  deps.grant.mockRejectedValueOnce(new Error("Disconnected"));
  trust.open("/original");
  await Promise.resolve();
  await trust.confirm();
  expect(deps.read()).toMatchObject({ project: "/original", phase: "ready", error: "Disconnected" });
  expect(deps.confirmed).not.toHaveBeenCalled();
  await trust.confirm();
  expect(deps.confirmed).toHaveBeenCalledExactlyOnceWith("/original");
});

it("retries a failed connection without writing or changing the project target", async () => {
  const { deps, trust } = fixture();
  deps.ensure.mockRejectedValueOnce(new Error("Connection refused"));
  trust.open("/original");
  await Promise.resolve();
  expect(deps.read()).toMatchObject({ phase: "failed", error: "Connection refused" });
  trust.retry();
  await Promise.resolve();
  expect(deps.read()).toMatchObject({ project: "/original", phase: "ready" });
  expect(deps.grant).not.toHaveBeenCalled();
});

it("reacquires the captured project's current daemon when retrying a failed write", async () => {
  const { deps, trust } = fixture();
  deps.grant.mockRejectedValueOnce(new Error("Daemon stopped"));
  trust.open("/original");
  await Promise.resolve();
  await trust.confirm();
  const restarted = { ...endpoint, port: 12346 };
  deps.ensure.mockResolvedValue(restarted);
  await trust.confirm();
  expect(deps.grant.mock.calls.map(([ep]) => ep)).toEqual([endpoint, restarted]);
  expect(deps.ensure.mock.calls.every(([project]) => project === "/original")).toBe(true);
  expect(deps.confirmed).toHaveBeenCalledExactlyOnceWith("/original");
});

it("refuses an unsent confirmation after its project closes", async () => {
  const { deps, trust } = fixture();
  trust.open("/original");
  await Promise.resolve();
  deps.exists.mockReturnValue(false);
  await trust.confirm();
  expect(deps.read()?.phase).toBe("failed");
  expect(deps.read()?.error).toContain("no longer open");
  expect(deps.grant).not.toHaveBeenCalled();
});

it("does not prepare a closed project or accept one closed during connection", async () => {
  const { deps, trust } = fixture();
  deps.exists.mockReturnValue(false);
  trust.open("/closed");
  expect(deps.ensure).not.toHaveBeenCalled();
  deps.exists.mockReturnValue(true);
  const connection = deferred<typeof endpoint>();
  deps.ensure.mockReturnValue(connection.promise);
  trust.open("/original");
  deps.exists.mockReturnValue(false);
  connection.resolve(endpoint);
  await Promise.resolve();
  expect(deps.read()).toMatchObject({ project: "/original", phase: "failed" });
  expect(deps.grant).not.toHaveBeenCalled();
});

it("validates the saved response before claiming that trust was granted", async () => {
  const { deps, trust } = fixture();
  deps.grant.mockResolvedValue(false);
  trust.open("/original");
  await Promise.resolve();
  await trust.confirm();
  expect(deps.read()?.error).toContain("did not confirm");
  expect(deps.confirmed).not.toHaveBeenCalled();
});
