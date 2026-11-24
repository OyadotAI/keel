// Stage or discard one hunk of the unstaged diff, from its header. The hunk is addressed by the id
// the daemon gave it (a hash of its text), so a file the agent moved since it was drawn is refused
// rather than acted on at the wrong place.
import { useState } from "react";
import { post } from "../api";
import type { HunkRow } from "../git";
import { useStore } from "../store";
import { Dialog } from "./kit";

type Action = "stage" | "discard";
const BUSY: Record<Action, string> = { stage: "Staging…", discard: "Discarding…" };

/// The Stage and Discard links on one hunk header; Discard asks first. Errors go to the diff's
/// banner through `onError`, and the diff is read again either way.
export function HunkActions({ lane, path, hunk, onError, reload }: { lane: string; path: string; hunk: HunkRow; onError: (e?: string) => void; reload: () => void }) {
  const ep = useStore((s) => s.projects[s.lanes[lane]?.project ?? ""]?.endpoint);
  const wt = useStore((s) => s.lanes[lane]?.wt);
  const [busy, setBusy] = useState<Action>();
  const [confirm, setConfirm] = useState(false);
  async function run(action: Action) {
    if (!ep || busy) return;
    setBusy(action);
    onError(undefined);
    try {
      await post(ep, "/api/git/act", { action: `${action}-hunk`, path, hunk: hunk.id }, { wt });
    } catch (e) {
      onError(`Couldn't ${action} this hunk: ${e instanceof Error ? e.message : String(e)}`);
    } finally {
      setBusy(undefined);
      reload();
    }
  }
  const file = path.split("/").at(-1);
  return (
    <span className="hunk-actions">
      {busy ? <span className="muted">{BUSY[busy]}</span> : <>
        <button className="link" aria-label={`Stage hunk ${hunk.n} of ${hunk.of}`} onClick={() => void run("stage")}>Stage</button>
        <button className="link" aria-label={`Discard hunk ${hunk.n} of ${hunk.of}`} onClick={() => setConfirm(true)}>Discard</button>
      </>}
      {confirm && (
        <Dialog title="Discard this hunk?" tone="danger" onClose={() => setConfirm(false)}>
          <p>Lines {hunk.first}–{hunk.last} of {file} go back to the last staged version. This cannot be undone.</p>
          <div className="actions">
            <button className="danger" onClick={() => { setConfirm(false); void run("discard"); }}>Discard hunk</button>
            <button onClick={() => setConfirm(false)}>Cancel</button>
          </div>
        </Dialog>
      )}
    </span>
  );
}
