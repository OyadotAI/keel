import { afterEach, beforeEach, expect, it, vi } from "vitest";

const store = new Map<string, string>();
beforeEach(() => {
  vi.resetModules();
  store.clear();
  vi.stubGlobal("localStorage", { getItem: (k: string) => store.get(k) ?? null, setItem: (k: string, v: string) => store.set(k, v), removeItem: (k: string) => store.delete(k) });
});
afterEach(() => { vi.useRealTimers(); vi.unstubAllGlobals(); });
const load = () => import("./quick-commands");

it("commands belong to one project and survive a reload of the module", async () => {
  const q = await load();
  q.addQuickCommand("/a", { name: "test", command: "make check" });
  const again = await (vi.resetModules(), load());
  expect(again.quickCommands("/a")).toEqual([{ name: "test", command: "make check" }]);
  expect(again.quickCommands("/b")).toEqual([]);
});

it("a duplicate name or a blank half is refused with the sentence to show", async () => {
  const q = await load();
  q.addQuickCommand("/a", { name: "test", command: "make" });
  expect(() => q.addQuickCommand("/a", { name: "test", command: "x" })).toThrow("A quick command named “test” already exists.");
  expect(() => q.addQuickCommand("/a", { name: " ", command: "x" })).toThrow();
  q.removeQuickCommand("/a", "test");
  expect(q.quickCommands("/a")).toEqual([]);
});

it("storage that will not parse reads as no commands", async () => {
  store.set("keel.quick-commands", "{nope");
  expect((await load()).quickCommands("/a")).toEqual([]);
});

it("runs in a connected shell at once, with Enter, and brings the shell on screen", async () => {
  const q = await load();
  const typed: string[] = [];
  q.registerShell("L", (t) => { typed.push(t); });
  const open = vi.fn();
  q.runQuickCommand("L", { name: "t", command: "make check" }, open);
  expect(typed).toEqual(["make check\r"]);
  expect(open).toHaveBeenCalledOnce();
  expect(q.quickWaiting("L")).toBe(false);
});

it("opens a closed shell and types the command once it connects", async () => {
  const q = await load();
  const open = vi.fn();
  q.runQuickCommand("L", { name: "t", command: "ls" }, open);
  expect(open).toHaveBeenCalledOnce();
  expect(q.quickWaiting("L")).toBe(true);
  const typed: string[] = [];
  q.registerShell("L", (t) => { typed.push(t); });
  expect(typed).toEqual(["ls\r"]);
  expect(q.quickWaiting("L")).toBe(false);
  expect(q.quickFailure("L")).toBeUndefined();
});

it("a shell that cannot start ends the wait and says why", async () => {
  const q = await load();
  q.runQuickCommand("L", { name: "build", command: "make" }, () => {});
  q.shellFailed("L", "spawn failed");
  expect(q.quickFailure("L")).toBe("build didn't run: the shell couldn't start. spawn failed");
  const typed: string[] = [];
  q.registerShell("L", (t) => { typed.push(t); });
  expect(typed).toEqual([]);
});

it("every wait ends: a shell that never opens fails at the ceiling", async () => {
  vi.useFakeTimers();
  const q = await load();
  q.runQuickCommand("L", { name: "build", command: "make" }, () => {});
  vi.advanceTimersByTime(q.SHELL_WAIT_MS);
  expect(q.quickFailure("L")).toBe("build didn't run: the shell did not open in time.");
});

it("control characters in a saved command are not typed as keys", async () => {
  const q = await load();
  const typed: string[] = [];
  q.registerShell("L", (t) => { typed.push(t); });
  q.runQuickCommand("L", { name: "t", command: "echo \x1b[31mhi\x03" }, () => {});
  expect(typed).toEqual(["echo [31mhi\r"]);
});
