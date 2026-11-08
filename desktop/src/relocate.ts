// Moving a project into the Keel workspace: what the daemon's answer means in words, which
// projects the person said to leave, and the one pure step that re-files the app's state.
import type { Lane, Project } from "./store";

/// Why the daemon will not move a project, by its code (`relocate.rs`, `Refusal`).
export type Refusal =
  | { code: "inside" } | { code: "contains" } | { code: "link"; target: string } | { code: "worktree"; main: string } | { code: "linked"; path: string }
  | { code: "submodule"; parent: string } | { code: "nested"; root: string } | { code: "taken"; to: string }
  | { code: "volume" } | { code: "turn"; lane: string } | { code: "terminal" } | { code: "live" } | { code: "dev" }
  | { code: "job"; command: string } | { code: "workspace"; error: string } | { code: "rename"; error: string }
  | { code: "repair"; error: string };

export interface MovePlan { from: string; to: string; refusal?: Refusal | null }
export interface Moved { from: string; to: string; warnings: { step: string; error: string }[] }

/// What the move dialog is showing.
export interface Moving {
  from: string;
  to: string;
  phase: "offer" | "moving" | "moved" | "refused";
  refusal?: Refusal;
  warnings?: Moved["warnings"];
  /// Offered on open: no lane exists yet, and one is made once the person answers.
  auto: boolean;
}

/// Refusals that are a property of the folder, not of the moment: never offered on open, since
/// a submodule would otherwise ask every time it is opened. The menu item still says them.
export const structural = (r?: Refusal | null) => !!r && ["inside", "contains", "link", "worktree", "linked", "submodule", "nested", "volume"].includes(r.code);

/// One sentence per refusal. `title` names a lane by what the person called it.
export function refusalText(r: Refusal, from: string, workspace: string, name: string, title: (lane: string) => string | undefined): string {
  switch (r.code) {
    case "inside": return "This project is already in the Keel workspace.";
    case "contains": return `${from} holds the Keel workspace itself, so it stays where it is.`;
    case "link": return `${from} is a link to ${r.target} — open the real folder and move that.`;
    case "worktree": return `This folder is a worktree of ${r.main}, so it stays with that repository.`;
    case "linked": return `This project has a git worktree of its own at ${r.path}. Moving the project would break it, so the project stays where it is — remove that worktree first (git worktree remove) to move it.`;
    case "submodule": return `This folder is a submodule of ${r.parent}, so it stays with that repository.`;
    case "nested": return `This folder is inside the repository at ${r.root}, so it stays with that repository.`;
    case "taken": return `There is already a “${name}” in the Keel workspace — rename one of them, then move from the project menu.`;
    case "volume": return `${from} is on a different disk from the Keel workspace; Keel moves within one disk — move it yourself, then open it from there.`;
    case "turn": { const t = r.lane ? title(r.lane) : undefined; return t ? `A turn is running in “${t}” — finish or stop it first.` : "Keel is writing to this project — try again in a moment."; }
    case "terminal": return "A terminal is open in this project — close or stop it first.";
    case "live": return "Claude Code is running in this folder outside Keel — quit it first.";
    case "dev": return "The dev server is running — stop it first.";
    case "job": return `The background job “${r.command}” is running — stop it first.`;
    case "workspace": return `Keel can’t write to ${workspace}: ${r.error}.`;
    case "rename": return `The move failed: ${r.error}. Nothing was moved.`;
    case "repair": return `Keel couldn’t relink the lane checkouts: ${r.error}. Nothing was moved.`;
  }
}

const STEP: Record<string, string> = {
  trust: "its trust and rules couldn’t be carried over — trust it again from the status bar",
  vouched: "Keel’s record of the plugins and servers it installed couldn’t be carried over",
  sessions: "Codex and Antigravity conversations couldn’t be re-filed in History",
  record: "Keel couldn’t note the old path, so Codex and Antigravity History may miss older conversations",
  history: "Claude Code’s history couldn’t be renamed; earlier conversations stay under the old path",
};
export const warningText = (w: { step: string; error: string }) => `Moved, but ${STEP[w.step] ?? w.step}: ${w.error}.`;

const DECLINED = "keel.workspace.declined";
/// The projects the person said to keep where they are, on this device. Not in the repository:
/// a decision about where a folder lives on one machine is not a property of the repository.
export function declined(): string[] {
  try { const v = JSON.parse(localStorage.getItem(DECLINED) ?? "[]"); return Array.isArray(v) ? v.filter((p) => typeof p === "string") : []; } catch { return []; }
}
export function decline(path: string, on = true) {
  const next = declined().filter((p) => p !== path).concat(on ? [path] : []);
  try { localStorage.setItem(DECLINED, JSON.stringify(next)); } catch { /* private window: asked again next time */ }
}

type Keyed = Record<string, unknown>;
interface Rekeyable { order: string[]; projects: Record<string, Project>; lanes: Record<string, Lane>; trusted: Keyed; setup: Keyed; setupRun: Keyed; readiness: Keyed; handed: Keyed }

/// Every map keyed by a project's path, re-filed under where it moved. Pure, so the one thing a
/// move can get quietly wrong — a map left under the old path — has a test.
export function rekey<T extends Rekeyable>(s: T, from: string, to: string): Pick<T, keyof Rekeyable> {
  const move = <V,>(m: Record<string, V>): Record<string, V> => Object.fromEntries(Object.entries(m).map(([k, v]) => [k === from ? to : k, v]));
  const name = to.split(/[\\/]/).filter(Boolean).pop() ?? to;
  const projects = move(s.projects);
  if (projects[to]) projects[to] = { ...projects[to], path: to, name, endpoint: undefined, starting: false, error: undefined, sessions: undefined };
  const lanes = Object.fromEntries(Object.entries(s.lanes).map(([id, l]) => [id, l.project === from ? { ...l, project: to, loaded: false } : l]));
  return {
    order: s.order.map((p) => (p === from ? to : p)),
    projects, lanes,
    trusted: move(s.trusted), setup: move(s.setup), setupRun: move(s.setupRun), readiness: move(s.readiness), handed: move(s.handed),
  } as Pick<T, keyof Rekeyable>;
}

/// The editor's unsaved drafts, keyed by `[project, wt, path]`, re-filed the same way.
export function rekeyDrafts(files: Record<string, unknown>, from: string, to: string): Record<string, unknown> {
  return Object.fromEntries(Object.entries(files).map(([k, v]) => {
    try { const [p, wt, path] = JSON.parse(k) as [string, string, string]; return [p === from ? JSON.stringify([to, wt, path]) : k, v]; } catch { return [k, v]; }
  }));
}
