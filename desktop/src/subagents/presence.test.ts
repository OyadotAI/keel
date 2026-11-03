// Presence reconciliation keeps each tile in the committed tree through its own close deadline.
import { expect, it } from "vitest";
import { retainDepartures, retireDepartures, withDeparting } from "./presence";
import type { Invocation } from "./model";
const child = (id: string): Invocation => ({ id, status: "working", output: "waiting" });
const one = child("one"), two = child("two"), three = child("three");
const initial = () => ({ scope: "parent", children: [one, two, three], gone: [], ms: 260 });

it("a removed child is returned as leaving in the same reconciliation, with no absent render", () => {
  const next = retainDepartures(initial(), "parent", [one, three], 260, 1000);
  expect(withDeparting(next.children, next.gone).map((c) => [c.id, !!c.leaving])).toEqual([["one", false], ["two", true], ["three", false]]);
  expect(next.gone[0].child).toBe(two);
  expect(next.gone[0].until).toBe(1260);
});
it("overlapping completions preserve order and independent deadlines", () => {
  const first = retainDepartures(initial(), "parent", [two, three], 260, 1000);
  const next = retainDepartures(first, "parent", [three], 260, 1100);
  expect(withDeparting(next.children, next.gone).map((c) => c.id)).toEqual(["one", "two", "three"]);
  expect(next.gone.map((entry) => entry.until)).toEqual([1260, 1360]);
  const retired = retireDepartures(next, 1260);
  expect(withDeparting(retired.children, retired.gone).map((c) => c.id)).toEqual(["two", "three"]);
});
it("revival, scope replacement and reduced motion revoke retained departures", () => {
  const closing = retainDepartures(initial(), "parent", [one, three], 260, 1000);
  expect(retainDepartures(closing, "parent", [one, two, three], 260, 1100).gone).toEqual([]);
  expect(retainDepartures(closing, "replacement", [], 260, 1100).gone).toEqual([]);
  expect(retainDepartures(closing, "parent", [one, three], 0, 1100).gone).toEqual([]);
});
