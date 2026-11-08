// Lifecycle filtering and responsive geometry are independent of provider execution.
import { expect, it } from "vitest";
import { flipFrom, gridColumns, liveInvocations, withDeparting, type Invocation, type ChildState } from "../subagents";
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
it("a finished pane keeps its slot while it closes and does not come back once it is gone", () => {
  const shown = withDeparting([child("one"), child("three")], [{ child: child("two"), at: 1 }]);
  expect(shown.map((c) => [c.id, !!c.leaving])).toEqual([["one", false], ["two", true], ["three", false]]);
  expect(withDeparting([child("two")], [{ child: child("two"), at: 0 }]).map((c) => !!c.leaving)).toEqual([false]);
  expect(withDeparting([], [{ child: child("late"), at: 9 }]).map((c) => c.id)).toEqual(["late"]);
});
it("a pane plays its move from the box it had and nothing when it stayed put or has no size", () => {
  expect(flipFrom({ left: 0, top: 0, width: 800, height: 600 }, { left: 0, top: 0, width: 400, height: 300 })).toBe("translate(0px, 0px) scale(2, 2)");
  expect(flipFrom({ left: 400, top: 0, width: 400, height: 300 }, { left: 0, top: 300, width: 400, height: 300 })).toBe("translate(400px, -300px) scale(1, 1)");
  expect(flipFrom({ left: 0, top: 0, width: 400, height: 300 }, { left: 0, top: 0, width: 400, height: 300 })).toBeUndefined();
  expect(flipFrom({ left: 0, top: 0, width: 400, height: 300 }, { left: 0, top: 0, width: 0, height: 0 })).toBeUndefined();
});

it("a child re-keyed by the daemon is one pane, not a ghost left unconfirmed", () => {
  const pending = { id: "antigravity:c1:pending", child: "c1", status: "working" as const, output: "waiting" as const };
  const run = { id: "antigravity:c1:0", child: "c1", status: "working" as const, output: "waiting" as const };
  expect(liveInvocations([pending], snapshot([run])).map((c) => c.id)).toEqual(["antigravity:c1:0"]);
});
