import { UpdateButton } from "./UpdateButton";
import { useEffect, useRef, useState } from "react";
import { useStore } from "../store";
import { Floating } from "./Menu";
import { Icon } from "./icons";
import { Dialog, restoreFocus } from "./kit";
import { label } from "../keys";
import { LaneViewSwitch } from "./LaneViewSwitch";

export type Asking = "merge" | "discard" | "close" | null;

/// The lane's head: its name, where it runs, and its actions behind ⋯ — three bordered buttons on
/// every lane were the loudest thing on screen and the least used.
/// The one place a lane is renamed — the header is on screen whether the sidebar is hidden or the
/// project collapsed, so F2, a double-click and ⌘K all land here. Enter or leaving keeps the name,
/// Esc keeps the old one, and focus goes back to wherever the rename was asked for.
function Rename({ lane, title }: { lane: string; title: string }) {
  // Read before the input mounts and takes focus.
  const [back] = useState(() => document.activeElement);
  const done = useRef(false);
  const field = useRef<HTMLInputElement>(null);
  // A lane switch unmounts the field without a blur; keep what was typed rather than leave the
  // editor armed to reopen on the next visit.
  useEffect(() => () => finish(field.current?.value ?? null), []); // eslint-disable-line react-hooks/exhaustive-deps
  const finish = (name: string | null) => {
    if (done.current) return;
    done.current = true;
    const t = name?.trim();
    useStore.setState((s) => ({ renaming: null, ...(t ? { lanes: { ...s.lanes, [lane]: { ...s.lanes[lane], title: t } } } : {}) }));
    requestAnimationFrame(() => restoreFocus(back));
  };
  return (
    <input
      ref={field}
      autoFocus
      defaultValue={title}
      className="field"
      style={{ margin: 0, height: 28, minWidth: 260 }}
      aria-label="Lane name"
      onFocus={(e) => e.currentTarget.select()}
      onBlur={(e) => finish(e.target.value)}
      onKeyDown={(e) => (e.key === "Enter" ? finish(e.currentTarget.value) : e.key === "Escape" && (e.stopPropagation(), finish(null)))}
    />
  );
}

export function LaneHeader({ lane, toggleShell, shell }: { lane: string; toggleShell: () => void; shell: boolean }) {
  const title = useStore((s) => s.lanes[lane]?.title);
  const isolated = useStore((s) => s.lanes[lane]?.isolated);
  const wt = useStore((s) => s.lanes[lane]?.wt);
  const agent = useStore((s) => s.lanes[lane]?.agent);
  const [menu, setMenu] = useState<DOMRect | null>(null);
  const [asking, setAsking] = useState<Asking>(null);
  const renaming = useStore((s) => s.renaming === lane);
  const setRenaming = (on: boolean) => useStore.setState({ renaming: on ? lane : null });
  return (
    <header className="lane-header">
      <div className="lane-heading">
        {renaming ? (
          <Rename lane={lane} title={title ?? ""} />
        ) : (
          <span className="lane-name" onDoubleClick={() => setRenaming(true)} title="Double-click or F2 in the sidebar to rename">
            {title}
          </span>
        )}
        {isolated ? <span className="lane-branch">{wt ? `keel/${wt}` : "own branch"}</span> : <span className="small faint">shares the project's tree</span>}
        {agent === "codex" && <span className="small faint">codex</span>}
      </div>
      <UpdateButton />
      <LaneViewSwitch lane={lane} />
      <button className="ghost" aria-label="Lane actions" title="Lane actions" onClick={(e) => setMenu(e.currentTarget.getBoundingClientRect())}>
        <Icon name="more" />
      </button>
      <button className="ghost" aria-label="Close lane" title={`Close lane (${label({ key: "w" })})`} onClick={() => setAsking("close")}>
        <Icon name="x" />
      </button>
      {menu && (
        <Floating anchor={menu} onClose={() => setMenu(null)}>
          <LaneViewSwitch lane={lane} menu onSwitch={() => setMenu(null)} />
          <button onClick={() => (setMenu(null), toggleShell())}>{shell ? "Close the shell" : "Open a shell"}</button>
          <button onClick={() => (setMenu(null), setRenaming(true))}>Rename lane…</button>
          <div className="sep" />
          {isolated && wt && (
            <>
              <button onClick={() => (setMenu(null), setAsking("merge"))}>Merge lane… <kbd>{label({ key: "m", shift: true })}</kbd></button>
              <button onClick={() => (setMenu(null), setAsking("discard"))}>Discard lane… <kbd>{label({ key: "Backspace", shift: true })}</kbd></button>
            </>
          )}
          <button onClick={() => (setMenu(null), setAsking("close"))}>Close lane <kbd>{label({ key: "w" })}</kbd></button>
        </Floating>
      )}
      {asking && <Confirm lane={lane} what={asking} done={() => setAsking(null)} />}
    </header>
  );
}

/// Merge, discard and close each say what they will do before they do it — in the middle of
/// the window, because each is a decision about the whole lane.
export function Confirm({ lane, what, done }: { lane: string; what: Asking; done: () => void }) {
  const wt = useStore((s) => s.lanes[lane]?.wt);
  const title = useStore((s) => s.lanes[lane]?.title);
  const busy = useStore((s) => !!s.lanes[lane]?.running);
  const loss = useStore((s) => s.lanes[lane]?.confirmDiscard);
  const { merge, discard, closeLane } = useStore.getState();
  if (what === "merge")
    return (
      <Dialog title={`Merge keel/${wt}?`} onClose={done}>
        <p>
          Into the branch it was cut from, with <code>--no-ff</code> so the lane stays one reviewable merge. The lane closes and its branch is deleted.
          {busy && " The agent is mid-turn — merge when it is idle."}
        </p>
        <div className="actions">
          <button className="primary" disabled={busy} onClick={() => (done(), void merge(lane))}>
            Merge
          </button>
          <button onClick={done}>Cancel</button>
        </div>
      </Dialog>
    );
  if (what === "discard")
    return (
      <Dialog title={`Discard keel/${wt}?`} onClose={done} tone="danger">
        <p>{loss ?? "Its branch and its checkout under .keel/worktrees are deleted. This cannot be undone."}</p>
        <div className="actions">
          <button
            className="danger"
            disabled={busy}
            onClick={async () => {
              // Asked once without force: the daemon either discards, or says what it would lose
              // — shown here, with the forced button.
              await discard(lane, !!loss);
              if (!useStore.getState().lanes[lane]?.confirmDiscard || loss) done();
            }}
          >
            {loss ? "Discard anyway" : "Discard"}
          </button>
          <button onClick={done}>Keep it</button>
        </div>
      </Dialog>
    );
  return (
    <Dialog title={`Close “${title ?? "this lane"}”?`} onClose={done}>
      <p>
        {busy ? "The agent is mid-turn in this lane, and closing stops it. " : "Closing ends the agent in this lane. "}
        {wt ? `The branch keel/${wt} and its checkout stay — reopen it from the project's menu.` : "The conversation stays in History, so you can resume it."}
      </p>
      <div className="actions">
        <button className="primary" onClick={() => (done(), closeLane(lane))}>
          Close lane
        </button>
        <button onClick={done}>Keep it open</button>
      </div>
    </Dialog>
  );
}
