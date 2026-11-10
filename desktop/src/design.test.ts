// The live canvas's rule: which turn counts as editing the frontend, for every agent's shape.
import { describe, expect, it, vi } from "vitest";
vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn(async () => undefined) }));
vi.mock("@tauri-apps/api/event", () => ({ listen: vi.fn() }));
vi.mock("./store", () => ({ useStore: { getState: () => ({ lanes: {} }), setState: vi.fn(), subscribe: vi.fn() } }));
vi.mock("./typers", () => ({ typeInto: vi.fn() }));
import { editingFrontend } from "./design";
import type { Lane } from "./store";

const lane = (turn: Record<string, unknown>, running = true) =>
  ({ running, conv: { turns: [{ calls: {}, files: [], ...turn }] } }) as unknown as Lane;

describe("editingFrontend", () => {
  it("names the frontend file a Claude or Antigravity Edit/Write call wrote", () => {
    expect(editingFrontend(lane({ calls: { a: { tool: "Edit", subject: "x", input: { file_path: "/r/src/Button.tsx" } } } }))).toBe("/r/src/Button.tsx");
  });
  it("names a frontend file Codex wrote outside a call", () => {
    expect(editingFrontend(lane({ files: ["/r/app/page.css"] }))).toBe("/r/app/page.css");
  });
  it("ignores backend files", () => {
    expect(editingFrontend(lane({ calls: { a: { tool: "Write", subject: "/r/src/main.rs" } }, files: ["/r/api.go"] }))).toBeUndefined();
  });
  it("says nothing once the turn has ended", () => {
    expect(editingFrontend(lane({ files: ["/r/a.tsx"] }, false))).toBeUndefined();
    expect(editingFrontend(lane({ files: ["/r/a.tsx"], closed: "done" }))).toBeUndefined();
  });
});
