import { expect, it } from "vitest";
import { hunkLines, isWorkspace, prepare, slug } from "./git";

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

it("a hunk row carries the daemon's id and its place, so it can be staged on its own", () => {
  const p = prepare({ path: "a.ts", untracked: false, hunks: [
    { header: "@@ -1 +1,2 @@", lines: [], id: "abc" },
    { header: "@@ -9,3 +10,4 @@", lines: [] },
  ] });
  expect(p.rows[0]).toMatchObject({ kind: "hunk", id: "abc", n: 1, of: 2, first: 1, last: 2 });
  expect(p.rows[1]).toMatchObject({ kind: "hunk", id: undefined, n: 2, of: 2, first: 10, last: 13 });
});

it("a hunk header with no count is one line", () => {
  expect(hunkLines("@@ -7 +7 @@ fn x")).toEqual({ first: 7, last: 7 });
  expect(hunkLines("@@ -1,0 +0,0 @@")).toEqual({ first: 0, last: 0 });
});

it("a folder holding several repositories is a workspace; a repository opened at its root is not", () => {
  const base = { is_repo: true, branch: null, changes: [] };
  expect(isWorkspace({ ...base, repos: [{ dir: "", branch: "main" }] })).toBe(false);
  expect(isWorkspace({ ...base, repos: [{ dir: "api", branch: "main" }] })).toBe(true);
  expect(isWorkspace({ ...base, repos: [{ dir: "a", branch: "main" }, { dir: "b", branch: null }] })).toBe(true);
  expect(isWorkspace(base)).toBe(false);
});
