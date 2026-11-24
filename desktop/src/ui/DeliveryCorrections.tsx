// Sending feedback preserves unresolved state and makes the chat transition an explicit choice.
import { useState } from "react";
import type { Endpoint } from "../api";
import { useStore } from "../store";
import { mutateTask, requestId, watchReceipt,useCorrectionDraft, type TaskView, type Receipt, type Correction } from "../delivery";

/** Each item keeps its original range and delivery history after the agent responds. */
export function DeliveryCorrections({ ep, lane, view }: { ep: Endpoint; lane: string; view: TaskView }) {
  const [adding, setAdding] = useState(false);
  const scope=JSON.stringify([view.record.owner.checkout,view.record.task]);
  const [draft,setDraft,clearDraft]=useCorrectionDraft(`${scope}:new`), [error, setError] = useState(""), [busy, setBusy] = useState(false), [status, setStatus] = useState("");
  const terminal = useStore((s) => s.lanes[lane]?.interface === "terminal"), running = useStore((s) => s.lanes[lane]?.running);
  const record = view.record, unresolved = record.corrections.filter((c) => c.resolution === "unresolved"), unavailable = !!view.owner_error || !!view.control_error;
  async function act(action: () => Promise<unknown>) { setBusy(true); setError(""); try { await action(); return true; } catch (error) { setError(String(error)); return false; } finally { setBusy(false); } }
  async function send() {
    if (!view.owner) return;
    const selected = Object.fromEntries(unresolved.map((c) => [c.id, c.revision]));
    const input = { owner: view.owner, selected, remote_head: record.feedback?.head ?? null }, receipt = requestId(`batch:${record.task}`, input);
    const result = await mutateTask<Receipt>(ep, view, "batches", { ...input, receipt }); setStatus(`Correction batch: ${result.phase}`);
    if (result.phase !== "queued") { const latest = await watchReceipt(ep, receipt, lane); setStatus(`Correction batch: ${latest.phase}${latest.error ? ` · ${latest.error}` : ""}`); }
  }
  return <section className="delivery-corrections"><div className="delivery-section-head"><h3 title="Sending feedback and finishing a response leave corrections unresolved. Resolve them after inspection.">Corrections{unresolved.length ? <span className="tab-count">{unresolved.length}</span> : null}</h3>{!adding && <button className="link" onClick={() => setAdding(true)}>Add correction</button>}</div>
    {adding && <><textarea autoFocus className="field" aria-label="Add a task correction" value={draft} onChange={(event) => setDraft(event.target.value)} placeholder="What should change? Select a diff line to anchor a correction." /><div className="actions"><button disabled={unavailable || busy || !draft.trim()} onClick={() => void act(async () => { await mutateTask(ep, view, "corrections", { revision: record.revision, body: draft, anchor: null }); clearDraft(); setAdding(false); })}>Save correction</button><button className="link" onClick={() => setAdding(false)}>Cancel</button></div></>}
    <div className="actions">{terminal ? <button disabled={busy || running} onClick={() => void act(async () => { await useStore.getState().switchInterface(lane, "chat"); })}>Continue this task in chat</button> : unresolved.length > 0 && <button disabled={unavailable || busy} onClick={() => void act(send)}>Send {unresolved.length} to the agent</button>}
      {record.publication?.url && <button disabled={unavailable || busy} onClick={() => void act(async () => { await mutateTask(ep, view, "feedback", {}); })}>Refresh PR feedback</button>}</div>
    {terminal && <p className="small muted">Stop active terminal work first, then explicitly continue with the same provider in chat to send a durable batch.</p>}
    {record.feedback && <p className="small muted">Feedback at {record.feedback.head.slice(0, 12)} · {new Date(record.feedback.fetched_at).toLocaleString()} · {record.feedback.complete ? "Complete read" : "Incomplete; prior corrections retained"}{record.feedback.errors.map((e) => ` · ${e}`)}</p>}
    <ul className="delivery-correction-list">{record.corrections.map((correction) => <CorrectionRow scope={scope} key={correction.id} correction={correction} disabled={unavailable || busy} update={(patch) => act(async () => { await mutateTask(ep,view,`corrections/${encodeURIComponent(correction.id)}`,{revision:record.revision,body:null,resolved:null,...patch}); })} />)}</ul>
    {status && <p role="status">{status}</p>}{error && <p className="error" role="alert">{error}</p>}
  </section>;
}

function CorrectionRow({ correction, disabled, update,scope }: { correction: Correction; disabled: boolean; scope:string; update: (patch: {body?:string;resolved?:boolean}) => Promise<boolean> }) {
  const [editing,setEditing]=useState(false),[body,setBody,clearDraft]=useCorrectionDraft(`${scope}:${correction.id}`,correction.body);
  const anchor=correction.anchor;
  return <li><div><span className="eyebrow">{correction.resolution.replaceAll("_"," ")}</span>{anchor && <p className="small"><code>{anchor.path}:{anchor.first ?? "file"}–{anchor.last ?? ""}</code> · {anchor.side} · {anchor.identity.head.slice(0,7)}</p>}{correction.outdated && <p className="small muted">Original location is historical.</p>}{editing ? <label>Edit correction<textarea className="field" value={body} onChange={(event)=>setBody(event.target.value)} /><div className="actions"><button disabled={disabled ||!body.trim()} onClick={()=>void update({body}).then((saved)=>{if(saved){clearDraft();setEditing(false);}})}>Save edit</button><button onClick={()=>setEditing(false)}>Cancel edit</button></div></label> : <p className="delivery-note">{correction.body}</p>}{correction.url && <a href={correction.url}>Source feedback</a>}<p className="small muted">Sent in {correction.deliveries.length} batch{correction.deliveries.length===1?"":"es"}</p></div><div className="actions"><button disabled={disabled} onClick={()=>void update({resolved:correction.resolution==="unresolved"})}>{correction.resolution==="unresolved"?"Resolve":"Reopen"}</button><button disabled={disabled} onClick={()=>{setEditing(true);}}>Edit</button></div></li>;
}
