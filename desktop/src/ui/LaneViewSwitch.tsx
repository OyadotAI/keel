import { useStore, type ChatInterface } from "../store";
import { Icon } from "./icons";

/** Both entry points retain the store's session-handoff guards. */
export function LaneViewSwitch({ lane, menu = false, onSwitch }: {
  lane: string;
  menu?: boolean;
  onSwitch?: () => void;
}) {
  const surface = useStore((s) => s.lanes[lane]?.interface ?? "chat");
  const busy = useStore((s) => {
    const l = s.lanes[lane];
    return !!(l?.running || l?.submitting || l?.receipt);
  });
  const hint = busy ? "Finish or stop the turn and resolve any unconfirmed message before switching views" : undefined;
  function change(value: ChatInterface) {
    onSwitch?.();
    void useStore.getState().switchInterface(lane, value);
  }
  if (menu) {
    const next = surface === "terminal" ? "chat" : "terminal";
    return <button disabled={busy} title={hint} onClick={() => change(next)}>
      <Icon name={next === "chat" ? "chat" : "terminal"} size={14} />
      {next === "chat" ? "Switch to formatted chat" : "Switch to terminal"}
    </button>;
  }
  return <div className="segmented lane-view-switch" role="group" aria-label="Lane view" title={hint}>
    <button aria-pressed={surface === "chat"} disabled={busy} onClick={() => change("chat")} title={hint ?? "Formatted chat view"}>
      <Icon name="chat" size={13} /> Chat
    </button>
    <button aria-pressed={surface === "terminal"} disabled={busy} onClick={() => change("terminal")} title={hint ?? "Agent terminal view"}>
      <Icon name="terminal" size={13} /> Terminal
    </button>
  </div>;
}
