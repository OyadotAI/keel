// The one question about where a project lives: move it into the Keel workspace, or keep it.
import { useEffect, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { revealItemInDir } from "@tauri-apps/plugin-opener";
import { useStore } from "../store";
import { refusalText, warningText } from "../relocate";
import { Banner, Dialog } from "./kit";

const REVEAL = navigator.userAgent.includes("Mac") ? "Reveal in Finder" : "Show in Explorer";
const nameOf = (path: string) => path.split(/[\\/]/).filter(Boolean).pop() ?? path;

/// Mounted at the app's root rather than on the project row: a move re-files the row under its
/// new path, and a dialog drawn inside the old one would unmount mid-answer.
export function MoveProject() {
  const m = useStore((s) => s.moving);
  const titles = useStore((s) => s.lanes);
  const [workspace, setWorkspace] = useState("");
  const box = useRef<HTMLDivElement>(null);
  useEffect(() => { void invoke<string>("projects_dir").then(setWorkspace, () => undefined); }, []);
  // The dialog focuses its first button once; a phase that replaces the buttons focuses again.
  useEffect(() => { box.current?.closest(".dialog")?.querySelector<HTMLElement>(".actions button:not(:disabled)")?.focus(); }, [m?.phase]);
  if (!m) return null;
  const { answerMove, closeMove } = useStore.getState();
  const name = nameOf(m.from);
  const close = () => { if (m.phase === "offer") void answerMove(false); else if (m.phase !== "moving") closeMove(); };
  if (m.phase === "moved") return (
    <Dialog title="Moved into the Keel workspace" onClose={close}>
      <div ref={box}>
        <p>{name} is now at <code className="project-location">{m.to}</code>.</p>
        <p>Terminals and editors that had the old folder open need to reopen it from here. Sign-ins in the Browser tab stay with the old place.</p>
        {m.warnings?.map((w) => <Banner key={w.step} tone="warn">{warningText(w)}</Banner>)}
      </div>
      <div className="actions">
        <button className="primary" onClick={closeMove}>Done</button>
        <button onClick={() => void revealItemInDir(m.to)}>{REVEAL}</button>
      </div>
    </Dialog>
  );
  const refused = m.phase === "refused" && m.refusal;
  return (
    <Dialog title={`Move “${name}” into the Keel workspace?`} onClose={close}>
      <div ref={box}>
        <p>From <code className="project-location">{m.from}</code> to <code className="project-location">{m.to}</code>.</p>
        <p>Its lanes’ branches and checkouts, its trust and rules, and its conversation history come with it.</p>
        {refused
          ? <Banner tone="error">{refusalText(m.refusal!, m.from, workspace, name, (lane) => titles[lane]?.title)}</Banner>
          : <Banner tone="warn">Terminals and editors that have this folder open will need to reopen it from the new place.</Banner>}
      </div>
      <div className="actions">
        {refused ? <button className="primary" onClick={closeMove}>OK</button> : <>
          <button className="primary" disabled={m.phase === "moving"} aria-live="polite" onClick={() => void answerMove(true)}>{m.phase === "moving" ? "Moving…" : "Move"}</button>
          <button disabled={m.phase === "moving"} onClick={() => void answerMove(false)}>Keep it here</button>
        </>}
      </div>
    </Dialog>
  );
}

/// Whether a project is outside the workspace, for the menu item that offers the move. Asked of
/// the shell once per row: both paths resolved, so a folder reached through a link compares true.
export function useOutsideWorkspace(path: string): boolean {
  const [outside, setOutside] = useState(false);
  useEffect(() => {
    let live = true;
    void invoke<boolean>("in_workspace", { path }).then((inside) => { if (live) setOutside(!inside); }, () => undefined);
    return () => { live = false; };
  }, [path]);
  return outside;
}
