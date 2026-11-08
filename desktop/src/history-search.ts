// Which past sessions a History search shows: matched on what the listing already carries.
import { AGENT_NAME, type Session } from "./store";

/// Title, branch and agent only — the listing never carries what a conversation said
/// (non-negotiable 7), so a search must not go looking for it either. Every word must match.
export function matchSessions(list: Session[], query: string): Session[] {
  const words = query.toLowerCase().split(/\s+/).filter(Boolean);
  if (!words.length) return list;
  return list.filter((s) => {
    const hay = [s.title || "Untitled session", s.branch ?? "", AGENT_NAME[s.provider ?? "claude"]].join(" ").toLowerCase();
    return words.every((w) => hay.includes(w));
  });
}
