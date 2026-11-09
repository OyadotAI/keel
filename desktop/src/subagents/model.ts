// Invocation lifecycle and read-only output stay independent from parent turn execution state.
import type { Lane } from "../store";
/** Provider evidence, never silence or an output socket close, changes execution status. */
export type ChildStatus = "working" | "waiting" | "done" | "failed" | "cancelled" | "interrupted" | "unknown";
/** Output can be absent while lifecycle remains trustworthy. */
export type Availability = "available" | "waiting" | "unavailable" | "disconnected";
/** A reused process contributes a separate invocation for each observed child turn. */
export interface Invocation { id: string; child?: string | null; member?: string | null; status: ChildStatus; activity?: string | null; started?: string | null; finished?: string | null; output: Availability }
/** Parent tail owns lifecycle; selected output never edits these statuses. */
export interface ChildSnapshot { invocations: Invocation[]; truncated: boolean }
/** An identity-scoped snapshot prevents a replacement lane from inheriting another parent's children. */
export interface ChildState extends ChildSnapshot { scope: string; connected: boolean }
/** Plain selectable text is not a child PTY and cannot execute terminal escape sequences. */
export interface ChildDetail { items: { kind: string; text: string }[]; availability: Availability; truncated: boolean }
/** All observation and preferences are keyed to the full provider, parent, and checkout identity. */
export const childScope = (lane?: Pick<Lane, "project" | "wt" | "agent" | "session">): string => lane ? JSON.stringify([lane.project, lane.wt, lane.agent, lane.session]) : "";
/** Unknown or disconnected work stays inspectable but is not advertised as confirmed activity. */
export const childActive = (child: Invocation): boolean => child.status === "working" || child.status === "waiting";
/** A lane's live children, read from the agent's own transcripts — a terminal lane's through its
 *  tail, a chat lane's through `/api/session/subagents` — and only for the parent it was read for. */
export function childrenOf(lane: Lane): ChildState | undefined { return lane.subagents?.scope === childScope(lane) ? lane.subagents : undefined; }
/** Connection loss retains history; process exit is explicit interruption, not fabricated completion. */
export function disconnectedChildren(lane: Lane, interrupted = false): ChildState | undefined {
  const state = childrenOf(lane);
  return state && { ...state, connected: interrupted, invocations: state.invocations.map((child) => interrupted && childActive(child) ? { ...child, status: "interrupted" } : child) };
}
