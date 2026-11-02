// Split geometry clamps the displayed cards without mutating the reader's preferred ratio.
import { expect, it } from "vitest";
import { cardLayout, cardPreference, CARD_SPLIT_DEFAULT } from "./git-card-layout";
import { cardResizeKey } from "./git-card-resize";
it("normal minima and constrained windows preserve both usable card headers", () => {
  const large = cardLayout(1000, .95, 34, 34, true, true);
  expect(large.lower).toBeGreaterThanOrEqual(100); expect(large.upper).toBeGreaterThanOrEqual(170);
  const short = cardLayout(100, .72, 34, 34, true, true);
  expect(short.upper).toBeGreaterThanOrEqual(34); expect(short.lower).toBeGreaterThanOrEqual(34);
  const tiny = cardLayout(30, .72, 34, 34, true, true);
  expect(tiny.upper + tiny.lower + tiny.separator).toBeGreaterThan(30);
});
it("collapse reserves headers and reopening restores the preferred split", () => {
  expect(cardLayout(1000, .72, 34, 34, false, true).upper).toBe(34);
  expect(cardLayout(1000, .72, 34, 34, true, false).lower).toBe(34);
  const collapsed = cardLayout(1000, .72, 34, 34, false, false);
  expect(collapsed.upper).toBe(34); expect(collapsed.lower).toBe(34);
  expect(cardLayout(1000, .72, 34, 34, true, true).upper).toBeCloseTo(990 * .72);
});
it("keyboard steps and reset use displayed geometry and safe saved preferences", () => {
  const layout = cardLayout(1000, .6, 34, 34, true, true);
  expect(cardResizeKey(layout, "ArrowUp", false)).toBeCloseTo(.58);
  expect(cardResizeKey(layout, "ArrowDown", true)).toBeCloseTo(.7);
  expect(cardResizeKey(layout, "Home", false)).toBe(CARD_SPLIT_DEFAULT);
  for (const value of [undefined, "", "NaN", Infinity, -1, 2]) expect(cardPreference(value)).toBe(CARD_SPLIT_DEFAULT);
  expect(cardPreference("0.6")).toBe(.6);
});
