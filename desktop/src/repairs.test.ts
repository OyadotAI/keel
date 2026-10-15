import { afterEach, beforeEach, expect, it, vi } from "vitest";
import type { Lane } from "./store";

vi.mock("./streams", () => ({ open: vi.fn() }));
vi.mock("./notify", () => ({ notify: vi.fn() }));
vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn() }));

beforeEach(() => {
  vi.resetModules();
  vi.stubGlobal("localStorage", { getItem: () => null, setItem: vi.fn() });
});
afterEach(() => vi.unstubAllGlobals());

async function fixture() {
  const { useStore } = await import("./store");
  const plan: Lane = { id: "plan", project: "/repo", title: "My plan", interface: "chat", agent: "claude", mode: "plan", draft: "Keep my draft", known: false, isolated: false, jobs: [], queued: [], conv: { turns: [] }, running: false, loaded: true, pending: [] };
  useStore.setState({ projects: { "/repo": { path: "/repo", name: "Repo", lanes: ["plan"] } }, order: ["/repo"], lanes: { plan }, active: "plan" });
  return useStore;
}

it("creates an editing lane without changing the person's planning lane or draft", async () => {
  const store = await fixture();
  const { repairLane } = await import("./repairs");
  const id = repairLane("/repo");
  expect(id).not.toBe("plan");
  expect(store.getState().lanes[id]).toMatchObject({ project: "/repo", interface: "chat", mode: "acceptEdits", isolated: false, title: "Readiness fixes" });
  expect(store.getState().lanes.plan).toMatchObject({ mode: "plan", draft: "Keep my draft" });
});

it("reuses a shared editing lane and keeps repairs out of an isolated checkout", async () => {
  const store = await fixture();
  const base = store.getState().lanes.plan;
  store.setState({
    lanes: { plan: base, branch: { ...base, id: "branch", mode: "acceptEdits", isolated: true, wt: "feature" }, shared: { ...base, id: "shared", mode: "acceptEdits", running: true } },
    projects: { "/repo": { path: "/repo", name: "Repo", lanes: ["plan", "branch", "shared"] } }, active: "branch",
  });
  const { repairLane } = await import("./repairs");
  expect(repairLane("/repo")).toBe("shared");
  expect(Object.keys(store.getState().lanes)).toHaveLength(3);
});
