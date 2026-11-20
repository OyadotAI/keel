// Tests for terminal-follow.ts: who moved the scroll position decides whether it is kept.
import { expect, test } from "vitest";
import { nextFollow } from "./terminal-follow";

test("a scroll the person made is kept, and scrolling back down follows again", () => {
  expect(nextFollow(true, false, true)).toEqual({ following: false, toBottom: false });
  expect(nextFollow(false, true, true)).toEqual({ following: true, toBottom: false });
});

test("a jump nobody made is undone while following, and left alone once the person scrolled up", () => {
  expect(nextFollow(true, false, false)).toEqual({ following: true, toBottom: true });
  expect(nextFollow(false, false, false)).toEqual({ following: false, toBottom: false });
  expect(nextFollow(true, true, false)).toEqual({ following: true, toBottom: false });
});
