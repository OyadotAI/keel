import { afterEach, expect, it, vi } from "vitest";

afterEach(() => vi.unstubAllGlobals());
it("closing a subscription ignores already queued worker messages without affecting other readers", async () => {
  vi.resetModules();
  class WorkerStub {
    static current: WorkerStub;
    onmessage?: (e: { data: unknown }) => void;
    postMessage = vi.fn();
    constructor() { WorkerStub.current = this; }
  }
  vi.stubGlobal("Worker", WorkerStub);
  const { open } = await import("./streams");
  const receive = vi.fn(), ended = vi.fn(), other = vi.fn();
  const first = open("/one", "token", receive, ended);
  open("/two", "token", other, ended);
  const [firstMessage, secondMessage] = WorkerStub.current.postMessage.mock.calls;
  first.close();
  WorkerStub.current.onmessage?.({ data: { id: firstMessage[0].id, events: [{ event: "turn", data: [] }], end: {} } });
  WorkerStub.current.onmessage?.({ data: { id: secondMessage[0].id, events: [{ event: "turn", data: [] }] } });
  expect(receive).not.toHaveBeenCalled(); expect(ended).not.toHaveBeenCalled(); expect(other).toHaveBeenCalledOnce();
});
