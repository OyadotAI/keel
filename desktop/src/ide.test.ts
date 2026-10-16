import { beforeEach, expect, it, vi } from "vitest";
import { completedEdits, rememberedLayout, relativeFile, useIde, visibleTree } from "./ide";
import { applyFrames } from "./reduce";

beforeEach(() => {
  useIde.setState({ layout: "chat", lanes: {} });
  vi.stubGlobal("localStorage", { getItem: vi.fn(), setItem: vi.fn() });
});

it("keeps the layout preference separate from agent session state and tolerates unavailable storage", () => {
  useIde.getState().setLayout("ide");
  expect(useIde.getState().layout).toBe("ide");
  expect(localStorage.setItem).toHaveBeenCalledWith("keel.workspace-layout", "ide");
  vi.mocked(localStorage.getItem).mockReturnValue("ide");
  expect(rememberedLayout()).toBe("ide");
  vi.mocked(localStorage.getItem).mockImplementation(() => { throw new Error("blocked"); });
  expect(rememberedLayout()).toBe("chat");
});

it("keeps Markdown view choices per file and reveals source when navigating to a line", () => {
  const ide = useIde.getState();
  ide.open("one", "README.md", "code");
  expect(ide.markdownView("one", "split")).toBe(true);
  ide.open("one", "guide.md", "code");
  ide.markdownView("one", "preview");
  ide.open("one", "README.md", "code");
  expect(useIde.getState().lanes.one.markdown?.["README.md"]).toBe("split");
  ide.navigate("one", "README.md", 42);
  expect(useIde.getState().lanes.one).toMatchObject({ mode: "code", markdown: { "README.md": "source", "guide.md": "preview" }, targetLine: { line: 42 } });
  ide.open("two", "main.ts", "code");
  expect(ide.markdownView("two")).toBe(false);
  expect(useIde.getState().lanes.two.markdown).toBeUndefined();
});

it("pauses following on manual selection, restores it explicitly, and retains lane-specific tabs", () => {
  const s = useIde.getState();
  s.open("one", "src/main.ts", "diff", false);
  expect(useIde.getState().lanes.one.follow).toBe(true);
  s.open("one", "README.md", "code");
  s.open("two", "src/other.rs", "code");
  expect(useIde.getState().lanes.one).toMatchObject({ selected: "README.md", follow: false, tabs: ["src/main.ts", "README.md"], expanded: ["src"] });
  s.patch("one", { follow: true });
  s.open("one", "src/main.ts", "diff", false);
  expect(useIde.getState().lanes.one).toMatchObject({ selected: "src/main.ts", follow: true });
  expect(useIde.getState().lanes.two.selected).toBe("src/other.rs");
  s.close("one", "src/main.ts");
  expect(useIde.getState().lanes.one).toMatchObject({ selected: "README.md", follow: false });
  s.close("one", "README.md");
  expect(useIde.getState().lanes.one.selected).toBeUndefined();
});

it("only follows successful completed writes, including child calls", () => {
  let conv = applyFrames({ turns: [] }, [
    { op: "open", turn: "t", prompt: "change it" },
    { op: "call", turn: "t", id: "edit", tool: "Edit", subject: "main", writes: ["src/main.ts"] },
    { op: "call", turn: "t", id: "failed", tool: "Edit", subject: "bad", writes: ["bad.ts"] },
    { op: "result", turn: "t", id: "failed", state: "failed" },
  ]);
  expect(completedEdits(conv)).toEqual([]);
  conv = applyFrames(conv, [{ op: "result", turn: "t", id: "edit", state: "ok" }]);
  expect(completedEdits(conv).map((e) => e.path)).toEqual(["src/main.ts"]);
  conv = applyFrames(conv, [
    { op: "call", turn: "t", id: "child", parent: "agent", tool: "Write", subject: "child", writes: ["child.ts"] },
    { op: "result", turn: "t", id: "child", state: "ok" },
  ]);
  expect(completedEdits(conv).map((e) => e.path)).toEqual(["src/main.ts", "child.ts"]);
});

it("normalizes paths only within the active checkout", () => {
  expect(relativeFile("/repo/.keel/worktrees/feature/src/a.ts", "/repo/.keel/worktrees/feature")).toBe("src/a.ts");
  expect(relativeFile("/repo/src/a.ts", "/repo/.keel/worktrees/feature")).toBeUndefined();
  expect(relativeFile("../secret", "/repo")).toBeUndefined();
  expect(relativeFile("C:\\repo\\src\\a.ts", "C:\\repo")).toBe("src/a.ts");
  expect(relativeFile("C:/other/a.ts", "C:/repo")).toBeUndefined();
  expect(relativeFile("src/a.ts", "/repo")).toBe("src/a.ts");
});

it("flattens expanded folders for virtualization and includes removed changed files", () => {
  const nodes = [{ name: "src", path: "src", dir: true, children: [{ name: "a.ts", path: "src/a.ts", dir: false }] }];
  expect(visibleTree(nodes, []).map((r) => r.node.path)).toEqual(["src"]);
  const rows = visibleTree(nodes, ["src"], ["removed.ts", "src/a.ts"]);
  expect(rows.map((r) => r.node.path)).toEqual(["src", "src/a.ts", "removed.ts"]);
  expect(rows[1]).toMatchObject({ depth: 1, parent: "src" });
});

it("finishes reads under event bursts, schedules one follow-up, and rejects late context responses", async () => {
  const { liveReader } = await import("./ide");
  vi.useFakeTimers();
  try {
    let finish!: (value: string) => void;
    const read = vi.fn(() => new Promise<string>((resolve) => { finish = resolve; }));
    const accept = vi.fn();
    const reader = liveReader(read, accept, vi.fn());
    reader.refresh(); reader.refresh(); reader.refresh();
    await vi.advanceTimersByTimeAsync(150);
    expect(read).toHaveBeenCalledTimes(1);
    reader.refresh(); reader.refresh();
    finish("first");
    await vi.advanceTimersByTimeAsync(0);
    expect(accept).toHaveBeenCalledWith("first");
    await vi.advanceTimersByTimeAsync(150);
    expect(read).toHaveBeenCalledTimes(2);
    reader.close();
    finish("stale checkout");
    await vi.advanceTimersByTimeAsync(300);
    expect(accept).toHaveBeenCalledTimes(1);
    reader.refresh();
    await vi.advanceTimersByTimeAsync(300);
    expect(read).toHaveBeenCalledTimes(2);
  } finally { vi.useRealTimers(); }
});

it("keeps staged and unstaged selections distinct, and line navigation exits diff/follow mode", () => {
  const ide = useIde.getState();
  ide.open("git", "src/main.ts", "diff", true, true);
  expect(useIde.getState().lanes.git).toMatchObject({ selected: "src/main.ts", mode: "diff", staged: true });
  ide.open("git", "src/main.ts", "diff", true, false);
  expect(useIde.getState().lanes.git.staged).toBe(false);
  ide.navigate("git", "src/main.ts", 42);
  expect(useIde.getState().lanes.git).toMatchObject({ mode: "code", follow: false, targetLine: { line: 42 } });
  expect(useIde.getState().lanes.git.staged).toBeUndefined();
});

it("hands IDE shortcuts out of xterm while preserving Ctrl+C and legacy Chat behavior", async () => {
  vi.stubGlobal("navigator", { platform: "MacIntel" });
  const { isKeel } = await import("./keys");
  const key = (code: string, modifiers: Partial<KeyboardEvent>) => ({ code, metaKey: false, ctrlKey: false, altKey: false, shiftKey: false, ...modifiers } as KeyboardEvent);
  useIde.getState().setLayout("ide");
  expect(isKeel(key("KeyP", { metaKey: true }))).toBe(true);
  expect(isKeel(key("KeyG", { ctrlKey: true }))).toBe(true);
  expect(isKeel(key("KeyC", { ctrlKey: true }))).toBe(false);
  useIde.getState().setLayout("chat");
  expect(isKeel(key("KeyP", { metaKey: true }))).toBe(false);
  expect(isKeel(key("KeyG", { ctrlKey: true }))).toBe(false);
  expect(isKeel(key("KeyK", { metaKey: true }))).toBe(true);
});
