// The chat turn footer says only what the agent reported.
import { describe, expect, it } from "vitest";
import { duration, turnFacts } from "./turn-facts";

describe("turnFacts", () => {
  it("says duration, tokens, cost and context", () => {
    expect(turnFacts({ context: 42_000, input: 1200, output: 340, cache_read: 30_000, cache_write: 0, cost: 0.0421 }, 83_000)).toEqual(["1m 23s", "31k in · 340 out", "$0.042", "42k context"]);
  });
  it("says nothing it was not told", () => {
    expect(turnFacts(undefined, undefined)).toEqual([]);
  });
});

describe("duration", () => {
  it("reads as part of a sentence", () => {
    expect(duration(83_000)).toBe(" in 1m 23s");
    expect(duration(undefined)).toBe("");
    expect(duration(119_700)).toBe(" in 2m 0s");
  });
});
