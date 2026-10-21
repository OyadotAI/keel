// Keel's agent team across coding agents: what `/api/team` reports, and the sentences about it.
import { get, type Endpoint } from "./api";

export interface TeamPlan {
  add: string[];
  replace: string[];
  kept: string[];
  not_restored: string[];
  present: boolean;
  agents: { claude: boolean; antigravity: boolean; codex: boolean };
}

export interface TeamUpdated {
  replaced: string[];
  added: string[];
  kept: string[];
  not_restored: string[];
  uncommitted: string[];
  committed?: string | null;
  note?: string | null;
  /** The lane it went into, because a turn or a terminal session was writing the project's tree. */
  lane?: string | null;
}

export const readTeam = (ep: Endpoint) => get<TeamPlan>(ep, "/api/team");

/** Files an update would write: stale ones replaced and missing ones added. */
export const pending = (p?: TeamPlan) => (p ? p.add.length + p.replace.length : 0);

/** An update is offered only to a project that has the team; one without it is adoption. */
export const offered = (p?: TeamPlan) => !!p?.present && pending(p) > 0;

const NAMES = [["claude", "Claude Code"], ["antigravity", "Antigravity"], ["codex", "Codex"]] as const;

/** "Claude Code, Antigravity and Codex", from whichever of them a list says. */
export function joined(names: string[]): string {
  return names.length < 2 ? names.join("") : `${names.slice(0, -1).join(", ")} and ${names.at(-1)}`;
}

/** Which agents the team is for, from the plan; all three when it has not been read. */
export function forAgents(p?: TeamPlan): string {
  return joined(NAMES.filter(([k]) => !p || p.agents[k]).map(([, n]) => n));
}

/** "Codex isn't on this machine." — the agents left out, so a missing folder is never a mystery. */
export function absent(p?: TeamPlan): string {
  if (!p) return "";
  const off = NAMES.filter(([k]) => !p.agents[k]).map(([, n]) => n);
  return off.length ? `${joined(off)} ${off.length === 1 ? "isn't" : "aren't"} on this machine.` : "";
}

/** The other coding agents on this machine, by name: what a user-scope MCP server is copied to. */
export function others(p?: TeamPlan): string {
  return joined(NAMES.filter(([k]) => k !== "claude" && !!p?.agents[k]).map(([, n]) => n));
}

/** Which agents a list of written paths installed the team for. */
export function agentsIn(paths: string[]): string {
  const has = (prefix: string) => paths.some((p) => p.startsWith(prefix));
  return joined([
    ...(has(".claude/agents/") ? ["Claude Code"] : []),
    ...(has(".agents/agents/") ? ["Antigravity"] : []),
    ...(has(".codex/agents/") ? ["Codex"] : []),
  ]);
}

const plural = (n: number, one: string) => `${n} ${one}${n === 1 ? "" : "s"}`;

/** The sentence after an update. */
export function updatedSentence(r: TeamUpdated): string {
  const counts = `${r.replaced.length} replaced, ${r.added.length} added.`;
  const kept = r.kept.length ? ` Kept ${plural(r.kept.length, "file")} you changed.` : "";
  if (!r.replaced.length && !r.added.length) return `The team is up to date.${kept}`;
  if (r.lane) return `A session was editing the project, so the update went to its own lane, keel/${r.lane}: ${counts}${kept} Finish that lane to merge it.`;
  if (r.committed) return `Team updated in commit ${r.committed}: ${counts}${kept}`;
  return `Team updated: ${counts}${r.note ? ` Not committed: ${r.note}.` : ""}${kept}`;
}
