// What a tool call looks like in the chat, from its input alone: an edit as a diff, a write as
// the file, a todo list as a checklist, a command as the command. The decoders already name every
// agent's tools the same (`Bash`, `Edit`, `Write` for Claude, Codex and Antigravity alike), so this
// is one reading for all three; any field missing falls back to the raw input rather than a guess.
import type { Prepared } from "./git";

/** Rows drawn before the rest is cut, the same ceiling the Turns view uses for its diffs. */
export const DIFF_ROWS = 400;

export type ToolView =
  | { kind: "diff"; path: string; rows: Prepared["rows"]; adds: number; dels: number }
  | { kind: "todos"; todos: { text: string; status: string }[] }
  | { kind: "command"; command: string }
  | { kind: "raw" };

const str = (v: unknown): string | undefined => (typeof v === "string" ? v : undefined);
const lines = (text: string) => (text === "" ? [] : text.replace(/\n$/, "").split("\n"));
/** At most the rows that will ever be drawn: a 2 MB `Write` is one call, and splitting and walking
 *  all of it on every mount of its row is the work CLAUDE.md says to do once or not at all. */
const firstRows = (text: string) => { let at = -1; for (let n = 0; n <= DIFF_ROWS; n++) { at = text.indexOf("\n", at + 1); if (at < 0) return text; } return text.slice(0, at); };
const count = (text: string) => (text === "" ? 0 : text.replace(/\n$/, "").split("\n").length);

/** One replacement as rows: shared head and tail as context, the middle as removed then added. */
function replacement(before: string, after: string): Prepared["rows"] {
  const a = lines(firstRows(before)), b = lines(firstRows(after));
  let head = 0;
  while (head < a.length && head < b.length && a[head] === b[head]) head++;
  let tail = 0;
  while (tail < a.length - head && tail < b.length - head && a[a.length - 1 - tail] === b[b.length - 1 - tail]) tail++;
  const ctx = (text: string) => ({ kind: "ctx", old: null, new: null, text });
  return [
    ...a.slice(0, head).map(ctx),
    ...a.slice(head, a.length - tail).map((text) => ({ kind: "del", old: null, new: null, text })),
    ...b.slice(head, b.length - tail).map((text) => ({ kind: "add", old: null, new: null, text })),
    ...a.slice(a.length - tail).map(ctx),
  ];
}

function diff(path: string, pairs: [string, string][]): ToolView {
  const rows: Prepared["rows"] = [];
  for (const [before, after] of pairs) {
    if (rows.length) rows.push({ kind: "hunk", text: "…" });
    rows.push(...replacement(before, after));
  }
  return { kind: "diff", path, rows, adds: rows.filter((r) => r.kind === "add").length, dels: rows.filter((r) => r.kind === "del").length };
}

/** How to draw a call, or `raw` for the JSON when its input is not the shape this knows. */
export function toolView(tool: string, input: Record<string, unknown> | undefined): ToolView {
  if (!input) return { kind: "raw" };
  const path = str(input.file_path) ?? str(input.path) ?? str(input.TargetFile) ?? "";
  if (tool === "Edit" && str(input.old_string) !== undefined && str(input.new_string) !== undefined) return diff(path, [[input.old_string as string, input.new_string as string]]);
  if (tool === "MultiEdit" && Array.isArray(input.edits)) {
    const pairs = (input.edits as Record<string, unknown>[]).filter((e) => str(e.old_string) !== undefined && str(e.new_string) !== undefined).map((e) => [e.old_string, e.new_string] as [string, string]);
    if (pairs.length) return diff(path, pairs);
  }
  if (tool === "Write" && str(input.content) !== undefined) {
    const content = input.content as string;
    return { ...diff(path, [["", firstRows(content)]]), adds: count(content) } as ToolView;
  }
  if (tool === "TodoWrite" && Array.isArray(input.todos)) {
    return { kind: "todos", todos: (input.todos as Record<string, unknown>[]).map((t) => ({ text: str(t.content) ?? str(t.activeForm) ?? "", status: str(t.status) ?? "pending" })) };
  }
  if (tool === "Bash" && str(input.command)) return { kind: "command", command: input.command as string };
  return { kind: "raw" };
}

/** Plain verbs for what a tool did, for people who never learned the tool names. */
const VERBS: Record<string, string> = {
  Bash: "Ran", Edit: "Edited", MultiEdit: "Edited", Write: "Created", Read: "Read", NotebookEdit: "Edited",
  Grep: "Searched for", Glob: "Looked for", WebFetch: "Opened", WebSearch: "Searched the web for",
  TodoWrite: "Updated the plan", Agent: "Asked a helper", Task: "Asked a helper", AskUserQuestion: "Asked you",
  ExitPlanMode: "Proposed a plan", Skill: "Used",
};
const base = (path: string) => path.split(/[\\/]/).filter(Boolean).at(-1) ?? path;

/** One line for a call: a verb a person reads without knowing the agent's tools, and what it was
 *  done to — a file by its name, a command as typed. Unknown tools keep their own name. */
export function describe(tool: string, subject: string, input: Record<string, unknown> | undefined): { verb: string; object: string } {
  const verb = VERBS[tool];
  if (!verb) return { verb: tool, object: subject };
  if (tool === "TodoWrite" || tool === "ExitPlanMode") return { verb, object: "" };
  const path = str(input?.file_path) ?? str(input?.path) ?? str(input?.notebook_path);
  if (path && ["Edit", "MultiEdit", "Write", "Read", "NotebookEdit"].includes(tool)) return { verb, object: base(path) };
  return { verb, object: subject };
}

/** What a call's state means in words; finished well says nothing at all. */
export const STATE_WORDS: Record<string, string> = { running: "working…", ok: "", failed: "didn't work", interrupted: "stopped" };
