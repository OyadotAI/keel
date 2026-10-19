import { useState } from "react";
import { Dialog } from "./kit";
import { terminalConnected } from "../typers";
import { AGENT_NAME, useStore, type ChatInterface } from "../store";
import { useIde } from "../ide";
import { Icon } from "./icons";

/** Both entry points retain the store's session-handoff guards. */
export function LaneViewSwitch({ lane, menu = false, onSwitch }: {
  lane: string;
  menu?: boolean;
  onSwitch?: () => void;
}) {
  const coding = useIde((s) => s.layout === "ide");
  const [confirm, setConfirm] = useState(false);
  const surface = useStore((s) => s.lanes[lane]?.interface ?? "chat");
  const name = useStore((s) => AGENT_NAME[s.lanes[lane]?.agent ?? "claude"]);
  const busy = useStore((s) => {
    const l = s.lanes[lane];
    return !!(l?.running || l?.submitting || l?.receipt);
  });
  const hint = busy ? "Finish or stop the turn and resolve any unconfirmed message before switching views" : undefined;
  function change(value: ChatInterface) {
    const l = useStore.getState().lanes[lane];
    if (value === "chat" && l?.interface === "terminal" && l.agent !== "claude" && l.known && terminalConnected(lane)) { setConfirm(true); return; }
    onSwitch?.();
    void useStore.getState().switchInterface(lane, value);
  }
  const confirmation = confirm && <Dialog title="Switch to Chat?" onClose={() => setConfirm(false)}>
    <p>Keel will ask {name} to exit the terminal normally, then reopen this same conversation in Chat. Copy any unsent text in the terminal before continuing.</p>
    <div className="actions">
      <button onClick={() => setConfirm(false)}>Keep terminal open</button>
      <button className="primary" disabled={busy} onClick={() => { setConfirm(false); onSwitch?.(); void useStore.getState().switchInterface(lane, "chat"); }}>Exit terminal and switch</button>
    </div>
  </Dialog>;
  if (coding) return null;
  if (menu) {
    const next = surface === "terminal" ? "chat" : "terminal";
    return <><button disabled={busy} title={hint} onClick={() => change(next)}>
      <Icon name={next === "chat" ? "chat" : "terminal"} size={14} />
      {next === "chat" ? "Switch to formatted chat" : "Switch to terminal"}
    </button>{confirmation}</>;
  }
  return <><div className="segmented lane-view-switch" role="group" aria-label="Lane view" title={hint}>
    <button aria-pressed={surface === "chat"} disabled={busy} onClick={() => change("chat")} title={hint ?? "Formatted chat view"}>
      <Icon name="chat" size={13} /> Chat
    </button>
    <button aria-pressed={surface === "terminal"} disabled={busy} onClick={() => change("terminal")} title={hint ?? "Agent terminal view"}>
      <Icon name="terminal" size={13} /> Terminal
    </button>
  </div>{confirmation}</>;
}
