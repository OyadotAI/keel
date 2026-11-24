// What git says about a lane's checkout, and diffs prepared once for drawing.

export interface Change {
  path: string;
  status: string;
  label: string;
  staged: boolean;
  dir: boolean;
}

/** Porcelain has independent index and working-tree columns; a file may belong to both. */
export function inArea(change: Change, staged: boolean): boolean {
  if (change.status === "??") return !staged;
  if (staged) return change.staged;
  return change.status.length > 1 ? change.status[1] !== " " : !change.staged;
}

export interface GitStatus {
  is_repo: boolean;
  branch: string | null;
  changes: Change[];
  collapsed?: boolean;
  /** `HEAD`'s commit. */
  head?: string | null;
  /** Commits the base lacks; absent where git cannot say. */
  ahead?: number | null;
  /** Changes a commit would take: `changes` without what Keel keeps out of commits. */
  unsaved?: number | null;
  /** The pull request opened from this branch, and the commit GitHub has for it. */
  pr?: { url: string; head: string } | null;
  /** Each repository in the opened folder; one with an empty `dir` when the folder is the repository. */
  repos?: { dir: string; branch: string | null }[];
}

/// A folder that holds several repositories rather than being one: branches, history and sync
/// belong to each repository, and asking git about the folder itself only fails.
export const isWorkspace = (git?: GitStatus) => (git?.repos?.length ?? 0) > 1 || !!git?.repos?.[0]?.dir;

export interface DiffLine {
  kind: "add" | "del" | "ctx" | string;
  old: number | null;
  new: number | null;
  text: string;
}

export interface DiffResponse {
  identity?: import("./delivery").CheckoutIdentity | null;
  path: string;
  /// `id` is set only on a hunk of the unstaged diff shown whole — what Stage and Discard send.
  hunks: { header: string; lines: DiffLine[]; id?: string }[];
  untracked: boolean;
  note?: string | null;
}

/// A diff flattened into rows once, when it arrives — counts, keys and all. The Swift view
/// recomputed every one of these in its body, per render, per line, which on a 20,000-line file
/// was the beachball.
export interface Prepared {
  identity?: import("./delivery").CheckoutIdentity | null;
  path: string;
  rows: (HunkRow | ({ kind: string } & DiffLine))[];
  adds: number;
  dels: number;
  note?: string | null;
}

/// A hunk header row; `id`, `n` and `of` let it be staged or discarded on its own.
export interface HunkRow { kind: "hunk"; text: string; id?: string; n?: number; of?: number; first?: number; last?: number }

/// The new side's first and last line from a `@@ -a,b +c,d @@` header, for "Lines c–d".
export function hunkLines(header: string): { first: number; last: number } {
  const m = /\+(\d+)(?:,(\d+))?/.exec(header);
  const first = m ? Number(m[1]) : 0;
  const count = m?.[2] === undefined ? 1 : Number(m[2]);
  return { first, last: first + Math.max(count, 1) - 1 };
}

export function prepare(d: DiffResponse): Prepared {
  const rows: Prepared["rows"] = [];
  let adds = 0;
  let dels = 0;
  for (const [n, h] of d.hunks.entries()) {
    rows.push({ kind: "hunk", text: h.header, id: h.id, n: n + 1, of: d.hunks.length, ...hunkLines(h.header) });
    for (const l of h.lines) {
      if (l.kind === "add") adds++;
      else if (l.kind === "del") dels++;
      rows.push(l);
    }
  }
  return { path: d.path, rows, adds, dels, note: d.note, identity: d.identity };
}

/// The daemon's limit on a lane name (`worktree::valid_name`), less room for a `-xyz` that
/// breaks a collision.
const NAME = 41 - 4;

/// A branch name from what was asked: the name you would look for in `git log` later.
export function slug(text: string): string {
  const words = text
    .toLowerCase()
    .replace(/[^a-z0-9]+/g, "-")
    .replace(/^-+|-+$/g, "")
    .split("-")
    .filter(Boolean)
    .slice(0, 5);
  let s = "";
  for (const w of words) {
    const next = s ? `${s}-${w}` : w;
    if (next.length > NAME) break;
    s = next;
  }
  return s || words[0]?.slice(0, NAME) || "lane";
}
