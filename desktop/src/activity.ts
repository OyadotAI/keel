// Shared display-only lane activity. Independent children do not change parent execution flags.
import type { Lane, Session } from "./store";

/** Native prompts and Keel cards share one lane's waiting treatment without duplicate badges. */
export function waiting(l: Lane): number { return l.pending.length || (l.attention ? 1 : 0); }

const VERBS: [RegExp, string][] = [
  [/^Bash$/, "Running"], [/^(Grep|Glob|WebSearch)$/, "Searching"], [/^(Read|LS|WebFetch)$/, "Reading"],
  [/^Write$/, "Writing"], [/^(Edit|MultiEdit)$/, "Editing"], [/^(Task|Agent)$/, "Agent —"], [/^mcp__keel__browser_/, "Browser —"],
];
const names = (agents: (string | null)[]) => agents.length > 2 ? `${agents.length} agents` : agents.map((a) => a || "an agent").join(" and ");

/** Use current observations for visuals while keeping the parent turn's execution state intact. */
export function activity(l: Lane): { label: string; since?: string; tone: "idle" | "live" | "warn" } {
  const questions = waiting(l);
  if (questions) return { label: questions > 1 ? `paused — ${questions} questions waiting` : "waiting for you", tone: "warn" };
  const t = l.conv.turns.at(-1);
  if (t?.gate?.status === "running") return { label: `Checking — ${t.gate.command ?? "the project's checks"}`, tone: "live" };
  const running = l.observation?.running ?? l.running;
  const background = Object.values(l.observation?.background ?? l.conv.background ?? {});
  if (!running && background.length) return { label: `${names(background)} working in the background`, tone: "live" };
  if (!running) return { label: "idle", tone: "idle" };
  const call = t && !t.closed && Object.values(t.calls).reverse().find((c) => c.state === "running" && !c.parent);
  if (call) {
    const verb = VERBS.find(([re]) => re.test(call.tool))?.[1] ?? call.tool;
    return { label: `${verb} ${call.subject.slice(0, 60)}`, since: t?.startedAt, tone: "live" };
  }
  return { label: t && !t.closed && Object.keys(t.blocks).length ? "Writing…" : "Thinking…", since: t?.startedAt, tone: "live" };
}

/** Member identities come from lifecycle metadata; unnamed children still contribute to activity. */
export function workingMembers(l: Lane, session?: Session): string[] {
  if (sessionEnded(l, session)) return [];
  const names = new Set(Object.values(l.observation?.background ?? l.conv.background ?? {}).filter((a): a is string => !!a));
  if (!(l.observation?.running ?? l.running) || waiting(l)) return [...names];
  const turn = l.conv.turns.at(-1);
  if (turn?.closed) return [...names];
  for (const call of Object.values(turn?.calls ?? {})) {
    if (!/^(Agent|Task)$/.test(call.tool) || typeof call.input?.subagent_type !== "string") continue;
    if (call.state === "running" || (turn?.children[call.id] ?? []).some((id) => turn?.calls[id]?.state === "running")) names.add(call.input.subagent_type);
  }
  return [...names];
}

/** Only Claude History carries a process-liveness signal; other adapters report activity separately. */
export function sessionEnded(lane: Lane, session?: Session): boolean {
  return (lane.agent ?? "claude") === "claude" && session?.live === false;
}
