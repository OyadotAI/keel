// The weight budget from the plan, as a failing check rather than a number somebody remembers.
// Run after `vite build`. Gzip sizes, because that is what is downloaded and parsed at launch.
import { readdirSync, readFileSync } from "node:fs";
import { gzipSync } from "node:zlib";

const BUDGET = {
  // Everything the window needs to draw its first frame.
  initial: 400 * 1024,
  // Any one lazily loaded chunk (the terminal is the big one).
  chunk: 200 * 1024,
};

const dir = new URL("../dist/assets/", import.meta.url);
const html = readFileSync(new URL("../dist/index.html", import.meta.url), "utf8");
let initial = 0;
const failures = [];
for (const name of readdirSync(dir)) {
  if (!/\.(js|css)$/.test(name)) continue;
  const size = gzipSync(readFileSync(new URL(name, dir))).length;
  const atLaunch = html.includes(name);
  if (atLaunch) initial += size;
  else if (size > BUDGET.chunk) failures.push(`${name}: ${(size / 1024).toFixed(0)} KB gzip, budget ${BUDGET.chunk / 1024} KB`);
}
if (initial > BUDGET.initial) failures.push(`initial load: ${(initial / 1024).toFixed(0)} KB gzip, budget ${BUDGET.initial / 1024} KB`);
console.log(`initial load ${(initial / 1024).toFixed(0)} KB gzip (budget ${BUDGET.initial / 1024} KB)`);
if (failures.length) {
  console.error(failures.join("\n"));
  process.exit(1);
}
