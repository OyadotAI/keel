// Naming contracts across providers, durable pending intent, history, and stale refreshes.
import { beforeEach, describe, expect, it, vi } from "vitest";
import { createLaneNames, restoredNaming, resumeCommand, sessionForLane } from "./lane-names";
import type { Agent, Lane, Project, Session } from "./store";
const mocks = vi.hoisted(() => ({ post: vi.fn() }));
vi.mock("./api", () => ({ post: mocks.post }));
const ep = { port: 12345, token: "fixture" };
function fixture(agent: Agent = "claude") {
  const lane: Lane = { id: "lane", project: "/repo", title: "New lane", agent, session: "s1", known: true, naming: { revision: 0 }, isolated: false, jobs: [], queued: [], conv: { turns: [] }, running: false, loaded: true, pending: [] };
  const model: { lanes: Record<string, Lane>; projects: Record<string, Project> } = { lanes: { lane }, projects: { "/repo": { path: "/repo", name: "Repo", lanes: ["lane"], endpoint: ep } } };
  const persist = vi.fn();
  const names = createLaneNames({ read: () => model, lane: (id, patch) => { if (model.lanes[id]) model.lanes[id] = { ...model.lanes[id], ...patch }; }, project: (path, patch) => { model.projects[path] = { ...model.projects[path], ...patch }; }, endpoint: async () => ep, persist, label: (provider) => ({ claude: "Claude", codex: "Codex", antigravity: "Antigravity" })[provider] });
  return { model, persist, names };
}
function session(provider: Agent = "claude", title = "Provider name", revision = 1): Session {
  return { id: "s1", provider, title, title_source: "explicit", title_revision: revision, messages: 1, live: false, busy: false };
}
beforeEach(() => { vi.resetAllMocks(); });
describe.each<Agent>(["claude", "codex", "antigravity"])("%s naming", (provider) => {
  it("adopts native names and later terminal renames while rejecting older snapshots", () => {
    const { names, model } = fixture(provider);
    names.reconcile("/repo", [session(provider, "Automatic", 1)]);
    expect(model.lanes.lane.title).toBe("Automatic");
    names.reconcile("/repo", [session(provider, "Terminal rename", 2)]);
    names.reconcile("/repo", [session(provider, "Stale reply", 1)]);
    expect(model.lanes.lane.title).toBe("Terminal rename");
  });
  it("saves natively and preserves the History name after close", async () => {
    const { names, model, persist } = fixture(provider);
    mocks.post.mockResolvedValue(session(provider, "My session", 2));
    expect(await names.rename("lane", "My session")).toBe(true);
    expect(persist).toHaveBeenCalled();
    expect(mocks.post).toHaveBeenCalledWith(ep, "/api/session/rename", expect.objectContaining({ id: "s1", provider, name: "My session" }));
    expect(await names.flush("lane")).toBe(true); delete model.lanes.lane;
    expect(model.projects["/repo"].sessions?.[0].title).toBe("My session");
    expect(names.reconcile("/repo", [session(provider, "Old", 1)])[0].title).toBe("My session");
  });
  it("holds immediate close until an in-flight save completes", async () => {
    const { names, model } = fixture(provider); let finish!: (s: Session) => void;
    mocks.post.mockImplementation(() => new Promise((resolve) => { finish = resolve; }));
    const renaming = names.rename("lane", "Pending"); await Promise.resolve(); await Promise.resolve();
    let closed = false; const closing = names.flush("lane").then((okay) => { closed = okay; });
    expect(closed).toBe(false); expect(model.lanes.lane.naming?.pending).toBe("Pending");
    finish(session(provider, "Pending", 3)); await renaming; await closing; expect(closed).toBe(true);
  });
  it("keeps failed names retryable and prevents an unsaved close", async () => {
    const { names, model } = fixture(provider); mocks.post.mockRejectedValue(new Error("offline"));
    expect(await names.rename("lane", "Retry me")).toBe(false);
    expect(model.lanes.lane.naming?.pending).toBe("Retry me"); expect(model.lanes.lane.nameError).toContain("Rename again to retry");
    expect(await names.flush("lane")).toBe(false);
    mocks.post.mockResolvedValue(session(provider, "Retry me", 4)); expect(await names.rename("lane", "Retry me")).toBe(true);
    expect(model.lanes.lane.nameError).toBeUndefined();
  });
  it("serializes rapid renames without an older success replacing the newer edit", async () => {
    const { names, model } = fixture(provider); let first!: (s: Session) => void;
    mocks.post.mockImplementationOnce(() => new Promise((resolve) => { first = resolve; })).mockResolvedValueOnce(session(provider, "Second", 3));
    const one = names.rename("lane", "First"); const two = names.rename("lane", "Second");
    await Promise.resolve(); await Promise.resolve(); expect(mocks.post).toHaveBeenCalledOnce();
    first(session(provider, "First", 2)); await one; expect(model.lanes.lane.title).toBe("Second");
    await two; expect(model.lanes.lane.title).toBe("Second"); expect(model.lanes.lane.naming?.pending).toBeUndefined();
  });
});
it.each<Agent>(["codex", "antigravity"])("associates an early %s rename with its real session", async (provider) => {
  const { names, model } = fixture(provider); model.lanes.lane.known = false; model.lanes.lane.session = "temporary";
  expect(await names.rename("lane", "Early")).toBe(true); expect(mocks.post).not.toHaveBeenCalled();
  model.lanes.lane.known = true; model.lanes.lane.session = "s1";
  mocks.post.mockResolvedValueOnce(session(provider, "Generated", 1)).mockResolvedValueOnce(session(provider, "Early", 2));
  await names.register("lane"); expect(mocks.post.mock.calls[1][2]).toMatchObject({ id: "s1", provider, name: "Early" });
  expect(model.lanes.lane.title).toBe("Early");
});
it("isolates equal ids from different providers", async () => {
  const { names, model } = fixture("codex"); model.lanes.other = { ...model.lanes.lane, id: "other", agent: "antigravity", title: "Other" };
  mocks.post.mockResolvedValue(session("codex", "Codex name", 2)); await names.rename("lane", "Codex name"); expect(model.lanes.other.title).toBe("Other");
});
it("makes a migration decision only for legacy layouts", () => {
  expect(restoredNaming("Old manual label")).toEqual({ revision: 0, legacy: "Old manual label" });
  expect(restoredNaming("New lane")).toEqual({ revision: 0 });
  expect(restoredNaming("Native", { revision: 2, source: "explicit" })).toEqual({ revision: 2, source: "explicit" });
});
it.each<Agent>(["claude", "codex", "antigravity"])("copies the correct %s resume command", (provider) => {
  expect(resumeCommand(session(provider))).toBe({ claude: "claude --resume s1", codex: "codex resume s1", antigravity: "agy --conversation s1" }[provider]);
});

it("retains the last A edit when an A to B to A sequence ends in failure", async () => {
  const { names, model } = fixture();
  let first!: (s: Session) => void;
  mocks.post.mockRejectedValue(new Error("last save failed"))
    .mockImplementationOnce(() => new Promise((resolve) => { first = resolve; }))
    .mockResolvedValueOnce(session("claude", "B", 3));
  const one = names.rename("lane", "A"); const two = names.rename("lane", "B"); const three = names.rename("lane", "A");
  await Promise.resolve(); await Promise.resolve(); first(session("claude", "A", 2));
  expect(await Promise.all([one, two, three])).toEqual([true, true, false]);
  expect(model.lanes.lane).toMatchObject({ title: "A", naming: { pending: "A" }, nameSaving: false });
  expect(model.lanes.lane.nameError).toContain("Rename again to retry");
  expect(await names.flush("lane")).toBe(false);
});

it("uses provider identity when reading activity metadata with equal session ids", () => {
  const { model } = fixture("codex");
  const list = [session("claude"), session("codex")];
  expect(sessionForLane(model.lanes.lane, list)?.provider).toBe("codex");
});

it("retains a named Claude lane until its deferred name has a real resumable session", async () => {
  const { names, model } = fixture("claude"); model.lanes.lane.known = false;
  mocks.post.mockResolvedValue({ ...session("claude", "Early", 1), title_source: "pending" });
  expect(await names.rename("lane", "Early")).toBe(true);
  expect(await names.flush("lane")).toBe(false);
  expect(model.lanes.lane.nameError).toContain("Start this conversation");
  mocks.post.mockResolvedValue(session("claude", "Early", 3));
  expect(await names.flush("lane")).toBe(true);
  expect(model.lanes.lane.known).toBe(true);
});
it.each<Agent>(["claude", "codex", "antigravity"])("reconnect never overwrites a later %s terminal rename with stale local intent", async (provider) => {
  const { names, model } = fixture(provider);
  model.lanes.lane.naming = { pending: "Old intent", edit: "old-edit", revision: 1, source: "generated" };
  mocks.post.mockResolvedValue(session(provider, "Later terminal rename", 4));
  await names.register("lane");
  expect(mocks.post).toHaveBeenCalledOnce();
  expect(model.lanes.lane).toMatchObject({ title: "Later terminal rename", naming: { revision: 4 } });
  expect(model.lanes.lane.naming?.pending).toBeUndefined();
});

it.each<Agent>(["claude", "codex", "antigravity"])("a recovered %s name retires the old failed job so close succeeds", async (provider) => {
  const { names, model } = fixture(provider); model.lanes.lane.naming = { revision: 1, source: "generated" };
  mocks.post.mockRejectedValueOnce(new Error("offline"));
  expect(await names.rename("lane", "Old intent")).toBe(false);
  mocks.post.mockResolvedValue(session(provider, "Newer terminal name", 4));
  await names.register("lane");
  expect(model.lanes.lane.title).toBe("Newer terminal name");
  expect(model.lanes.lane.naming?.pending).toBeUndefined(); expect(model.lanes.lane.nameError).toBeUndefined();
  expect(await names.flush("lane")).toBe(true);
  expect(mocks.post).toHaveBeenCalledTimes(2);
});
