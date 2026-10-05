/// Hooks, as a person reads them: grouped by command, named by the script they end up running.

export interface Hook {
  event: string;
  command: string;
  scope: string;
  source: string;
}

/// What each Claude Code hook event means, in the words someone would use for it.
export const EVENTS: Record<string, string> = {
  PreToolUse: "Before each tool call",
  PostToolUse: "After each tool call",
  PostToolUseFailure: "After a tool call fails",
  PermissionRequest: "When Claude asks permission",
  UserPromptSubmit: "When you send a prompt",
  Notification: "On notifications",
  Stop: "When a turn ends",
  SubagentStart: "When a subagent starts",
  SubagentStop: "When a subagent finishes",
  PreCompact: "Before compacting",
  PostCompact: "After compacting",
  SessionStart: "When a session starts",
  SessionEnd: "When a session ends",
};

const SCRIPT = /(?:"|')?((?:\$\{?HOME-?\}?|~|\/)[^\s"';|&]*\.(?:sh|bash|zsh|py|js|mjs|cjs|ts|rb|ps1|cmd))\b/g;

export function tilde(p: string): string {
  return p.replace(/^\/(?:Users|home)\/[^/]+/, "~").replace(/^\$\{?HOME-?\}?/, "~");
}

/// One row per distinct command, with every event it is registered for. A hook manager (Orca,
/// say) registers one long wrapper on a dozen events; shown raw that is a dozen screens of shell.
/// The row leads with the script the wrapper ends up running, because that is the thing to know.
export function groupHooks(hooks: Hook[]): { command: string; source: string; scope: string; events: string[]; runs: string; owner?: string }[] {
  const by = new Map<string, { command: string; source: string; scope: string; events: string[]; runs: string; owner?: string }>();
  for (const h of hooks) {
    const key = `${h.scope}\0${h.source}\0${h.command}`;
    const seen = by.get(key);
    if (seen) {
      if (!seen.events.includes(h.event)) seen.events.push(h.event);
      continue;
    }
    const scripts = [...h.command.matchAll(SCRIPT)].map((m) => tilde(m[1]));
    // Prefer the POSIX script a cross-platform wrapper falls through to on this machine.
    const script = scripts.find((x) => !/\.(cmd|ps1)$/.test(x)) ?? scripts[0];
    const first = h.command.trim().split(/\s+/)[0] ?? "";
    const runs = script ? `Runs ${script}` : h.command.length <= 80 ? h.command : `Runs ${tilde(first)}`;
    const owner = script?.match(/^~\/\.([a-z0-9-]+)\//i)?.[1];
    by.set(key, { command: h.command, source: h.source, scope: h.scope, events: [h.event], runs, owner: owner && owner[0].toUpperCase() + owner.slice(1) });
  }
  return [...by.values()];
}
