// The team's review before a pull request opens, as the daemon keeps it (`prereview.rs`). The
// record is the daemon's — an agent's `gh pr create` starts one before any window asks — so this
// holds only the last reading per lane, re-read when `reviews.changed` says so and on every
// reconnect. Nothing here decides anything.

import { useSyncExternalStore } from "react";
import { get, post, url, type Endpoint } from "./api";
import { start } from "./runs";

export interface Finding {
  path: string;
  line: number;
  severity: "blocking" | "warning" | "note" | string;
  title: string;
  body: string;
}

export type Outcome =
  | { kind: "verdict"; verdict: string; reason: string; summary: string; findings: Finding[]; report: string; took: number; cost?: number | null }
  | { kind: "none"; cause: string; line: string; took: number; cost?: number | null };

export interface Opening {
  title: string;
  body: string;
  draft: boolean;
  base: string;
}

export interface Review {
  lane: string;
  wt?: string | null;
  sha: string;
  base: string;
  started: number;
  phase: "waiting" | "checking" | "fetching" | "reviewing" | "done";
  /// What Open will open: the person's title and draft, or the agent's `gh pr create` words.
  opening: Opening;
  by_agent: boolean;
  outcome?: Outcome | null;
}

export interface Seen {
  review: Review | null;
  /// Where the checkout stands now: a review of another commit is stale.
  head: string;
  /// The page saw it running and the daemon no longer has it: Keel restarted.
  restarted?: boolean;
}

const seen = new Map<string, Seen>();
const listeners = new Set<() => void>();
const changed = () => listeners.forEach((l) => l());

export const reviewOf = (lane: string): Seen | undefined => seen.get(lane);

export function useReview(lane: string): Seen | undefined {
  return useSyncExternalStore(
    (l) => (listeners.add(l), () => listeners.delete(l)),
    () => reviewOf(lane),
  );
}

export async function refreshReview(lane: string, ep: Endpoint, wt?: string) {
  let now: Seen;
  try {
    now = await get<Seen>(ep, "/api/prereview", { lane, wt });
  } catch {
    return;
  }
  const before = seen.get(lane);
  const wasRunning = !!before?.review && !before.review.outcome;
  seen.set(lane, { ...now, restarted: !now.review && (wasRunning || !!before?.restarted) });
  changed();
}

export async function startReview(lane: string, ep: Endpoint, wt?: string, form: { title?: string; draft?: boolean } = {}) {
  await post(ep, "/api/prereview/start", { lane, title: form.title ?? "", draft: !!form.draft }, { wt });
  await refreshReview(lane, ep, wt);
}

export async function stopReview(lane: string, ep: Endpoint, wt?: string) {
  await post(ep, "/api/prereview/stop", { lane }, { wt }).catch(() => undefined);
}

/// Put away a finished review, for every window: the daemon drops it, so a reconnect does not
/// bring it back, and the form returns.
export async function dismissReview(lane: string, ep: Endpoint, wt?: string) {
  seen.delete(lane);
  changed();
  await post(ep, "/api/prereview/dismiss", { lane }, { wt }).catch(() => undefined);
}

/// The branch the review compared against, as `gh` names a base: the pull request opens onto
/// the diff the verdict is about.
export const baseOf = (r: Review) => r.base.replace(/^origin\//, "");

/// The Open step, one run per checkout — the key the Pull request tab has always used, so a tab
/// switch mid-push still cannot offer a second pull request.
export const prKey = (ep: Endpoint, wt?: string) => `pr:${ep.port}:${wt ?? ""}`;
export type PrRun = { lines: string[]; result?: "done" | "failed" };

export function openPullRequest(ep: Endpoint, wt: string | undefined, o: { title: string; body?: string; draft: boolean; base?: string }) {
  start(
    prKey(ep, wt),
    url(ep, "/api/github/pr", { title: o.title, body: o.title ? o.body : undefined, draft: o.draft ? "true" : undefined, base: o.base, wt }),
    ep.token,
    { lines: [] } as PrRun,
    (s, e) => ({
      lines: e.event === "line" || e.event === "fatal" ? [...s.lines, String(e.data)].slice(-100) : s.lines,
      result: e.event === "done" ? (e.data === "0" || e.data === 0 ? "done" : "failed") : e.event === "fatal" ? "failed" : s.result,
    }),
    (s) => ({ ...s, result: s.result ?? "failed" }),
  );
}

/// The pull request a run opened, or found already open: `gh` prints its address either way —
/// on success, and in the error when one exists for the branch. Once there is one, nothing offers
/// to open another: a second click used to make a second pull request.
export function prLink(run?: { state: PrRun }): string | undefined {
  return run?.state.lines.map((l) => l.match(/https:\/\/github\.com\/[^/\s]+\/[^/\s]+\/pull\/\d+/)?.[0]).find(Boolean);
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
