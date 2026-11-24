// The design system's one rule, enforced: a colour or a token is defined in design/ and nowhere
// else. Everything outside uses the roles in tokens.css, so the brand stays five colours and a
// change to one of them is one edit.
import { expect, it } from "vitest";
import { readdirSync, readFileSync, statSync } from "node:fs";
import { join, relative } from "node:path";
import { fileURLToPath, URL } from "node:url";

const SRC = join(fileURLToPath(new URL(".", import.meta.url)), "..");
const COLOR = /#[0-9a-fA-F]{3,8}\b|\brgba?\(|\bhsla?\(/;
// A hex string that is not a colour: GitHub anchors and the like in copy.
const NOT_COLOR = /(?:issues|pull|discussion_r)[/#]|\b#\d+\b/;

function files(dir) {
  return readdirSync(dir).flatMap((name) => {
    const path = join(dir, name);
    if (statSync(path).isDirectory()) return name === "design" || name === "fonts" ? [] : files(path);
    return /\.(css|tsx?)$/.test(name) && !/\.test\.tsx?$/.test(name) ? [path] : [];
  });
}

it("no colour is written outside design/", () => {
  const found = files(SRC).flatMap((path) =>
    readFileSync(path, "utf8").split("\n").flatMap((line, i) =>
      COLOR.test(line) && !NOT_COLOR.test(line) ? [`${relative(SRC, path)}:${i + 1}: ${line.trim().slice(0, 100)}`] : []));
  expect(found).toEqual([]);
});

it("tokens are defined once, in design/tokens.css", () => {
  const found = files(SRC).filter((path) => path.endsWith(".css")).flatMap((path) =>
    [...readFileSync(path, "utf8").matchAll(/:root[^{]*\{/g)].map(() => relative(SRC, path)));
  expect(found).toEqual([]);
});
