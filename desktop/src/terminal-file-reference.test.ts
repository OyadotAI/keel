// Terminal file insertion preserves prompt ownership across asynchronous validation and reconnects.
import { afterEach, beforeEach, expect, it, vi } from "vitest";
const api = vi.hoisted(() => ({ post: vi.fn() }));
vi.mock("./api", async (original) => ({ ...await original<typeof import("./api")>(), post: api.post }));
import { insertTerminalFile, type TerminalReferenceDependencies } from "./terminal-file-reference";
import { register, type TerminalLease, terminalLease, pasteInto } from "./typers";
import { terminalFilePath } from "./terminal-file-path";
import type { Lane } from "./store";

const reference = { project: "/repo", path: "src/日本語 file.ts" };
let lane: Lane, active: string | undefined, connection: ReturnType<typeof register>;
let writes: ReturnType<typeof vi.fn>, focus: ReturnType<typeof vi.fn>;
let listeners: Set<Parameters<TerminalReferenceDependencies["subscribe"]>[0]>;
let deps: TerminalReferenceDependencies;
const change = (patch: Partial<Lane> = {}, selected = active) => { lane = { ...lane, ...patch }; active = selected; for (const listener of listeners) listener({ lanes: { terminal: lane }, active }); };
beforeEach(() => {
  lane = { id: "terminal", project: "/repo", interface: "terminal", agent: "claude", title: "Test", session: "session", isolated: false, known: false, jobs: [], queued: [], conv: { turns: [] }, running: false, loaded: true, pending: [] };
  active = lane.id; writes = vi.fn(); focus = vi.fn(); listeners = new Set();
  connection = register(lane.id, writes, { focus });
  api.post.mockReset().mockResolvedValue({ ok: true });
  deps = { read: () => ({ lanes: { terminal: lane }, active }), patch: (_id, patch) => change(typeof patch === "function" ? patch(lane) : patch), endpoint: async () => ({ port: 1, token: "fixture" }), subscribe: (listener) => { listeners.add(listener); return () => { listeners.delete(listener); }; } };
});
afterEach(() => connection());

it.each(["claude", "codex", "antigravity"] as const)("%s receives a quoted file at its prompt without Enter or queued work", async (agent) => {
  change({ agent });
  expect(await insertTerminalFile(deps, lane.id, reference)).toBe(true);
  expect(writes.mock.calls).toEqual([["\x1b[200~'src/日本語 file.ts' \x1b[201~"]]);
  expect(writes.mock.calls[0][0]).not.toMatch(/[\r\n]/);
  expect(lane.queued).toEqual([]); expect(lane.references).toBeUndefined();
  expect(focus).toHaveBeenCalledOnce(); expect(listeners.size).toBe(0);
  expect(api.post).toHaveBeenCalledWith({ port: 1, token: "fixture" }, "/api/references/validate", { reference });
});

it.each(["a\rb", "a\nb", "a\tb", "a\x1b[201~b", "a\x85b"])("a control-bearing filename %j never becomes input or a different file", async (path) => {
  expect(await insertTerminalFile(deps, lane.id, { ...reference, path })).toBe(false);
  expect(api.post).not.toHaveBeenCalled(); expect(writes).not.toHaveBeenCalled();
  expect(lane.referenceError).toContain("control characters");
});

it("quoting preserves apostrophes, Unicode and literal platform separators", () => {
  expect(terminalFilePath("src/file.ts")).toBe("src/file.ts ");
  expect(terminalFilePath("src/it's 日本語.ts")).toBe("'src/it'\\''s 日本語.ts' ");
  expect(terminalFilePath("src\\file.ts")).toBe("'src\\file.ts' ");
});

it.each(["replacement", "close", "hidden", "away-back", "session", "provider", "checkout"])("%s during validation cannot receive a stale paste", async (reason) => {
  let finish!: (value: { ok: boolean }) => void;
  api.post.mockReturnValue(new Promise((resolve) => { finish = resolve; }));
  const adding = insertTerminalFile(deps, lane.id, reference); await Promise.resolve();
  if (reason === "replacement") { connection(); connection = register(lane.id, writes, { focus }); }
  if (reason === "close") connection();
  if (reason === "hidden") { connection.visible(false); connection.visible(true); }
  if (reason === "away-back") { change({}, "other"); change({}, "terminal"); }
  if (reason === "session") change({ session: "replacement" });
  if (reason === "provider") change({ agent: "codex" });
  if (reason === "checkout") change({ wt: "other" });
  finish({ ok: true }); expect(await adding).toBe(false);
  expect(writes).not.toHaveBeenCalled(); expect(focus).not.toHaveBeenCalled();
  expect(lane.referenceError).toContain("destination changed"); expect(listeners.size).toBe(0);
});

it("a decision appearing during validation is not answered by the path", async () => {
  let finish!: (value: { ok: boolean }) => void;
  api.post.mockReturnValue(new Promise((resolve) => { finish = resolve; }));
  const adding = insertTerminalFile(deps, lane.id, reference); await Promise.resolve();
  change({ attention: { id: "decision", kind: "permission" } }); finish({ ok: true });
  expect(await adding).toBe(false); expect(writes).not.toHaveBeenCalled();
  expect(lane.referenceError).toContain("current question or approval");
});

it("unavailable or foreign targets fail before validation and never queue input", async () => {
  expect(await insertTerminalFile(deps, lane.id, { ...reference, wt: "other" })).toBe(false);
  change({ elsewhere: true }); expect(await insertTerminalFile(deps, lane.id, reference)).toBe(false);
  change({ elsewhere: false, disconnected: true }); expect(await insertTerminalFile(deps, lane.id, reference)).toBe(false);
  change({ disconnected: false }); connection(); expect(await insertTerminalFile(deps, lane.id, reference)).toBe(false);
  expect(api.post).not.toHaveBeenCalled(); expect(writes).not.toHaveBeenCalled(); expect(lane.queued).toEqual([]);
});

it("a failed validation or a closed socket never reports insertion", async () => {
  api.post.mockRejectedValueOnce(new Error("validation unavailable"));
  expect(await insertTerminalFile(deps, lane.id, reference)).toBe(false);
  expect(writes).not.toHaveBeenCalled();
  writes.mockReturnValue(false);
  expect(await insertTerminalFile(deps, lane.id, reference)).toBe(false);
  expect(lane.referenceStatus).toBeUndefined(); expect(focus).not.toHaveBeenCalled();
});

it("a captured lease cannot write through another registration or a hide/show cycle", () => {
  const lease: TerminalLease = terminalLease(lane.id)!;
  connection.visible(false); connection.visible(true);
  expect(pasteInto(lane.id, "file", lease)).toBe(false);
  const next = terminalLease(lane.id)!;
  connection(); connection = register(lane.id, writes);
  expect(pasteInto(lane.id, "file", next)).toBe(false); expect(writes).not.toHaveBeenCalled();
});
