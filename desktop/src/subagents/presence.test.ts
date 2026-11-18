// A finished tile holds its slot until the burst ends, so the grid re-tracks once and not per child.
import { expect, it } from "vitest";
import { deadline, expire, holdSlots, type Presence } from "./presence";
import type { Invocation } from "./model";
const child = (id: string, status: Invocation["status"] = "working", owner?: string): Invocation => ({ id, child: owner, status, output: "waiting" });
const one = child("one"), two = child("two"), three = child("three");
const timing = { linger: 10_000, close: 560 };
const start = (...live: Invocation[]): Presence => holdSlots({ scope: "parent", slots: [] }, "parent", live, live, 0);
const row = (state: Presence) => state.slots.map((slot) => [slot.slot, slot.id, slot.status, !!slot.held, !!slot.leaving]);

it("an ended child keeps its slot and end status while a sibling works", () => {
  const next = holdSlots(start(one, two, three), "parent", [one, three], [one, child("two", "failed"), three], 1000);
  expect(row(next)).toEqual([["one", "one", "working", false, false], ["two", "two", "failed", true, false], ["three", "three", "working", false, false]]);
  expect(deadline(next, false, timing)).toBeUndefined();
});
it("a new child takes the earliest held slot under a new key", () => {
  const held = holdSlots(start(one, two), "parent", [two], [child("one", "done"), two], 1000);
  const next = holdSlots(held, "parent", [two, three], [child("one", "done"), two, three], 2000);
  expect(row(next)).toEqual([["three", "three", "working", false, false], ["two", "two", "working", false, false]]);
});
it("a re-keyed child keeps its slot and key", () => {
  const pending = child("agy:c1:pending", "working", "c1"), run = child("agy:c1:0", "working", "c1");
  const next = holdSlots(start(pending, two), "parent", [two, run], [two, run], 1000);
  expect(row(next)).toEqual([["agy:c1:pending", "agy:c1:0", "working", false, false], ["two", "two", "working", false, false]]);
});
it("a child re-keyed and ended in one snapshot is held with its end status", () => {
  const pending = child("agy:c1:pending", "working", "c1");
  const next = holdSlots(start(pending, two), "parent", [two], [two, child("agy:c1:0", "done", "c1")], 1000);
  expect(row(next)[0]).toEqual(["agy:c1:pending", "agy:c1:0", "done", true, false]);
});
it("a reused child's next turn is a new tile in the earliest held slot, not its old one", () => {
  const first = child("codex:c1:t1", "working", "c1"), second = child("codex:c1:t2", "working", "c1");
  const ended = [child("one", "done"), child("codex:c1:t1", "done", "c1"), three];
  const held = holdSlots(start(one, first, three), "parent", [three], ended, 1000);
  expect(row(holdSlots(held, "parent", [three, second], [...ended, second], 2000)).map(([slot, id]) => [slot, id])).toEqual([["codex:c1:t2", "codex:c1:t2"], ["codex:c1:t1", "codex:c1:t1"], ["three", "three"]]);
});
it("the hold ends at the linger, or at once when the lane is idle", () => {
  const quiet = holdSlots(start(one), "parent", [], [child("one", "done")], 1000);
  expect(deadline(quiet, false, timing)).toBe(11_000);
  expect(expire(quiet, 10_999, false, timing)).toBe(quiet);
  const closing = expire(quiet, 11_000, false, timing);
  expect(row(closing)).toEqual([["one", "one", "done", true, true]]);
  expect(deadline(closing, false, timing)).toBe(11_560);
  expect(expire(closing, 11_560, false, timing).slots).toEqual([]);
  expect(row(expire(quiet, 1000, true, timing))).toEqual([["one", "one", "done", true, true]]);
  expect(expire(quiet, 1000, true, { linger: 10_000, close: 0 }).slots).toEqual([]);
});
it("a child arriving during the dissolve revokes it", () => {
  const closing = expire(holdSlots(start(one, two), "parent", [], [child("one", "done"), child("two", "done")], 1000), 11_000, false, timing);
  const next = holdSlots(closing, "parent", [three], [three], 11_100);
  expect(row(next)).toEqual([["three", "three", "working", false, false], ["two", "two", "done", true, false]]);
  expect(deadline(next, false, timing)).toBeUndefined();
});
it("a replaced scope holds nothing, and a child already ended at mount takes no slot", () => {
  const held = holdSlots(start(one), "parent", [], [child("one", "done")], 1000);
  expect(holdSlots(held, "replacement", [], [], 2000).slots).toEqual([]);
  expect(holdSlots({ scope: "parent", slots: [] }, "parent", [], [child("old", "done")], 0)).toMatchObject({ slots: [], quiet: undefined });
});
