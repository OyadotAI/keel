// File-drop feedback stays outside xterm so it cannot alter provider output or prompt contents.
import { useFileDrag } from "../file-drag";
import { useStore } from "../store";
import { terminalReferenceProblem } from "../terminal-file-reference";
import type { ReactNode } from "react";

/** Only provider surfaces participate in internal drops; shell and hidden terminals stay excluded. */
export function TerminalFileSurface({ lane, agent, hidden, children }: { lane: string; agent: boolean; hidden: boolean; children: ReactNode }) {
  const current = useStore((s) => s.lanes[lane]), active = useStore((s) => s.active);
  const drag = useFileDrag((s) => s.drag);
  const over = agent && !hidden && drag?.target === lane && !terminalReferenceProblem(current, active, drag.reference);
  return <div className={`terminal${hidden ? " hidden" : ""}${over ? " reference-over" : ""}`} aria-hidden={hidden} data-reference-target={agent && !hidden ? lane : undefined}>
    {agent && <TerminalFileFeedback lane={lane} />}{children}
  </div>;
}

/** Describe insertion and rejection without pretending the agent accepted or executed a prompt. */
export function TerminalFileFeedback({ lane }: { lane: string }) {
  const current = useStore((s) => s.lanes[lane]), active = useStore((s) => s.active);
  const drag = useFileDrag((s) => s.drag);
  const hover = drag?.target === lane;
  const problem = hover ? terminalReferenceProblem(current, active, drag.reference) : undefined;
  const error = current?.referenceError;
  const text = current?.referencePending ? current.referenceStatus : hover ? problem ?? "Drop to insert this file path. Finish your message before submitting." : error ?? current?.referenceStatus;
  if (!text) return null;
  return <div className={`terminal-file-feedback small${error || problem ? " error" : " muted"}`} role={error && !hover ? "alert" : "status"}>
    <span>{text}</span>{error && !hover && <button aria-label="Dismiss file insertion error" onClick={() => useStore.setState((s) => ({ lanes: { ...s.lanes, [lane]: { ...s.lanes[lane], referenceError: undefined } } }))}>×</button>}
  </div>;
}
