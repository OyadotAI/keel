import { expect, it } from "vitest";
import { prepare, slug } from "./git";

it("prepares a diff once: rows, counts", () => {
  const p = prepare({
    path: "a.ts",
    untracked: false,
    hunks: [{ header: "@@ -1 +1,2 @@", lines: [
      { kind: "del", old: 1, new: null, text: "a" },
      { kind: "add", old: null, new: 1, text: "b" },
      { kind: "add", old: null, new: 2, text: "c" },
    ] }],
  });
  expect(p.rows).toHaveLength(4);
  expect([p.adds, p.dels]).toEqual([2, 1]);
});

it("names a branch for the ask", () => {
  expect(slug("Fix the login bug on Safari, please!")).toBe("fix-the-login-bug-on");
  expect(slug("???")).toBe("lane");
  // Within the daemon's 41 with room for a `-xyz` collision suffix.
  expect(slug("Refactor the authentication middleware implementation").length).toBeLessThanOrEqual(37);
  expect(slug("a".repeat(80))).toHaveLength(37);
});

import { frameable } from "./api";

it("frames this machine's dev servers and nothing else", () => {
  expect(frameable("http://0.0.0.0:8000/")).toBe("http://127.0.0.1:8000/");
  expect(frameable("https://localhost:5173")).toBe("https://localhost:5173/");
  expect(frameable("http://192.168.1.4:3000")).toBeNull();
  expect(frameable("not a url")).toBeNull();
});

import { clean } from "./typers";

it("strips what could break out of a bracketed paste", () => {
  expect(clean("ok\x1b[201~rm -rf ~\r")).toBe("ok[201~rm -rf ~");
  expect(clean("a\tb\nc")).toBe("a\tb\nc");
});

import { score } from "./ui/Palette";

it("finds a command by the starts of its words", () => {
  expect(score("nlb", "New lane on its own branch")).not.toBeNull();
  expect(score("xyz", "New lane")).toBeNull();
  const starts = score("nl", "New lane")!.score;
  const middle = score("nl", "manlike")!.score;
  expect(starts).toBeGreaterThan(middle);
});

import { matches } from "./keys";

it("matches a chord by its physical key, so shifted brackets still work", () => {
  const e = (init: Partial<KeyboardEvent>) => ({ metaKey: false, ctrlKey: false, shiftKey: false, altKey: false, ...init }) as KeyboardEvent;
  const mac = /Mac/.test(navigator.platform);
  const mod = mac ? { metaKey: true } : { ctrlKey: true, shiftKey: true };
  expect(matches(e({ ...mod, code: "KeyK", key: "k" }), { key: "k" })).toBe(true);
  expect(matches(e({ code: "KeyK", key: "k" }), { key: "k" })).toBe(false);
});
