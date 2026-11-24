import { expect, it } from "vitest";
import { findCount } from "./ui/TerminalFind";

it("the find count says which match, none, or that it stopped counting", () => {
  expect(findCount("", { resultIndex: 0, resultCount: 3 })).toBe("");
  expect(findCount("x")).toBe("");
  expect(findCount("x", { resultIndex: 2, resultCount: 12 })).toBe("3 of 12");
  expect(findCount("x", { resultIndex: -1, resultCount: 0 })).toBe("No matches");
  expect(findCount("x", { resultIndex: -1, resultCount: 4 })).toBe("4 matches");
  expect(findCount("x", { resultIndex: 5, resultCount: 1000 })).toBe("1000+");
});
