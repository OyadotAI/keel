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

type Git = NonNullable<Lane["git"]>;
/** Changes a commit would take; an older daemon reports only the whole list. */
const unsaved = (git: Git) => git.unsaved ?? git.changes.length;
/** The work as it stands, so a closed card stays closed until this changes. */
export const shareKey = (git: Git) => `${git.head ?? ""}:${unsaved(git)}`;

/** What the share card offers: a first pull request, more changes for the one already open, or
 *  nothing — when there is no work the base or the pull request lacks, or it was closed at this
 *  state. It used to show for any lane that had ever changed a file, for good. */
export function shareOffer(git: Lane["git"], dismissed?: string): "open" | "update" | undefined {
  if (!git?.is_repo || dismissed === shareKey(git)) return undefined;
  const dirty = unsaved(git) > 0;
  if (git.pr) return dirty || (!!git.head && git.pr.head !== git.head) ? "update" : undefined;
  return dirty || (git.ahead ?? 0) > 0 ? "open" : undefined;
}

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
  await prepare(l, ep, title, step);
  step("open");
  return openPullRequest(ep, l.wt, { title, draft: false }, lane);
}

/** The git half on its own: a branch off the main line, then everything committed as `message`.
 *  Shared with "Commit your changes first", which otherwise had no way off `main`. */
export async function prepare(l: Lane, ep: Endpoint, message: string, step: (s: ShipStep) => void = () => {}): Promise<void> {
  if (needsBranch(l)) {
    step("branch");
    await post(ep, "/api/git/branch", { action: "create", name: branchName(message) });
  }
  if (l.git && unsaved(l.git)) {
    step("commit");
    await post(ep, "/api/git/commit", { message }, { wt: l.wt });
  }
}
