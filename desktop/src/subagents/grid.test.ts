// Lifecycle filtering and responsive geometry are independent of provider execution.
import { expect, it } from "vitest";
import { gridColumns, liveInvocations, type Invocation, type ChildState } from "../subagents";
const child = (id: string, status: Invocation["status"] = "working"): Invocation => ({ id, status, output: "waiting" });
const snapshot = (invocations: Invocation[], connected = true): ChildState => ({ scope: "parent", invocations, connected, truncated: false });
it("restores full parent space only after every child has a confirmed terminal outcome", () => {
  const live = [child("one"), child("two")];
  expect(liveInvocations(live, snapshot([child("one", "done"), child("two")]))).toEqual([child("two")]);
  for (const status of ["done", "failed", "cancelled", "interrupted"] as const) {
    expect(liveInvocations(live, snapshot([child("one", status), child("two", status)]))).toEqual([]);
  }
});
it("missing or disconnected snapshots retain prior work as unconfirmed, not completed", () => {
  const before = [child("one")];
  expect(liveInvocations(before, snapshot([], false))[0].status).toBe("unknown");
  expect(liveInvocations(before, snapshot([]))[0].status).toBe("unknown");
  expect(liveInvocations(before, undefined)[0].status).toBe("unknown");
  expect(liveInvocations([], snapshot([child("old", "done"), child("new")])).map((c) => c.id)).toEqual(["new"]);
});
it("supports a four-pane square and stacks narrow grids without hiding children", () => {
  expect(gridColumns(1200, 4)).toBe(2);
  expect(gridColumns(1200, 5)).toBe(3);
  expect(gridColumns(400, 4)).toBe(1);
  expect(gridColumns(800, 1)).toBe(1);
});
