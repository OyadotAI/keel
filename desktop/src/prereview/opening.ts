// Stream ownership survives panel remounts and cannot convert a failed/partial open into success.
import { url, type Endpoint } from "../api";
import { clear, runOf, start, useRun } from "../runs";
import type { Arrived } from "../stream.worker";
import { LOG_LINES } from "./constants";

/** User input travels with explicit target guards instead of relying on a later current branch. */
export interface OpeningInput { title: string; body?: string; draft: boolean; base?: string; head?: string; branch?: string }
/** Stream completion and a returned PR link are separate facts, preserving uncertain results. */
export interface PrRun { lines: string[]; result?: "done" | "failed"; complete?: boolean; error?: string; link?: string; existing?: boolean; actualHead?: string; target?: { branch?: string; head?: string; base?: string } }

/** A restarted daemon at a reused port cannot inherit another endpoint's publication state. */
export const prKey = (ep: Endpoint, wt?: string) => JSON.stringify(["pr", ep.port, ep.token, wt]);
export const usePrRun = (ep: Endpoint, wt?: string) => useRun<PrRun>(prKey(ep, wt));
export const prRun = (ep: Endpoint, wt?: string) => runOf<PrRun>(prKey(ep, wt));
export const clearOpening = (ep: Endpoint, wt?: string) => clear(prKey(ep, wt));

/** A completed PR for an earlier branch must not replace the new branch's form and actions. */
export function prForTarget(run: ReturnType<typeof prRun>, branch?: string) {
  return run?.running || !run?.state.target?.branch || !branch || run.state.target.branch === branch ? run : undefined;
}

/** Only terminal completion confirms success; fatal state survives a later zero exit. */
export function prStep(state: PrRun, event: Arrived): PrRun {
  if (event.event === "line") return { ...state, lines: [...state.lines, String(event.data)].slice(-LOG_LINES) };
  if (event.event === "fatal") return { ...state, error: String(event.data), result: "failed", lines: [...state.lines, String(event.data)].slice(-LOG_LINES) };
  if (event.event === "done") return { ...state, complete: true, result: state.result !== "failed" && String(event.data) === "0" ? "done" : "failed" };
  if (event.event === "pr") {
    const data = event.data as { url?: string; existing?: boolean; head?: string };
    if (data?.url?.match(/^https:\/\/github\.com\/[^/\s]+\/[^/\s]+\/pull\/\d+$/)) return { ...state, link: data.url, existing: !!data.existing, actualHead: data.head };
  }
  return state;
}

/** EOF without a confirmed completion remains retryable uncertainty, never an apparent open. */
export function prEnd(state: PrRun, error?: string): PrRun {
  return error || !state.complete ? { ...state, result: "failed", error: error ?? "The connection closed before opening was confirmed. Retry opening to check for an existing pull request first." } : state;
}

/** Retries always reach backend reconciliation; a known same-branch link offers no duplicate create. */
export function beginOpening(ep: Endpoint, wt: string | undefined, opening: OpeningInput): boolean {
  const key = prKey(ep, wt), before = prRun(ep, wt);
  if (before?.running) return false;
  if (before?.state.link && before.state.target?.branch === opening.branch) return false;
  clear(key);
  return start(key, url(ep, "/api/github/pr", { title: opening.title, body: opening.body, draft: opening.draft || undefined, base: opening.base, head: opening.head, branch: opening.branch, wt }), ep.token,
    { lines: [], target: { branch: opening.branch, head: opening.head, base: opening.base } } satisfies PrRun, prStep, prEnd);
}
