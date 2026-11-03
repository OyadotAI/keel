// Child pane state follows conversation identity and retains reader choices across updates.
import { beforeEach, expect, it, vi } from "vitest";
import type { ChildSnapshot } from "../subagents";
let module: typeof import("../subagents");
const snapshot = (ids = ["run-one"]): ChildSnapshot => ({ truncated: false, invocations: ids.map((id) => ({ id, member: "pm", status: "working", output: "waiting" })) });
beforeEach(async () => { vi.resetModules(); module = await import("../subagents"); });
it("the first child reveals its pane once and future work respects collapse and selection", () => {
  module.reconcilePane("parent", snapshot());
  expect(module.paneOf("parent")).toMatchObject({ open: true, selected: "run-one" });
  module.patchPane("parent", { open: false }); module.reconcilePane("parent", snapshot(["run-one", "run-two"]));
  expect(module.paneOf("parent")).toMatchObject({ open: false, selected: "run-one" });
  expect(module.paneOf("other-parent").open).toBe(false); expect(module.paneOf("other-parent").selected).toBeUndefined();
});
it("output failure preserves text, paused following, selection and reading position", () => {
  module.reconcilePane("parent", snapshot());
  const detail = { items: [{ kind: "text", text: "Useful progress" }], availability: "available" as const, truncated: false };
  module.patchChildOutput("parent", "run-one", { detail, top: 24, following: false });
  module.patchChildOutput("parent", "run-one", { disconnected: true });
  expect(module.paneOf("parent").outputs["run-one"]).toMatchObject({ detail, top: 24, following: false, disconnected: true });
  module.patchPane("parent", { open: false }); module.patchPane("parent", { open: true });
  expect(module.paneOf("parent").outputs["run-one"].top).toBe(24);
});
it("selected caches are bounded while the current invocation remains retained", () => {
  for (const id of ["one", "two", "three", "four"]) module.patchChildOutput("parent", id, { top: 1 });
  expect(Object.keys(module.paneOf("parent").outputs)).toEqual(["two", "three", "four"]);
});
