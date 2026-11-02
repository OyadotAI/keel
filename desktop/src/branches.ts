// Branch discovery uses locally recorded refs and deterministic search/filter/sort rules.
export interface Branch { name: string; current: boolean; upstream?: string | null; ahead: number; behind: number; subject?: string; when?: number | null; worktree?: { path: string; keel?: string | null } | null }
export interface RemoteBranch { name: string; remote: string; subject?: string; when?: number | null; tracking: string[] }
export interface Branches { current?: string | null; head?: string | null; local: Branch[]; remote?: string[]; remote_details?: RemoteBranch[]; remotes: string[]; staged: number; unstaged: number; restriction?: string | null }
export type BranchFilter = "all" | "local" | "remote";
export type BranchSort = "name" | "commit";
export type BranchRow = Branch & { kind: "local" | "remote"; remote?: string; tracking?: string[] };
export type GitAction = (message: string, run: () => Promise<unknown>) => Promise<boolean>;
const MILLISECONDS_PER_SECOND = 1000;
const COMMIT_UNITS: [Intl.RelativeTimeFormatUnit, number][] = [["day", 86400], ["hour", 3600], ["minute", 60]];

function compareName(a: BranchRow, b: BranchRow): number {
  const left = a.name.toLowerCase(), right = b.name.toLowerCase();
  return left < right ? -1 : left > right ? 1 : a.name < b.name ? -1 : a.name > b.name ? 1 : a.kind.localeCompare(b.kind);
}

/** Count the matching current branch but render it only once in its pinned summary. */
export function branchResults(data: Branches, query: string, filter: BranchFilter, sort: BranchSort): { rows: BranchRow[]; count: number } {
  const local: BranchRow[] = data.local.map((b) => ({ ...b, kind: "local" }));
  const remote: BranchRow[] = (data.remote_details ?? []).map((b) => ({ ...b, current: false, ahead: 0, behind: 0, kind: "remote", worktree: b.tracking.length === 1 ? data.local.find((local) => local.name === b.tracking[0] && !local.current)?.worktree : undefined }));
  const needle = query.trim().toLowerCase();
  const rows = [...local, ...remote].filter((b) => (filter === "all" || b.kind === filter) && b.name.toLowerCase().includes(needle));
  rows.sort((a, b) => sort === "commit" ? (b.when ?? -Infinity) - (a.when ?? -Infinity) || compareName(a, b) : compareName(a, b));
  return { count: rows.length, rows: rows.filter((b) => !b.current) };
}

/** Commit recency describes Git's recorded timestamp rather than suggesting a checkout history. */
export function branchCommitAge(when: number, now = Date.now()): string {
  const elapsed = (when * MILLISECONDS_PER_SECOND - now) / MILLISECONDS_PER_SECOND;
  const [unit, seconds] = COMMIT_UNITS.find(([, length]) => Math.abs(elapsed) >= length) ?? COMMIT_UNITS[COMMIT_UNITS.length - 1];
  return new Intl.RelativeTimeFormat(undefined, { numeric: "auto", style: "short" }).format(Math.round(elapsed / seconds), unit);
}

/** Match verified Keel worktree identity rather than assuming a branch name owns a lane. */
export function branchOwner(branch: BranchRow, project: string, lanes: { id: string; project: string; wt?: string; title: string }[]): { id: string; title: string } | undefined {
  if (!branch.worktree?.keel) return;
  return lanes.find((lane) => lane.project === project && lane.wt === branch.worktree?.keel);
}

/** Preserve unknown Git failures while adding actionable guidance for its known conflict refusal. */
export function branchFailure(error: unknown): string {
  const text = error instanceof Error ? error.message : String(error);
  return /would be overwritten|would be overwritten by checkout/i.test(text) ? `Couldn’t switch: local changes would be overwritten. Commit or stash them, then try again.\n${text}` : text;
}
