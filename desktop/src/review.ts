// Review verdicts and exported evidence share one interpretation of the latest check result.
import type { Call, CallState, Gate, Turn } from "./reduce";
import type { Arrived } from "./stream.worker";

export interface ReviewProblem { file: string; line: number; severity: string; message: string }
export interface ReviewCheck {
  state: "running" | "passed" | "failed" | "none";
  scope: string;
  recorded: string;
  started_at: string;
  finished_at?: string;
  complete?: boolean;
  command?: string;
  problems: ReviewProblem[];
  why?: string;
}
export interface ReviewEvidence {
  worked: Turn[];
  files: string[];
  commands: Call[];
  failed: number;
  latest?: Turn;
}
export function reviewEvidence(turns: Turn[] = []): ReviewEvidence {
  const worked = turns.filter((t) => t.files.length || Object.keys(t.calls).length);
  const commands = worked.flatMap((t) => Object.values(t.calls)).filter((c) => /^(Bash|Shell|Terminal)$/.test(c.tool));
  return { worked, files: [...new Set(worked.flatMap((t) => t.files))], commands, failed: commands.filter((c) => c.state === "failed").length, latest: turns.at(-1) };
}

/** Include the turn identity: two consecutive passing gates are distinct pieces of evidence. */
export function recordedCheck(turn?: Turn): string {
  return JSON.stringify([turn?.key ?? turn?.id, turn?.gate]);
}
export function reviewScope(session: string | undefined, wt: string | undefined, revision: number, recorded: string, port?: number): string {
  return JSON.stringify([session, wt, revision, recorded, port]);
}

export type ReviewStatus = "running" | "passed" | "failed" | "none" | "aborted" | "stale" | "not-run" | "unknown";
const REASONS: Record<Exclude<ReviewStatus, "passed">, string> = {
  running: "The project's checks are still running.",
  failed: "The project's checks failed. Fix the failure and run them again.",
  none: "No check command was found. Declare a project check before treating this work as verified.",
  aborted: "The checks were interrupted before finishing. Run them again.",
  stale: "The checkout or conversation changed after these checks. Run them again.",
  "not-run": "The latest turn has no check result. Run the project's checks to verify this work.",
  unknown: "The latest check result is not a recognized passing verdict. Run the checks again.",
};
const STATUS = new Set<ReviewStatus>(["running", "passed", "failed", "none", "aborted"]);
export interface ReviewVerdict { state: ReviewStatus; source: "manual" | "recorded"; command?: string | null; blocker?: string }

/** A stale manual run must never mask a newer automatic failure or fall back to an older pass. */
export function reviewVerdict(busy: boolean, run: ReviewCheck | undefined, scope: string, recorded: string, gate?: Gate): ReviewVerdict {
  let source: ReviewVerdict["source"] = "recorded";
  let state: ReviewStatus = gate ? STATUS.has(gate.status as ReviewStatus) ? gate.status as ReviewStatus : "unknown" : "not-run";
  let command = gate?.command;
  if (run?.scope === scope || run?.state === "running") {
    source = "manual"; state = run.state; command = run.command;
  } else if (run && (!run.scope || run.recorded === recorded)) {
    source = "manual"; state = "stale"; command = run.command;
  }
  const blocker = busy ? "The agent is mid-turn. Wait until it is idle."
    : state !== "passed" ? REASONS[state] : undefined;
  return { state, source, command, blocker };
}

/** Terminal events determine success; a fatal event cannot be overwritten by a later zero exit. */
export function checkStep(check: ReviewCheck, event: Arrived): ReviewCheck {
  switch (event.event) {
    case "start": return { ...check, command: String(event.data) };
    case "problem": return { ...check, problems: [...check.problems, event.data as ReviewProblem].slice(-200) };
    case "none": return { ...check, state: "none", why: String(event.data), complete: true };
    case "fatal": return { ...check, state: "failed", why: String(event.data) };
    case "done": return { ...check, state: check.state === "none" ? "none" : check.state !== "failed" && String(event.data) === "0" ? "passed" : "failed", complete: true };
    default: return check;
  }
}
export function checkEnd(check: ReviewCheck, error?: string): ReviewCheck {
  const finished_at = new Date().toISOString();
  if (error || !check.complete) return { ...check, state: "failed", why: error || check.why || "The check stopped before reporting completion.", finished_at };
  return { ...check, finished_at };
}

const COMMANDS: Record<CallState, { mark: string; label: string }> = {
  ok: { mark: "✓", label: "Succeeded" },
  failed: { mark: "✗", label: "Failed" },
  interrupted: { mark: "—", label: "Interrupted" },
  running: { mark: "…", label: "Running" },
};
export function commandEvidence(call: Call) {
  return { tool: call.tool, command: String(call.input?.command ?? call.input?.cmd ?? call.subject), reason: call.reason ?? null, state: call.state, failed: call.state === "failed" };
}
export function commandMarkdown(command: ReturnType<typeof commandEvidence>): string {
  const status = COMMANDS[command.state];
  const summary = command.command.split("\n")[0].slice(0, 160).replaceAll("`", "'");
  return `- ${status.mark} ${status.label}: \`${summary}\``;
}

export function verificationSummary(verdict: ReviewVerdict, run?: ReviewCheck): string {
  if (verdict.blocker) return verdict.blocker;
  return verdict.source === "manual" ? `Checks passed${run?.finished_at ? ` at ${new Date(run.finished_at).toLocaleTimeString()}` : ""}. Changes invalidate this result.` : "Checks passed for the latest recorded turn. Run them again to recheck the checkout.";
}
