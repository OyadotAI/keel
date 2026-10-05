// What git says about a lane's checkout, and diffs prepared once for drawing.

export interface Change {
  path: string;
  status: string;
  label: string;
  staged: boolean;
  dir: boolean;
}

export interface GitStatus {
  is_repo: boolean;
  branch: string | null;
  changes: Change[];
  collapsed?: boolean;
}

export interface DiffLine {
  kind: "add" | "del" | "ctx" | string;
  old: number | null;
  new: number | null;
  text: string;
}

export interface DiffResponse {
  path: string;
  hunks: { header: string; lines: DiffLine[] }[];
  untracked: boolean;
  note?: string | null;
}

/// A diff flattened into rows once, when it arrives — counts, keys and all. The Swift view
/// recomputed every one of these in its body, per render, per line, which on a 20,000-line file
/// was the beachball.
export interface Prepared {
  path: string;
  rows: ({ kind: "hunk"; text: string } | ({ kind: string } & DiffLine))[];
  adds: number;
  dels: number;
  note?: string | null;
}

export function prepare(d: DiffResponse): Prepared {
  const rows: Prepared["rows"] = [];
  let adds = 0;
  let dels = 0;
  for (const h of d.hunks) {
    rows.push({ kind: "hunk", text: h.header });
    for (const l of h.lines) {
      if (l.kind === "add") adds++;
      else if (l.kind === "del") dels++;
      rows.push(l);
    }
  }
  return { path: d.path, rows, adds, dels, note: d.note };
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
