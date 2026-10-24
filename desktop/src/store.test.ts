// Store-stream contracts for delivery, terminal lifecycle ordering, and supplemental observations.
import { beforeEach, afterEach, describe, expect, it, vi } from "vitest";
import type { Lane } from "./store";

const mocks = vi.hoisted(() => ({ post: vi.fn(), get: vi.fn(), open: vi.fn(), invoke: vi.fn(), needsYou: vi.fn() }));
vi.mock("./api", async (original) => ({ ...await original<typeof import("./api")>(), post: mocks.post, get: mocks.get }));
vi.mock("./streams", () => ({ open: mocks.open }));
vi.mock("./notify", () => ({ notify: vi.fn(), needsYou: mocks.needsYou }));
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

  it("native Codex requests notify once live and restore quietly without replaying older events", async () => {
    const store = await fixture();
    store.setState((s) => ({ lanes: { test: { ...s.lanes.test, interface: "terminal", agent: "codex" } }, projects: { "/repo": { ...s.projects["/repo"], endpoint: undefined } } }));
    store.getState().load("test");
    await vi.waitFor(() => expect(mocks.open.mock.calls.some(([u]) => u.includes("/api/events"))).toBe(true));
    const events = mocks.open.mock.calls.find(([u]) => u.includes("/api/events"))!;
    const request = { id: "old", kind: "permission" };
    events[2]([{ event: "terminal.attention.snapshot", data: { data: { revision: 5, lanes: { test: request } } } }]);
    expect(store.getState().lanes.test.attention).toEqual(request);
    expect(mocks.needsYou).not.toHaveBeenCalled();
    events[2]([{ event: "terminal.attention", data: { data: { lane: "test", attention: null, revision: 4 } } }]);
    expect(store.getState().lanes.test.attention).toEqual(request);
    const attention = { id: "next", kind: "permission" };
    const live = { event: "terminal.attention", data: { data: { lane: "test", attention, revision: 6 } } };
    events[2]([live, live]);
    expect(mocks.needsYou).toHaveBeenCalledExactlyOnceWith("Test is waiting for you", "Codex needs approval in its terminal.");
    events[2]([{ event: "terminal.attention.snapshot", data: { data: { revision: 6, lanes: { test: attention } } } }]);
    expect(mocks.needsYou).toHaveBeenCalledOnce();
    events[2]([{ event: "terminal.attention", data: { data: { lane: "test", attention: null, revision: 7 } } }]);
    expect(store.getState().lanes.test.attention).toBeUndefined();
    events[2]([{ event: "terminal.attention", data: { data: { lane: "test", attention: { id: "question", kind: "question" }, revision: 8 } } }]);
    expect(mocks.needsYou).toHaveBeenCalledTimes(2);
  });

  it.each([true, false])("terminal parent activity follows the last lifecycle operation: open=%s", async (open) => {
    const store = await fixture();
    store.setState((s) => ({ lanes: { test: { ...s.lanes.test, interface: "terminal", agent: "antigravity", known: true, session: "agy" } } }));
    store.getState().load("test");
    await vi.waitFor(() => expect(mocks.open.mock.calls.some(([u]) => u.includes("/api/session/tail"))).toBe(true));
    const tail = mocks.open.mock.calls.find(([u]) => u.includes("/api/session/tail"))!;
    tail[2]([{ event: "caught-up", data: "0" }]);
    const lifecycle = [{ turn: "t", op: "open" }, { turn: "t", op: "close", reason: "done" }];
    if (open) lifecycle.push({ turn: "next", op: "open" });
    tail[2]([{ event: "turn", data: lifecycle }]);
    expect(store.getState().lanes.test).toMatchObject({ running: open, transcriptOpen: open });
  });

  it("display observations never drain queued work and process exit clears them even after parent close", async () => {
    const store = await fixture();
    const queued = [{ id: "q", text: "next", attachments: [] }];
    store.setState((s) => ({ lanes: { test: { ...s.lanes.test, interface: "terminal", agent: "antigravity", known: true, session: "agy", queued } } }));
    store.getState().load("test");
    await vi.waitFor(() => expect(mocks.open.mock.calls.some(([u]) => u.includes("/api/session/tail"))).toBe(true));
    const tail = mocks.open.mock.calls.find(([u]) => u.includes("/api/session/tail"))!;
    const observation = { running: true, background: { child: "pm" }, usage: { input: 10, output: 20, cache_read: 30, cache_write: 0 }, truncated: false };
    tail[2]([{ event: "observation", data: observation }, { event: "caught-up", data: "1" }]);
    expect(store.getState().lanes.test).toMatchObject({ running: false, queued, observation });
    tail[2]([{ event: "ended", data: "gone" }]);
    expect(store.getState().lanes.test.observation).toEqual({ ...observation, running: false, background: {} });
    expect(mocks.post).not.toHaveBeenCalled();
  });

  it("an unreadable transcript clears observed activity but preserves usage until reconnect recovers", async () => {
    const store = await fixture();
    store.setState((s) => ({ lanes: { test: { ...s.lanes.test, interface: "terminal", agent: "antigravity", known: true, session: "agy", conv: { turns: [], background: { old: "qa" } } } } }));
    store.getState().load("test");
    await vi.waitFor(() => expect(mocks.open.mock.calls.some(([u]) => u.includes("/api/session/tail"))).toBe(true));
    const tail = mocks.open.mock.calls.find(([u]) => u.includes("/api/session/tail"))!;
    const observation = { running: true, background: { child: "pm" }, usage: { input: 10, output: 20, cache_read: 30, cache_write: 0 }, truncated: false };
    tail[2]([{ event: "observation", data: observation }, { event: "caught-up", data: "1" }]);
    tail[2]([{ event: "fatal", data: "that session's transcript is not on this machine" }]);
    expect(store.getState().lanes.test.observation).toEqual({ ...observation, running: false, background: {} });
    expect(store.getState().lanes.test.conv.background).toBeUndefined();
    tail[3]();
    await vi.advanceTimersByTimeAsync(2000);
    const retry = mocks.open.mock.calls.filter(([u]) => u.includes("/api/session/tail")).at(-1)!;
    expect(retry).not.toBe(tail);
    retry[2]([{ event: "observation", data: observation }, { event: "caught-up", data: "1" }]);
    expect(store.getState().lanes.test.observation).toEqual(observation);
  });

  it("a terminal whose process ended stops showing its subagents as working", async () => {
    const store = await fixture();
    store.setState((s) => ({ lanes: { test: { ...s.lanes.test, interface: "terminal", agent: "codex", known: true, session: "c" } } }));
    store.getState().load("test");
    await vi.waitFor(() => expect(mocks.open.mock.calls.some(([u]) => u.includes("/api/session/tail"))).toBe(true));
    const tail = mocks.open.mock.calls.find(([u]) => u.includes("/api/session/tail"))!;
    tail[2]([{ event: "turn", data: [{ turn: "T1", op: "open", prompt: "run the pm", replayed: true }, { turn: "T1", op: "background", id: "A1", agent: "pm", running: true }] }, { event: "caught-up", data: "1" }]);
    expect(Object.values(store.getState().lanes.test.conv.background ?? {})).toEqual(["pm"]);
    tail[2]([{ event: "ended", data: "gone" }]);
    expect(store.getState().lanes.test.conv.background).toBeUndefined();
  });

  it.each(["codex", "antigravity", "claude"])("a %s terminal followed mid-turn reads as working until the turn closes", async (agent) => {
    const store = await fixture();
    store.setState((s) => ({ lanes: { test: { ...s.lanes.test, interface: "terminal", agent: agent as Lane["agent"], known: true, session: "s" } } }));
    store.getState().load("test");
    await vi.waitFor(() => expect(mocks.open.mock.calls.some(([u]) => u.includes("/api/session/tail"))).toBe(true));
    const tail = mocks.open.mock.calls.find(([u]) => u.includes("/api/session/tail"))!;
    // The replay: one turn finished, and the one the terminal is running now, which the daemon
    // leaves open only while something is still writing it.
    tail[2]([{ event: "turn", data: [{ turn: "T0", op: "open", prompt: "earlier", replayed: true }, { turn: "T0", op: "close", reason: "done" }, { turn: "T1", op: "open", prompt: "deploy previews", replayed: true }] }, { event: "caught-up", data: "1" }]);
    expect(store.getState().lanes.test).toMatchObject({ running: true, transcriptOpen: true });
    tail[2]([{ event: "turn", data: [{ turn: "T1", op: "close", reason: "done" }] }]);
    expect(store.getState().lanes.test.running).toBe(false);
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
