// Presentation and findings text never decide whether a server operation succeeded.
import type { Finding, Review } from "./model";
import type { PrRun } from "./opening";
/// The branch the review compared against, as `gh` names a base: the pull request opens onto
/// the diff the verdict is about.
export const baseOf = (r: Review) => r.base.replace(/^origin\//, "");

/// The pull request a run opened, or found already open: `gh` prints its address either way —
/// on success, and in the error when one exists for the branch. Once there is one, nothing offers
/// to open another: a second click used to make a second pull request.
export function prLink(run?: { state: PrRun }): string | undefined {
  return run?.state.link ?? run?.state.lines.map((l) => l.match(/https:\/\/github\.com\/[^/\s]+\/[^/\s]+\/pull\/\d+/)?.[0]).find(Boolean);
}

/// `OyadotAI/oya-browser#44` for a pull request's address.
export function prLabel(link: string): string {
  const m = link.match(/github\.com\/([^/]+\/[^/]+)\/pull\/(\d+)/);
  return m ? `${m[1]}#${m[2]}` : link;
}

const ORDER: Record<string, number> = { blocking: 0, warning: 1, note: 2 };
export const sorted = (findings: Finding[]) => [...findings].sort((a, b) => (ORDER[a.severity] ?? 3) - (ORDER[b.severity] ?? 3));

/// A finding's path is the model's words: only a relative path inside the checkout is a link.
export const navigable = (path: string) => !!path && !path.startsWith("/") && !/^[A-Za-z]:/.test(path) && !path.split(/[\\/]/).includes("..");

/// What **Fix these** sends the lane's agent.
export function fixPrompt(r: Review, findings: Finding[]): string {
  const branch = r.wt ? `keel/${r.wt}` : "this branch";
  const list = sorted(findings)
    .map((f, i) => `${i + 1}. [${f.severity}] ${f.title} — ${f.path}:${f.line}${f.body ? `\n   ${f.body.replace(/\n/g, "\n   ")}` : ""}`)
    .join("\n\n");
  return `Keel's pre-PR review of \`${branch}\` at ${r.sha.slice(0, 7)} (against ${r.base}) found these. Fix the ones that hold; for any you disagree with, say why instead of changing code. Do not open the pull request — the person opens it from the lane after.\n\n${list}`;
}

/// "4m 15s", "38s".
export function clock(seconds: number): string {
  const m = Math.floor(seconds / 60);
  return m ? `${m}m ${seconds % 60}s` : `${seconds}s`;
}

const count = (n: number, word: string) => `${n} ${word}${n === 1 ? "" : "s"}`;
/// What a pull request carried, for somebody who has never read a git log: one commit is named,
/// several are counted, and with neither known the branch is still said.
export function prSaved(saved: { title?: string; commits?: number; files?: number } | undefined, branch?: string): string {
  const where = branch ? `on branch ${branch}` : "";
  const files = saved?.files !== undefined ? count(saved.files, "file") : "";
  const what = saved?.commits === 1 && saved.title ? `Saved as “${saved.title}”` : saved?.commits ? count(saved.commits, "commit") : "";
  const parts = [what, what && files, where].filter(Boolean).join(" · ");
  return what ? parts : where.replace(/^on/, "On");
}
