// Selected diff ranges keep the original checkout, side and quotation through later edits.
import { useState } from "react";
import { useStore } from "../store";
import { mutateTask, useDelivery,useCorrectionDraft, type Anchor } from "../delivery";
import type { Prepared } from "../git";

/** Shift extends only within one diff side; deleted lines retain their old-side coordinates. */
export interface DiffRange { side: "old" | "new"; first: number; last: number }
/** Keyboard fields provide the same range choice as line-number buttons. */
export function DiffCorrection({ lane, path, diff, range, setRange, close }: { lane: string; path: string; diff: Prepared; range?: DiffRange; setRange: (range: DiffRange | undefined) => void; close: () => void }) {
  const ep = useStore((s) => s.projects[s.lanes[lane]?.project]?.endpoint), snapshot = useDelivery(ep, lane);
  const project=useStore(s=>s.lanes[lane]?.project);
  const scope=JSON.stringify([snapshot.view?.record.owner.checkout??project,snapshot.view?.record.task??lane,path,diff.identity]);
  const [body,setBody,clearDraft]=useCorrectionDraft(scope), [error, setError] = useState(""), [busy, setBusy] = useState(false);
  async function save() {
    if (!ep || !snapshot.view || !diff.identity) return; setBusy(true); setError("");
    const quote = diff.rows.filter((row) => row.kind !== "hunk" && "old" in row && range && (row[range.side] ?? 0) >= range.first && (row[range.side] ?? 0) <= range.last).map((row) => row.text).join("\n");
    const anchor: Anchor = { identity: diff.identity, path, side: range?.side ?? "file", first: range?.first ?? null, last: range?.last ?? null, quote };
    try { await mutateTask(ep, snapshot.view, "corrections", { revision: snapshot.view.record.revision, body, anchor }); clearDraft();close(); } catch (error) { setError(String(error)); } finally { setBusy(false); }
  }
  return <form className="delivery-range-form" onSubmit={(event) => { event.preventDefault(); void save(); }}><strong>Correction · {path}</strong><div className="delivery-range"><label>Side<select value={range?.side ?? "file"} onChange={(event) => setRange(event.target.value === "file" ? undefined : { side: event.target.value as DiffRange["side"], first: 1, last: 1 })}><option value="file">Whole file</option><option value="old">Old / deleted</option><option value="new">New</option></select></label>{range && <><label>From<input aria-label="Correction first line" type="number" min="1" value={range.first} onChange={(event) => setRange({ ...range, first: Number(event.target.value) })} /></label><label>Through<input aria-label="Correction last line" type="number" min={range.first} value={range.last} onChange={(event) => setRange({ ...range, last: Number(event.target.value) })} /></label></>}</div>
    <label>Requested change<textarea className="field" autoFocus disabled={!snapshot.view} value={body} onChange={(event) => setBody(event.target.value)} required /></label><div className="actions"><button className="primary" type="submit" disabled={busy || !snapshot.view || !diff.identity || !body.trim()}>Save correction</button><button type="button" onClick={close}>Cancel</button></div>{!diff.identity && <p>Refresh the diff to capture an original checkout identity.</p>}{error && <p className="error" role="alert">{error}</p>}
  </form>;
}
