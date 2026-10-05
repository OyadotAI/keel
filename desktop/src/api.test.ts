import { expect, it } from "vitest";
import { query } from "./api";

// The daemon reads a bare `+` as a space; "C++" must arrive as "C++".
it("encodes a plus so the daemon does not read it as a space", () => {
  expect(query({ prompt: "C++ & a=b" })).toBe("?prompt=C%2B%2B%20%26%20a%3Db");
  expect(query({ a: undefined, b: "", c: 1 })).toBe("?c=1");
});
