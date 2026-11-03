// Moving a project: the re-filing of every path-keyed map, the remembered "keep it here", and
// the open flow that asks once, before any lane exists.
import { beforeEach, afterEach, describe, expect, it, vi } from "vitest";
import { decline, declined, rekey, rekeyDrafts, structural } from "./relocate";
import type { Lane, Project } from "./store";

const mocks = vi.hoisted(() => ({ post: vi.fn(), get: vi.fn(), open: vi.fn(), invoke: vi.fn() }));
vi.mock("./api", async (original) => ({ ...await original<typeof import("./api")>(), post: mocks.post, get: mocks.get }));
vi.mock("./streams", () => ({ open: mocks.open }));
vi.mock("./notify", () => ({ notify: vi.fn(), needsYou: vi.fn() }));
vi.mock("@tauri-apps/api/core", () => ({ invoke: mocks.invoke }));
const ep = { port: 12345, token: "fixture" };

beforeEach(() => {
  vi.resetModules(); vi.clearAllMocks();
  const saved = new Map<string, string>();
  vi.stubGlobal("localStorage", { getItem: (k: string) => saved.get(k) ?? null, setItem: (k: string, v: string) => saved.set(k, v), removeItem: (k: string) => saved.delete(k) });
  mocks.open.mockReturnValue({ close: vi.fn() });
});
afterEach(() => vi.unstubAllGlobals());

const lane = (id: string, project: string) => ({ id, project, title: id, interface: "chat", agent: "claude", known: false, isolated: false, jobs: [], queued: [], conv: { turns: [] }, running: false, loaded: true, pending: [] }) as Lane;

describe("rekey", () => {
  it("moves every path-keyed map and the editor's drafts, and leaves other projects alone", () => {
    const projects: Record<string, Project> = { "/old/app": { path: "/old/app", name: "app", lanes: ["a"], endpoint: ep }, "/other": { path: "/other", name: "other", lanes: ["b"] } };
    const s = { order: ["/old/app", "/other"], projects, lanes: { a: lane("a", "/old/app"), b: lane("b", "/other") }, trusted: { "/old/app": true }, setup: { "/old/app": [] }, setupRun: { "/old/app": {} }, readiness: { "/old/app": {} }, handed: { "/old/app": {} } };
    const next = rekey(s, "/old/app", "/ws/app");
    expect(next.order).toEqual(["/ws/app", "/other"]);
    expect(next.projects["/ws/app"]).toMatchObject({ path: "/ws/app", name: "app", lanes: ["a"], endpoint: undefined });
    expect(next.projects["/old/app"]).toBeUndefined();
    expect(next.lanes.a.project).toBe("/ws/app");
    expect(next.lanes.b.project).toBe("/other");
    for (const k of ["trusted", "setup", "setupRun", "readiness", "handed"] as const) expect(Object.keys(next[k])).toEqual(["/ws/app"]);
    const drafts = rekeyDrafts({ [JSON.stringify(["/old/app", "", "a.ts"])]: 1, [JSON.stringify(["/other", "", "a.ts"])]: 2 }, "/old/app", "/ws/app");
    expect(Object.keys(drafts)).toEqual([JSON.stringify(["/ws/app", "", "a.ts"]), JSON.stringify(["/other", "", "a.ts"])]);
  });

  it("remembers keep-it-here per path, and a move forgets it", () => {
    decline("/a"); decline("/b"); decline("/a");
    expect(declined()).toEqual(["/b", "/a"]);
    decline("/a", false);
    expect(declined()).toEqual(["/b"]);
  });

  it("a folder that can never move is not offered; a moment that will pass is", () => {
    expect(structural({ code: "submodule", parent: "/p" })).toBe(true);
    expect(structural({ code: "volume" })).toBe(true);
    expect(structural({ code: "dev" })).toBe(false);
    expect(structural(undefined)).toBe(false);
  });
});

describe("opening a folder", () => {
  async function store(inWorkspace: boolean, plan: object) {
    mocks.invoke.mockImplementation(async (cmd: string) => (cmd === "in_workspace" ? inWorkspace : cmd === "is_dir" ? true : ep));
    mocks.get.mockImplementation(async (_ep: unknown, path: string) => (path === "/api/project/move" ? plan : []));
    const { useStore } = await import("./store");
    useStore.setState({ lanes: {}, projects: {}, order: [], active: undefined, moving: null });
    return useStore;
  }

  it("offers the move once, before any lane exists, and keep-it-here is remembered", async () => {
    const s = await store(false, { from: "/old/app", to: "/ws/app" });
    await s.getState().openProject("/old/app", true);
    expect(s.getState().moving).toMatchObject({ phase: "offer", from: "/old/app", to: "/ws/app", auto: true });
    expect(s.getState().projects["/old/app"].lanes).toEqual([]);
    await s.getState().answerMove(false);
    expect(s.getState().moving).toBeNull();
    expect(s.getState().projects["/old/app"].lanes).toHaveLength(1);
    expect(declined()).toEqual(["/old/app"]);
    await s.getState().closeProject("/old/app");
    await s.getState().openProject("/old/app", true);
    expect(s.getState().moving).toBeNull();
  });

  it("a folder already in the workspace, or one opened without the offer, opens without a word", async () => {
    const s = await store(true, { from: "/ws/app", to: "/ws/app" });
    await s.getState().openProject("/ws/app", true);
    expect(s.getState().moving).toBeNull();
    expect(s.getState().projects["/ws/app"].lanes).toHaveLength(1);
    const t = await store(false, { from: "/new/app", to: "/ws/app" });
    await t.getState().openProject("/new/app");
    expect(t.getState().moving).toBeNull();
  });

  it("a submodule opens where it is, with no refusal shown", async () => {
    const s = await store(false, { from: "/repo/sub", to: "/ws/sub", refusal: { code: "submodule", parent: "/repo" } });
    await s.getState().openProject("/repo/sub", true);
    expect(s.getState().moving).toBeNull();
    expect(s.getState().projects["/repo/sub"].lanes).toHaveLength(1);
  });

  it("a move re-files the project under its new path and closes the old daemon", async () => {
    const s = await store(false, { from: "/old/app", to: "/ws/app" });
    mocks.post.mockResolvedValue({ from: "/old/app", to: "/ws/app", warnings: [] });
    await s.getState().openProject("/old/app", true);
    await s.getState().answerMove(true);
    expect(s.getState().moving).toMatchObject({ phase: "moved", to: "/ws/app" });
    expect(s.getState().order).toEqual(["/ws/app"]);
    expect(s.getState().projects["/ws/app"].lanes).toHaveLength(1);
    expect(mocks.invoke).toHaveBeenCalledWith("close_project", { path: "/old/app" });
  });

  it("a lost reply after the folder moved still re-files it", async () => {
    const s = await store(false, { from: "/old/app", to: "/ws/app" });
    mocks.invoke.mockImplementation(async (cmd: string, args?: { path?: string }) => (cmd === "in_workspace" ? false : cmd === "is_dir" ? args?.path === "/ws/app" : ep));
    mocks.post.mockRejectedValue(new TypeError("Load failed"));
    await s.getState().openProject("/old/app", true);
    await s.getState().answerMove(true);
    expect(s.getState().moving).toMatchObject({ phase: "moved" });
    expect(s.getState().order).toEqual(["/ws/app"]);
  });
});
