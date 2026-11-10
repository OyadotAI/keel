// A small mark per coding agent, so a lane says which agent drives it at a glance.
import type { Agent } from "../store";

/** Simple drawn marks in each agent's colour — not the vendors' logos. */
const MARKS: Record<Agent, { color: string; d: string }> = {
  claude: { color: "#d97757", d: "M12 2v20M2 12h20M4.9 4.9l14.2 14.2M19.1 4.9 4.9 19.1" },
  codex: { color: "currentColor", d: "M5 8l4 4-4 4M12 16h7" },
  antigravity: { color: "#3b7bf6", d: "M12 3l8 16H4zM12 10v5" },
};

/** The lane's agent as a 14px mark; decorative, the name beside it is what is read. */
export function AgentMark({ agent = "claude", size = 14 }: { agent?: Agent; size?: number }) {
  const m = MARKS[agent];
  return (
    <svg className="agent-mark" width={size} height={size} viewBox="0 0 24 24" fill="none" stroke={m.color} strokeWidth={2.4} strokeLinecap="round" strokeLinejoin="round" aria-hidden="true">
      <path d={m.d} />
    </svg>
  );
}
