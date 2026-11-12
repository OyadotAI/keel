// What an approval card asks, in words a person who has never used a terminal can answer.
//
// The chat lane is for people coming from Lovable: "wants to run `pnpm add zod`" is a question they
// cannot judge, so they either allow everything or get stuck. Each card instead says what will
// happen ("Install the packages your app needs?"), with the exact command kept behind Details.
//
// The words never make a request look safer than it is. A command that chains or redirects
// (`npm i && curl … | sh`), escalates (`sudo`), or reaches outside the project gets the plain
// fallback and opens its details, because a family sentence would describe only its first part.

/** What a card says, and whether its details should already be open. */
export interface Summary {
  title: string;
  /** Show the command or path straight away: the sentence alone is not enough to judge it. */
  open: boolean;
}

/** The parts of a pending request the summary reads — the same for every agent's card. */
export interface Asked {
  tool: string;
  command?: string;
  input?: Record<string, unknown>;
}

const UNKNOWN: Summary = { title: "Run a command on your computer?", open: true };

/** Shell syntax that runs more than one program or sends output somewhere. */
const COMPOUND = /[;&|$`<>()\n\\]/;
const PACKAGE_MANAGERS = new Set(["npm", "pnpm", "yarn", "bun"]);
const INSTALL_WORDS = new Set(["install", "i", "add", "ci"]);
const DEV_SCRIPTS = new Set(["dev", "start", "serve", "preview"]);
const TEST_SCRIPTS = new Set(["test", "check", "lint", "typecheck"]);
const BUILD_SCRIPTS = new Set(["build", "compile"]);
const DEPLOY_SCRIPTS = new Set(["deploy", "release", "publish"]);
const READS = new Set(["ls", "cat", "head", "tail", "wc", "pwd", "grep", "rg", "find", "tree", "which"]);
const GIT_LOOKS = new Set(["status", "diff", "log", "show", "blame"]);
const GIT_SAVES = new Set(["add", "commit", "stash"]);
const GIT_UNDOES = new Set(["reset", "checkout", "clean", "rebase", "restore", "revert", "switch"]);

const say = (title: string, open = false): Summary => ({ title, open });

/** Flags that turn a looking or saving command into one that runs, deletes or writes elsewhere:
 *  `find -exec`/`-delete`, `rg --pre`, git's `-c`, `--output`, `--upload-pack`. A family sentence
 *  ("Look through your project's files?") would hide exactly that. */
const RISKY_FLAG = /^(-exec|-execdir|-ok|-okdir|-delete|-fprint\w*|-fls|-c|-C|--pre|--output|--upload-pack|--receive-pack|--exec|--config)(=.*)?$/;

/** Whether any argument could do more than its program's plain sentence says: a risky flag, or a
 *  path given inside a flag (`--prefix=/elsewhere`) that leaves the project. */
function beyondItsWords(args: string[], root?: string): boolean {
  return args.some((a) => {
    if (RISKY_FLAG.test(a)) return true;
    const value = a.startsWith("-") && a.includes("=") ? a.slice(a.indexOf("=") + 1) : undefined;
    return !!value && (value.includes("/") || value.startsWith("~")) && !inside(value, root);
  });
}

/** The words of a command once leading `NAME=value` settings are set aside. */
function words(command: string): string[] {
  const all = command.trim().split(/\s+/).filter(Boolean);
  let i = 0;
  while (i < all.length && /^[A-Za-z_][A-Za-z0-9_]*=\S*$/.test(all[i])) i++;
  return all.slice(i);
}

/** Whether a path a command names stays in the project: relative and never climbing out, or
 *  absolute under the project's own folder. */
export function inside(path: string, root?: string): boolean {
  if (path.startsWith("~") || path === "..") return false;
  if (path.startsWith("/")) return !!root && (path === root || path.startsWith(root.replace(/\/$/, "") + "/")) && !path.split("/").includes("..");
  return !path.split("/").includes("..");
}

/** A package manager's script name: `npm run dev`, `pnpm dev`, `yarn test`. */
function script(w: string[]): string | undefined {
  if (!PACKAGE_MANAGERS.has(w[0])) return undefined;
  if (w[1] === "run") return w[2];
  return w[1];
}

/** A command's words, split once: the program, its first argument, and what it names. */
type Words = { w: string[]; first: string; second?: string; paths: string[] };
type Family = (c: Words) => Summary | undefined;

/** Installing: the app's packages, or a tool for the whole computer. */
const installs: Family = ({ w, first, second }) => {
  if (PACKAGE_MANAGERS.has(first) && second && INSTALL_WORDS.has(second)) {
    return w.includes("-g") || w.includes("--global") ? say("Install a tool on your computer?", true) : say("Install the packages your app needs?");
  }
  if ((first === "pip" || first === "pip3") && second === "install") return say("Install the packages your app needs?");
  if (first === "brew" || ((first === "cargo" || first === "go") && second === "install")) return say("Install a tool on your computer?", true);
  return undefined;
};

/** A package script by its name: dev, test, build, deploy. */
const scripts: Family = ({ w }) => {
  const name = script(w);
  if (!name) return undefined;
  if (DEV_SCRIPTS.has(name)) return say("Start your app so you can preview it?");
  if (TEST_SCRIPTS.has(name)) return say("Check that your app still works?");
  if (BUILD_SCRIPTS.has(name)) return say("Build your app to check for errors?");
  if (DEPLOY_SCRIPTS.has(name)) return say("Put your app online?", true);
  return undefined;
};

/** The usual tools run by their own names. */
const TOOLS: [(first: string, second?: string) => boolean, Summary][] = [
  [(f, s) => ["vitest", "jest", "pytest", "playwright"].includes(f) || (["cargo", "go", "make"].includes(f) && (s === "test" || s === "check")), say("Check that your app still works?")],
  [(f, s) => f === "tsc" || ((f === "cargo" || f === "make") && s === "build") || ((f === "next" || f === "vite") && s === "build"), say("Build your app to check for errors?")],
  [(f, s) => (f === "next" || f === "vite") && (s === undefined || s === "dev"), say("Start your app so you can preview it?")],
  [(f, s) => (f === "wrangler" || f === "fly") && s === "deploy" || f === "vercel", say("Put your app online?", true)],
  [(f) => READS.has(f), say("Look through your project's files?")],
  [(f) => f === "mkdir" || f === "touch", say("Add files to your project?")],
];
const tools: Family = ({ first, second }) => TOOLS.find(([test]) => test(first, second))?.[1];

/** Deleting files: counted, and only ever inside the project (checked before this runs). */
const deletes: Family = ({ first, paths }) => {
  if (first !== "rm") return undefined;
  // `.`, `*` and the like are the whole folder, not "1 file".
  if (paths.some((p) => p === "." || p === "./" || p.includes("*"))) return UNKNOWN;
  return paths.length ? say(`Delete ${paths.length} ${paths.length === 1 ? "file" : "files"} from your project?`, true) : UNKNOWN;
};

/** Downloading: named by the site it reaches. */
const downloads: Family = ({ w, first }) => {
  if (first !== "curl" && first !== "wget") return undefined;
  const site = w.map((a) => { try { return new URL(a).host; } catch { return ""; } }).find(Boolean);
  return site ? say(`Download something from ${site}?`, true) : UNKNOWN;
};

const FAMILIES: Family[] = [installs, scripts, tools, ({ w, first }) => (first === "git" ? gitWords(w) : undefined), deletes, downloads];

/** The plain question for one shell command, or the fallback when it cannot be read safely. */
export function commandWords(command: string, root?: string): Summary {
  if (!command.trim() || COMPOUND.test(command)) return UNKNOWN;
  // A leading `NAME=value` changes what runs (`NODE_OPTIONS=--require=…`): never a family sentence.
  if (words(command).length !== command.trim().split(/\s+/).length) return UNKNOWN;
  const w = words(command);
  const [first, second] = w;
  if (!first || first === "sudo" || first === "su") return UNKNOWN;
  const paths = w.slice(1).filter((a) => !a.startsWith("-"));
  if (paths.some((p) => !inside(p, root))) return UNKNOWN;
  if (beyondItsWords(w.slice(1), root)) return UNKNOWN;
  const c = { w, first, second, paths };
  for (const family of FAMILIES) {
    const said = family(c);
    if (said) return said;
  }
  return UNKNOWN;
}

/** Git, which can look, save, send or throw work away — said as which of those it is. */
function gitWords(w: string[]): Summary {
  const sub = w[1];
  if (GIT_LOOKS.has(sub) || (sub === "branch" && w.length === 2)) return say("Look at your project's history?");
  if (GIT_SAVES.has(sub)) return say("Save a version of your project?");
  if (sub === "push") {
    return w.some((a) => a === "-f" || a.startsWith("--force")) ? say("Replace the versions saved on GitHub? This can lose changes.", true) : say("Send your saved versions to GitHub?");
  }
  if (sub === "pull" || sub === "fetch") return say("Get the latest versions from GitHub?");
  if (GIT_UNDOES.has(sub)) return say("Undo saved work? This can lose changes.", true);
  return UNKNOWN;
}

/** The host of a web address, or the address itself when it will not parse. */
function host(url: unknown): string {
  try { return new URL(String(url)).host; } catch { return String(url ?? "a web page"); }
}

/** `create_issue` → "create issue". */
const spoken = (name: string) => name.replace(/[_-]+/g, " ").trim();

/** The plain question for any approval card — every agent's: Claude's hook and its SDK cards and
 *  Antigravity's hook card all name Claude's tools; Codex's App Server says `Command`, `File
 *  changes` or `MCP input`. */
export function approvalWords(asked: Asked, root?: string): Summary {
  const input = asked.input ?? {};
  const tool = asked.tool;
  if (tool === "Bash" || tool === "Command") {
    const command = asked.command || (typeof input.command === "string" ? input.command : Array.isArray(input.command) ? input.command.join(" ") : "");
    return commandWords(command, root);
  }
  if (["Write", "Edit", "MultiEdit", "NotebookEdit"].includes(tool)) {
    const path = String(input.file_path ?? input.notebook_path ?? "");
    if (!path) return say("Change a file in your project?", true);
    if (!inside(path, root)) return say("Change a file outside this project?", true);
    return say(`Change ${path.split("/").pop()}?`);
  }
  if (tool === "File changes") return say("Change files in your project?");
  if (tool === "WebFetch") return say(`Open ${host(input.url)}?`);
  if (tool === "WebSearch") return say(`Search the web for “${String(input.query ?? "")}”?`);
  const browser = /^mcp__keel__browser_(\w+)$/.exec(tool);
  if (browser) return say(`Use the browser to ${spoken(browser[1])}?`);
  const mcp = /^mcp__([^_]+(?:_[^_]+)*)__(.+)$/.exec(tool);
  if (mcp) return say(`Use ${spoken(mcp[1])} to ${spoken(mcp[2])}?`, true);
  return say(`Let it use ${tool}?`, true);
}

/** The answers a card offers, in the chat lane's words. Codex's App Server sends its own
 *  decisions; Keel's cards send allow / session / deny. */
export const ANSWER_WORDS: Record<string, string> = {
  allow: "Allow",
  accept: "Allow",
  session: "Allow for this chat",
  acceptForSession: "Allow for this chat",
  deny: "Don't allow",
  decline: "Don't allow",
  cancel: "Stop this message",
};
