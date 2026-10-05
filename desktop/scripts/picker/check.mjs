// The picker against a real browser: arm, aim, pick, walk to the parent, probe, disarm.
// `node scripts/picker/check.mjs` — needs Chrome; not part of the gate, which has no browser.
import { createServer } from "node:http";
import { readFile } from "node:fs/promises";
import { execFile } from "node:child_process";
import { promisify } from "node:util";
import { fileURLToPath } from "node:url";
import path from "node:path";

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "../..");
const server = createServer(async (req, res) => {
  const file = req.url === "/" ? "scripts/picker/check.html" : req.url.slice(1);
  try {
    res.end(await readFile(path.join(root, file)));
  } catch {
    res.statusCode = 404;
    res.end();
  }
}).listen(0, "127.0.0.1");
await new Promise((r) => server.on("listening", r));
const chrome = process.env.CHROME ?? "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome";
// Asynchronously: a synchronous spawn blocks the loop this server answers Chrome from.
const { stdout: dom } = await promisify(execFile)(chrome, ["--headless=new", "--disable-gpu", "--virtual-time-budget=15000", "--dump-dom", `http://127.0.0.1:${server.address().port}/`], { encoding: "utf8", timeout: 60_000 });
server.close();
const raw = dom.match(/data-result="([^"]*)"/)?.[1];
if (!raw) throw new Error("the page reported nothing — the picker threw");
const out = JSON.parse(raw.replace(/&quot;/g, '"').replace(/&amp;/g, "&").replace(/&lt;/g, "<").replace(/&gt;/g, ">"));
if (out.error) throw new Error(`the check page threw: ${out.error}`);
const want = {
  "page never sees a click while picking": out.pageClicks === 0,
  "hover names the element and its source": /^button \d+×\d+ · Hero\.tsx$/.test(out.hoverLabel),
  "↑ walks to the parent": out.afterUp?.startsWith("div "),
  "the selector matches exactly one node": out.unique === 1 && out.pick.exact,
  "framework hash classes are dropped, Tailwind's kept": out.parentSelector === "div.card.hover\\:bg-blue",
  "the inspector attribute is the source": out.pick.sources[0]?.file === "src/Hero.tsx",
  "an untouched element compares equal": out.sameBefore,
  "a changed element compares different": out.changedAfter,
  "a missing element is reported missing": out.goneFound === false,
  "Esc disarms and the page clicks again": out.disarmMsg?.on === false && out.pageClicksAfterDisarm === 1,
  "a pick two frames down reaches the top": out.deepPicked === true,
};
const failed = Object.entries(want).filter(([, ok]) => !ok);
for (const [name, ok] of Object.entries(want)) console.log(`${ok ? "ok  " : "FAIL"} ${name}`);
if (failed.length) process.exit(1);
