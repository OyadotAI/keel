import { beforeEach, afterEach, describe, expect, it, vi } from "vitest";
import type { Lane } from "./store";

const mocks = vi.hoisted(() => ({ post: vi.fn(), get: vi.fn(), open: vi.fn(), invoke: vi.fn() }));
vi.mock("./api", async (original) => ({ ...await original<typeof import("./api")>(), post: mocks.post, get: mocks.get }));
vi.mock("./streams", () => ({ open: mocks.open }));
vi.mock("./notify", () => ({ notify: vi.fn() }));
vi.mock("@tauri-apps/api/core", () => ({ invoke: mocks.invoke }));
const ep = { port: 12345, token: "fixture" };

async function fixture() {
  const { useStore } = await import("./store");
  const lane: Lane = { id: "test", project: "/repo", title: "Test", interface: "chat", agent: "claude", known: false, isolated: false, jobs: [], queued: [], conv: { turns: [] }, running: false, loaded: true, pending: [], draft: "Hello" };
  useStore.setState({ lanes: { test: lane }, projects: { "/repo": { path: "/repo", name: "Repo", lanes: ["test"], endpoint: ep } }, order: ["/repo"], active: "test", prepare: async () => ({ ep }) });
  return useStore;
}

beforeEach(() => {
  vi.resetModules(); vi.clearAllMocks(); vi.useFakeTimers();
  const saved = new Map<string, string>();
  vi.stubGlobal("localStorage", { getItem: (k: string) => saved.get(k) ?? null, setItem: (k: string, v: string) => saved.set(k, v) });
  mocks.open.mockReturnValue({ close: vi.fn() });
  mocks.get.mockResolvedValue([]);
  mocks.invoke.mockResolvedValue(ep);
});
afterEach(() => { vi.useRealTimers(); vi.unstubAllGlobals(); });

describe("chat delivery", () => {
  it("never drains queued work on historical idle records before replay catches up", async () => {
    const store = await fixture();
    const queued = [{ id: "queued", text: "Next turn", attachments: [] }];
    store.setState((s) => ({ lanes: { test: { ...s.lanes.test, managed: true, queued } } }));
    mocks.post.mockResolvedValue({ accepted: true, running: true });
    store.getState().load("test"); await Promise.resolve();
    const stream = mocks.open.mock.calls.find(([url]) => url.includes("/api/chat/events"))!;
    stream[2]([{ event: "record", data: { seq: 1, event: "idle", data: null } }]);
    await Promise.resolve();
    expect(mocks.post).not.toHaveBeenCalled();
    stream[2]([{ event: "record", data: { seq: 2, event: "accepted", data: { id: "running" } } }, { event: "caught-up", data: { running: true } }]);
    expect(store.getState().lanes.test.queued).toEqual(queued);
    expect(mocks.post).not.toHaveBeenCalled();
    stream[2]([{ event: "record", data: { seq: 3, event: "idle", data: null } }]);
    await Promise.resolve(); await Promise.resolve();
    expect(mocks.post).toHaveBeenCalledOnce();
    expect(store.getState().lanes.test.draft).toBe("Hello");
  });

  it("background and queued sends preserve an unrelated composer draft and attachments", async () => {
    const store = await fixture();
    const attachments = [{ path: "draft.txt", name: "draft.txt" }];
    store.setState((s) => ({ lanes: { test: { ...s.lanes.test, attachments } } }));
    mocks.post.mockResolvedValue({ accepted: true, running: true });
    await store.getState().send("test", "Queued earlier");
    expect(store.getState().lanes.test).toMatchObject({ draft: "Hello", attachments });
  });

  it("editing queued work never overwrites an unsent draft", async () => {
    const store = await fixture();
    const queued = [{ id: "q1", text: "Queued earlier", attachments: [] }];
    store.setState((s) => ({ lanes: { test: { ...s.lanes.test, queued } } }));
    store.getState().editQueued("test", "q1");
    expect(store.getState().lanes.test).toMatchObject({ draft: "Hello", queued });
    expect(store.getState().lanes.test.error).toContain("unsent draft");
    store.getState().draft("test", "");
    store.getState().editQueued("test", "q1");
    expect(store.getState().lanes.test).toMatchObject({ draft: "Queued earlier", queued: [], error: undefined });
  });

  it("persists queued messages across reload and persists their removal", async () => {
    const store = await fixture();
    store.setState((s) => ({ lanes: { test: { ...s.lanes.test, running: true } } }));
    await store.getState().send("test", "Hello");
    const saved = JSON.parse(localStorage.getItem("keel.layout.v1")!);
    expect(saved.lanes[0].queued[0].text).toBe("Hello");
    vi.resetModules();
    const { useStore: restored } = await import("./store");
    expect(restored.getState().lanes.test.queued[0].text).toBe("Hello");
    restored.getState().removeQueued("test", saved.lanes[0].queued[0].id);
    expect(JSON.parse(localStorage.getItem("keel.layout.v1")!).lanes[0].queued).toEqual([]);
  });

  it("a deduplicated retry of a completed turn does not leave the lane busy", async () => {
    const store = await fixture();
    const receipt = { id: "original", prompt: "Hello", mode: "plan" };
    store.setState((s) => ({ lanes: { test: { ...s.lanes.test, receipt, managed: true, cursor: 8 } } }));
    mocks.post.mockResolvedValue({ accepted: false, running: false });
    await store.getState().send("test", "Hello");
    expect(mocks.post.mock.calls[0][2].id).toBe("original");
    expect(store.getState().lanes.test).toMatchObject({ running: false, submitting: false, draft: "", receipt: undefined });
  });

  it("streamed completion stays authoritative when the POST response is lost", async () => {
    const store = await fixture();
    store.getState().load("test");
    await Promise.resolve();
    const stream = mocks.open.mock.calls.find(([url]) => url.includes("/api/chat/events"))!;
    mocks.post.mockImplementation(async (_ep, path, body) => {
      if (path === "/api/chat/send") {
        stream[2]([
          { event: "record", data: { seq: 1, event: "accepted", data: { id: body.id } } },
          { event: "record", data: { seq: 2, event: "idle", data: null } },
        ]);
        throw new Error("Response lost");
      }
    });
    await store.getState().send("test", "Hello");
    expect(store.getState().lanes.test).toMatchObject({ running: false, submitting: false, receipt: undefined, error: undefined, draft: "" });
  });

  it("a late POST response cannot overwrite a newer idle event", async () => {
    const store = await fixture();
    store.getState().load("test"); await Promise.resolve();
    const stream = mocks.open.mock.calls.find(([url]) => url.includes("/api/chat/events"))!;
    mocks.post.mockImplementation(async (_ep, _path, body) => {
      stream[2]([{ event: "record", data: { seq: 1, event: "accepted", data: { id: body.id } } }, { event: "record", data: { seq: 2, event: "idle", data: null } }]);
      return { accepted: true, running: true };
    });
    await store.getState().send("test", "Hello");
    expect(store.getState().lanes.test.running).toBe(false);
  });

  it("active work blocks switching views without changing the selected surface", async () => {
    const store = await fixture();
    store.setState((s) => ({ lanes: { test: { ...s.lanes.test, running: true } } }));
    await store.getState().switchInterface("test", "terminal");
    expect(store.getState().lanes.test.interface).toBe("chat");
    expect(mocks.post).not.toHaveBeenCalled();
    expect(store.getState().lanes.test.error).toContain("Stop the active turn");
  });

  it("closing a lane during preparation cannot start an invisible turn", async () => {
    const store = await fixture();
    store.setState({ prepare: async () => { store.setState({ lanes: {} }); return { ep }; } });
    await store.getState().send("test", "Hello");
    expect(mocks.post).not.toHaveBeenCalled();
    expect(store.getState().lanes).toEqual({});
  });

  it("an idle view switch retains the provider session and unsent draft", async () => {
    const store = await fixture();
    const load = vi.fn();
    store.setState((s) => ({ load, lanes: { test: { ...s.lanes.test, known: true, session: "native-session" } } }));
    mocks.post.mockResolvedValue({ ok: true });
    await store.getState().switchInterface("test", "terminal");
    expect(store.getState().lanes.test).toMatchObject({ interface: "terminal", session: "native-session", known: true, draft: "Hello" });
    expect(mocks.post).toHaveBeenCalledWith(ep, "/api/chat/control", { lane: "test", method: "close" });
    expect(load).toHaveBeenCalledWith("test");
  });

  it("a Codex terminal takes the conversation id the daemon found and can then switch", async () => {
    const store = await fixture();
    store.setState((s) => ({ projects: { "/repo": { ...s.projects["/repo"], endpoint: undefined } } }));
    store.getState().load("test"); await vi.waitFor(() => expect(mocks.open.mock.calls.some(([u]) => u.includes("/api/events"))).toBe(true));
    store.setState((s) => ({ lanes: { test: { ...s.lanes.test, interface: "terminal", agent: "codex", session: "keel-chosen" } } }));
    const events = mocks.open.mock.calls.find(([u]) => u.includes("/api/events"))!;
    events[2]([{ event: "terminal.session", data: { data: { lane: "other", session: "nope" } } }]);
    expect(store.getState().lanes.test).toMatchObject({ session: "keel-chosen", known: false });
    events[2]([{ event: "terminal.session", data: { data: { lane: "test", session: "codex-own" } } }]);
    expect(store.getState().lanes.test).toMatchObject({ session: "codex-own", known: true });
    expect(JSON.parse(localStorage.getItem("keel.layout.v1")!).lanes[0]).toMatchObject({ session: "codex-own", known: true });
    // Followed from that moment, with its agent named, whether or not it is on screen.
    await vi.waitFor(() => expect(mocks.open.mock.calls.some(([u]) => u.includes("/api/session/tail") && u.includes("agent=codex") && u.includes("codex-own"))).toBe(true));
  });

  it("an Antigravity terminal lane is followed with its agent named", async () => {
    const store = await fixture();
    store.setState((s) => ({ lanes: { test: { ...s.lanes.test, interface: "terminal", agent: "antigravity", known: true, session: "agy-1" } } }));
    store.getState().load("test");
    await vi.waitFor(() => expect(mocks.open.mock.calls.some(([u]) => u.includes("/api/session/tail") && u.includes("agent=antigravity") && u.includes("agy-1"))).toBe(true));
  });

  it("a Codex chat lane is not followed; its stream is Keel's own", async () => {
    const store = await fixture();
    store.setState((s) => ({ lanes: { test: { ...s.lanes.test, interface: "chat", agent: "codex", known: true, session: "c" } } }));
    store.getState().load("test"); await Promise.resolve(); await Promise.resolve();
    expect(mocks.open.mock.calls.some(([u]) => u.includes("/api/session/tail"))).toBe(false);
  });

  it("Claude's session list never sets a working Codex lane idle", async () => {
    const store = await fixture();
    store.setState((s) => ({ projects: { "/repo": { ...s.projects["/repo"], endpoint: undefined } } }));
    store.getState().load("test"); await vi.waitFor(() => expect(mocks.open.mock.calls.some(([u]) => u.includes("/api/events"))).toBe(true));
    const queued = [{ id: "q", text: "next", attachments: [] }];
    store.setState((s) => ({ lanes: { test: { ...s.lanes.test, interface: "terminal", agent: "codex", known: true, session: "c", running: true, queued } } }));
    const events = mocks.open.mock.calls.find(([u]) => u.includes("/api/events"))!;
    events[2]([{ event: "sessions", data: { data: [] } }]);
    expect(store.getState().lanes.test).toMatchObject({ running: true, queued });
  });

  it("a Codex terminal without a session id retains its original view and history", async () => {
    const store = await fixture();
    store.setState((s) => ({ lanes: { test: { ...s.lanes.test, interface: "terminal", agent: "codex" } } }));
    await store.getState().switchInterface("test", "chat");
    expect(store.getState().lanes.test.interface).toBe("terminal");
    expect(store.getState().lanes.test.error).toContain("has no conversation yet");
    expect(mocks.post).not.toHaveBeenCalled();
  });
});

describe("plan approval handoff", () => {
  const plan = { id: "plan-write", lane: "test", tool: "ExitPlanMode", command: "", rules: [], input: { plan: "Add the export button" }, session_id: "session" };

  async function planning() {
    const store = await fixture();
    store.setState((s) => ({ lanes: { test: { ...s.lanes.test, mode: "plan", running: true, managed: true, pending: [plan] } } }));
    store.getState().load("test");
    await Promise.resolve();
    const stream = mocks.open.mock.calls.find(([url]) => url.includes("/api/chat/events"))!;
    stream[2]([{ event: "caught-up", data: { running: true } }]);
    return { store, idle: () => stream[2]([{ event: "record", data: { seq: 1, event: "idle", data: null } }]) };
  }

  it("sends the hook sentinel and automatically builds the approved plan after idle", async () => {
    const { store, idle } = await planning();
    mocks.post.mockResolvedValue({ ok: true, accepted: true, running: true });
    await store.getState().answer("test", plan, "allow");
    expect(mocks.post).toHaveBeenCalledWith(ep, "/api/approve/answer", expect.objectContaining({ answer: "keel:plan-approved", rules: [] }));
    expect(store.getState().lanes.test).toMatchObject({ mode: "acceptEdits", draft: "Hello", pending: [] });
    expect(mocks.post).toHaveBeenCalledOnce();
    expect(JSON.parse(localStorage.getItem("keel.layout.v1")!).lanes[0]).toMatchObject({ mode: "acceptEdits", queued: [{ text: expect.stringContaining("Add the export button") }] });
    idle();
    await Promise.resolve(); await Promise.resolve();
    expect(mocks.post).toHaveBeenLastCalledWith(ep, "/api/chat/send", expect.objectContaining({ mode: "acceptEdits", prompt: expect.stringContaining("Implement the plan I approved") }));
    expect(store.getState().lanes.test.draft).toBe("Hello");
  });

  it("holds queued work when idle arrives before the approval response", async () => {
    const { store, idle } = await planning();
    store.setState((s) => ({ lanes: { test: { ...s.lanes.test, queued: [{ id: "later", text: "Later task", attachments: [] }] } } }));
    let accept!: (value: { ok: boolean }) => void;
    mocks.post.mockImplementationOnce(() => new Promise((resolve) => { accept = resolve; }));
    mocks.post.mockResolvedValue({ accepted: true, running: true });
    const answering = store.getState().answer("test", plan, "allow");
    idle();
    expect(mocks.post).toHaveBeenCalledOnce();
    accept({ ok: true });
    await answering; await Promise.resolve(); await Promise.resolve();
    expect(mocks.post).toHaveBeenLastCalledWith(ep, "/api/chat/send", expect.objectContaining({ mode: "acceptEdits", prompt: expect.stringContaining("Implement the plan I approved") }));
    expect(store.getState().lanes.test.queued.map((q) => q.text)).toEqual(["Later task"]);
  });

  it.each(["failed", "expired"])("does not switch modes or schedule implementation when approval is %s", async (failure) => {
    const { store } = await planning();
    if (failure === "failed") mocks.post.mockRejectedValueOnce(new Error("offline"));
    else mocks.post.mockResolvedValueOnce({ ok: false });
    await store.getState().answer("test", plan, "allow");
    expect(store.getState().lanes.test).toMatchObject({ mode: "plan", queued: [], pending: [plan], submitting: false });
    expect(store.getState().lanes.test.error).toContain("Could not approve the plan");
  });

  it("stays in plan mode when the plan is declined", async () => {
    const { store } = await planning();
    mocks.post.mockResolvedValue({ ok: true });
    await store.getState().answer("test", plan, "deny");
    expect(store.getState().lanes.test).toMatchObject({ mode: "plan", queued: [] });
    expect(mocks.post).toHaveBeenCalledWith(ep, "/api/approve/answer", expect.objectContaining({ decision: "deny", answer: "" }));
  });
});

describe("Codex terminal handoff", () => {
  it("waits for a normal exit before reopening the same session in Chat", async () => {
    const store = await fixture();
    const { register } = await import("./typers");
    store.setState((s) => ({ lanes: { test: { ...s.lanes.test, interface: "terminal", agent: "codex", known: true, session: "saved-session" } } }));
    const load = vi.fn(); store.setState({ load });
    const write = vi.fn(); const unregister = register("test", write);
    const switching = store.getState().switchInterface("test", "chat");
    expect(store.getState().lanes.test).toMatchObject({ interface: "terminal", submitting: true });
    expect(load).not.toHaveBeenCalled();
    await vi.advanceTimersByTimeAsync(200);
    unregister(); await switching;
    expect(store.getState().lanes.test).toMatchObject({ interface: "chat", submitting: false, session: "saved-session", known: true, draft: "Hello", error: undefined });
    expect(load).toHaveBeenCalledWith("test");
  });
  it("keeps the terminal and conversation on an exit timeout", async () => {
    const store = await fixture();
    const { register } = await import("./typers");
    store.setState((s) => ({ lanes: { test: { ...s.lanes.test, interface: "terminal", agent: "codex", known: true, session: "saved-session" } } }));
    const load = vi.fn(); store.setState({ load });
    const unregister = register("test", vi.fn());
    const switching = store.getState().switchInterface("test", "chat");
    await vi.advanceTimersByTimeAsync(10_000); await switching;
    expect(store.getState().lanes.test).toMatchObject({ interface: "terminal", submitting: false, session: "saved-session", draft: "Hello" });
    expect(store.getState().lanes.test.error).toContain("did not exit");
    expect(load).not.toHaveBeenCalled(); unregister();
  });
});
