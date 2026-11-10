// "Open pull request" in one click, for people who have never branched or committed: Keel takes
// the git steps itself — a branch when the lane is on the project's main line, a commit of
// everything, then the pull request, which pushes. Each step is an existing daemon route.
import { post, type Endpoint } from "./api";
import { openPullRequest } from "./prereview";
import { promptAttachments } from "./prompt-attachments";
import type { Lane } from "./store";

/** Branches a pull request cannot come from: the line it would merge into. */
const MAIN_LINE = /^(main|master|trunk|develop|dev)$/;
const MESSAGE_LIMIT = 72;
const SLUG_LIMIT = 40;

/** The step being taken, in words a person who does not know git can read. */
export type ShipStep = "branch" | "commit" | "open";
export const STEP_WORDS: Record<ShipStep, string> = { branch: "Putting your work on its own branch…", commit: "Saving your changes…", open: "Opening it on GitHub…" };

/** What the work is called: the lane's name, else its first prompt. Becomes the commit and the title. */
export function shipTitle(l: Pick<Lane, "title" | "conv">): string {
  const named = l.title && !/^untitled/i.test(l.title) ? l.title : promptAttachments(l.conv?.turns[0]?.prompt ?? "").text;
  return named.split("\n")[0].trim().slice(0, MESSAGE_LIMIT) || "Changes from Keel";
}

/** A new branch name from the title, with a short suffix so a second ship never collides. */
export function branchName(title: string, suffix = Math.random().toString(36).slice(2, 6)): string {
  const slug = title.toLowerCase().replace(/[^a-z0-9]+/g, "-").replace(/^-+|-+$/g, "").slice(0, SLUG_LIMIT).replace(/-+$/, "");
  return `keel/${slug || "changes"}-${suffix}`;
}

/** Whether the lane is on a branch a pull request cannot come from, and so needs one of its own. */
export const needsBranch = (l: Pick<Lane, "wt" | "git">) => !l.wt && (!l.git?.branch || MAIN_LINE.test(l.git.branch));

/** Branch if needed, commit if needed, open. Resolves once the opening has started; its progress
 *  and its link are the pull-request run's, which the dock already draws. */
export async function ship(lane: string, l: Lane, ep: Endpoint, step: (s: ShipStep) => void): Promise<boolean> {
  const title = shipTitle(l);
  if (needsBranch(l)) {
    step("branch");
    await post(ep, "/api/git/branch", { action: "create", name: branchName(title) });
  }
  if (l.git?.changes.length) {
    step("commit");
    await post(ep, "/api/git/commit", { message: title }, { wt: l.wt });
  }
  step("open");
  return openPullRequest(ep, l.wt, { title, draft: false }, lane);
}
