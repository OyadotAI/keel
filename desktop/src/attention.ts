// Attention is a projection of received lane evidence; visiting it never resolves its sources.
import type { Lane, Project } from "./store";
import type { Turn } from "./reduce";

const PRIORITY = { input: 0, error: 1, checks: 2, update: 3 } as const;
const ERROR_PREVIEW_LENGTH = 300;

/** Only open-project membership grants a lane a place in the workspace list. */
export interface AttentionSource {
  order: readonly string[];
  projects: Record<string, Pick<Project, "name" | "lanes" | "error"> | undefined>;
  lanes: Record<string, Lane | undefined>;
}
/** Source-owned reasons remain separate so acknowledgement cannot conceal unresolved work. */
export interface AttentionReason { kind: keyof typeof PRIORITY; text: string }
/** A request identity prevents navigation from focusing an unrelated approval after a race. */
export type AttentionDestination = { kind: "request"; request: string } | { kind: "terminal" | "error" | "checks" | "update" };
/** Rows carry display text and a destination, never executable commands or permission decisions. */
export interface AttentionItem {
  lane: string;
  project: string;
  projectName: string;
  title: string;
  reasons: AttentionReason[];
  priority: number;
  action: string;
  destination: AttentionDestination;
  disconnected: boolean;
}

function openLanes(source: AttentionSource): Lane[] {
  const seen = new Set<string>();
  return source.order.flatMap((path) => (source.projects[path]?.lanes ?? []).flatMap((id) => {
    const lane = source.lanes[id];
    if (!lane || lane.project !== path || seen.has(id)) return [];
    seen.add(id);
    return [lane];
  }));
}

/** Coverage describes received state rather than presenting dormant conversations as healthy. */
export function attentionCoverage(source: AttentionSource): { projects: number; unloaded: number } {
  return { projects: source.order.filter((path) => !!source.projects[path]).length, unloaded: openLanes(source).filter((lane) => !lane.loaded).length };
}

/** Conversation completion and project validation are independent observations. */
export function attentionEnding(turn?: Turn): string {
  if (!turn?.closed) return "New update";
  const stopped = turn.closed === "stopped" || turn.closed === "interrupted";
  const failed = turn.closed === "failed" || turn.failures.some((failure) => !failure.history);
  const ending = stopped ? "Stopped" : failed ? "Did not finish" : turn.closed === "done" ? "Finished" : "New update";
  const checks: Record<string, string> = { passed: "Checks passed", failed: "Checks failed", running: "Checks running", none: "Not checked" };
  return `${ending} · ${turn.gate ? Object.hasOwn(checks, turn.gate.status) ? checks[turn.gate.status] : "Check status unavailable" : "Not checked"}`;
}

function input(lane: Lane): { reason: AttentionReason; action: string; destination: AttentionDestination } | undefined {
  const request = lane.pending[0];
  if (request) {
    const question = request.tool === "AskUserQuestion";
    const count = lane.pending.length > 1 ? ` · ${lane.pending.length} requests` : "";
    const label = question ? "Needs your answer" : request.tool === "BrowserChoice" ? "Choose a browser" : "Permission requested";
    return { reason: { kind: "input", text: `${label}${count}` }, action: question ? "Answer question" : "Review request", destination: { kind: "request", request: request.id } };
  }
  if (lane.attention) return { reason: { kind: "input", text: lane.attention.kind === "permission" ? "Permission needed in terminal" : "Answer needed in terminal" }, action: "Open terminal", destination: { kind: "terminal" } };
}

function errors(lane: Lane): AttentionReason[] {
  const delivery = lane.pending.map((request) => lane.questionDrafts?.[request.id]?.error);
  const messages = [...new Set([lane.error, lane.nameError, ...delivery].filter((text): text is string => !!text))];
  return messages.map((text) => ({ kind: "error", text: `Lane error · ${text.slice(0, ERROR_PREVIEW_LENGTH)}${text.length > ERROR_PREVIEW_LENGTH ? "…" : ""}` }));
}

function item(lane: Lane, source: AttentionSource): AttentionItem | undefined {
  const request = input(lane), reasons: AttentionReason[] = request ? [request.reason] : [];
  reasons.push(...errors(lane));
  const latest = lane.conv.turns.at(-1);
  if (latest?.gate?.status === "failed") reasons.push({ kind: "checks", text: "Checks failed" });
  if (lane.unseen) reasons.push({ kind: "update", text: attentionEnding(latest) });
  if (!reasons.length) return;
  const kind = reasons[0].kind;
  const fallback = { error: { action: "Open lane", kind: "error" }, checks: { action: "View checks", kind: "checks" }, update: { action: "Read update", kind: "update" } } as const;
  const destination = request?.destination ?? { kind: fallback[kind as keyof typeof fallback].kind };
  return { lane: lane.id, project: lane.project, projectName: source.projects[lane.project]!.name, title: lane.title, reasons, priority: PRIORITY[kind], action: request?.action ?? fallback[kind as keyof typeof fallback].action, destination, disconnected: !!(lane.disconnected || source.projects[lane.project]?.error) };
}

/** Stable priority preserves project order and counts each extant lane once, including collapsed ones. */
export function attentionItems(source: AttentionSource): AttentionItem[] {
  return openLanes(source).flatMap((lane) => {
    const row = item(lane, source);
    return row ? [row] : [];
  }).sort((a, b) => a.priority - b.priority);
}

/** Recompute at activation so a closed project or resolved request cannot act through an old row. */
export function attentionSelection(source: AttentionSource, lane: string): AttentionItem | undefined {
  return attentionItems(source).find((row) => row.lane === lane);
}
