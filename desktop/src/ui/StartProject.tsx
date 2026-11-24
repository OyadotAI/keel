// The first lane of a newly opened project: which agent runs it, and whether it is a chat or the
// agent's own terminal. Asked rather than assumed, and remembered as the next default.
import { useState } from "react";
import { AGENT_NAME, useStore, type Agent, type ChatInterface } from "../store";
import { AgentMark } from "./AgentMark";
import { Dialog } from "./kit";

const KEY = "keel.start-choice";
const AGENTS: { id: Agent; note: string }[] = [
  { id: "claude", note: "Anthropic’s Claude Code" },
  { id: "codex", note: "OpenAI’s Codex CLI" },
  { id: "antigravity", note: "Google’s Antigravity" },
  { id: "opencode", note: "The open-source OpenCode" },
];
const SURFACES: { id: ChatInterface; label: string; note: string }[] = [
  { id: "chat", label: "Chat", note: "A conversation with each turn’s files, checks and cost." },
  { id: "terminal", label: "Terminal", note: "The agent’s own CLI, exactly as in your terminal." },
];

/// The last choice, or Claude in chat for a first project.
export function lastChoice(): { agent: Agent; surface: ChatInterface } {
  try {
    const saved = JSON.parse(localStorage.getItem(KEY) ?? "null") as { agent?: string; surface?: string } | null;
    const agent = AGENTS.find((a) => a.id === saved?.agent)?.id ?? "claude";
    const surface = SURFACES.find((x) => x.id === saved?.surface)?.id ?? "chat";
    return { agent, surface };
  } catch {
    return { agent: "claude", surface: "chat" };
  }
}

/// Shown while `starting` names a project; Start makes its first lane, Not now leaves it empty.
export function StartProject() {
  const path = useStore((s) => s.starting);
  const name = useStore((s) => (s.starting ? s.projects[s.starting]?.name : undefined));
  const [choice, setChoice] = useState(lastChoice);
  if (!path) return null;
  const close = () => useStore.setState({ starting: undefined });
  const start = () => {
    try { localStorage.setItem(KEY, JSON.stringify(choice)); } catch { /* the choice still applies to this lane */ }
    const s = useStore.getState();
    close();
    if (s.projects[path]) s.select(s.newLane(path, false, choice.agent, choice.surface));
  };
  return (
    <Dialog title={`Start ${name ?? "this project"}`} onClose={close}>
      <div className="start-project">
        <p className="start-label" id="start-agent">Agent</p>
        <div className="start-agents" role="radiogroup" aria-labelledby="start-agent">
          {AGENTS.map((a) => (
            <button key={a.id} role="radio" aria-checked={choice.agent === a.id} className="start-agent" onClick={() => setChoice((c) => ({ ...c, agent: a.id }))}>
              <AgentMark agent={a.id} size={18} />
              <span><strong>{AGENT_NAME[a.id]}</strong><small>{a.note}</small></span>
            </button>
          ))}
        </div>
        <p className="start-label" id="start-surface">Work in</p>
        <div className="segmented start-surface" role="radiogroup" aria-labelledby="start-surface">
          {SURFACES.map((x) => <button key={x.id} role="radio" aria-checked={choice.surface === x.id} onClick={() => setChoice((c) => ({ ...c, surface: x.id }))}>{x.label}</button>)}
        </div>
        <p className="start-note">{SURFACES.find((x) => x.id === choice.surface)?.note}</p>
        <div className="actions">
          <button className="primary" onClick={start}>Start with {AGENT_NAME[choice.agent]}</button>
          <button onClick={close}>Not now</button>
        </div>
      </div>
    </Dialog>
  );
}
