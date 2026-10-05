import { useState } from "react";
import { useStore } from "../store";
import { Floating } from "./Menu";
import { Icon } from "./icons";
import { Dialog } from "./kit";

export type Asking = "merge" | "discard" | "close" | null;

/// The lane's head: its name, where it runs, and its actions behind ⋯ — three bordered buttons on
/// every lane were the loudest thing on screen and the least used.
export function LaneHeader({ lane, toggleShell, shell }: { lane: string; toggleShell: () => void; shell: boolean }) {
  const title = useStore((s) => s.lanes[lane]?.title);
  const isolated = useStore((s) => s.lanes[lane]?.isolated);
  const wt = useStore((s) => s.lanes[lane]?.wt);
  const agent = useStore((s) => s.lanes[lane]?.agent);
  const [menu, setMenu] = useState<DOMRect | null>(null);
  const [asking, setAsking] = useState<Asking>(null);
  const [renaming, setRenaming] = useState(false);
  const rename = (name: string) => {
    if (name.trim()) useStore.setState((s) => ({ lanes: { ...s.lanes, [lane]: { ...s.lanes[lane], title: name.trim() } } }));
    setRenaming(false);
  };
  return (
    <header className="lane-header">
      <div className="lane-heading">
        {renaming ? (
          <input autoFocus defaultValue={title} className="field" style={{ margin: 0, height: 26 }} onBlur={(e) => rename(e.target.value)} onKeyDown={(e) => e.key === "Enter" && rename(e.currentTarget.value)} aria-label="Lane name" />
        ) : (
          <span className="lane-name" onDoubleClick={() => setRenaming(true)} title="Double-click to rename">
            {title}
          </span>
        )}
        {isolated ? <span className="lane-branch">{wt ? `keel/${wt}` : "own branch"}</span> : <span className="small faint">shares the project's tree</span>}
        {agent === "codex" && <span className="small faint">codex</span>}
      </div>
      <button className="ghost" aria-label="Lane actions" title="Lane actions" onClick={(e) => setMenu(e.currentTarget.getBoundingClientRect())}>
        <Icon name="more" />
      </button>
      <button className="ghost" aria-label="Close lane" title="Close lane (⌘W)" onClick={() => setAsking("close")}>
        <Icon name="x" />
      </button>
      {menu && (
        <Floating anchor={menu} onClose={() => setMenu(null)}>
          <button onClick={() => (setMenu(null), toggleShell())}>{shell ? "Close the shell" : "Open a shell"}</button>
          <button onClick={() => (setMenu(null), setRenaming(true))}>Rename lane…</button>
          <div className="sep" />
          {isolated && wt && (
            <>
              <button onClick={() => (setMenu(null), setAsking("merge"))}>Merge lane… <kbd>⌘⇧M</kbd></button>
              <button onClick={() => (setMenu(null), setAsking("discard"))}>Discard lane… <kbd>⌘⇧⌫</kbd></button>
            </>
          )}
          <button onClick={() => (setMenu(null), setAsking("close"))}>Close lane <kbd>⌘W</kbd></button>
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
