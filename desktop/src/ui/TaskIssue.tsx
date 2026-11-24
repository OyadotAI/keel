// Preview source and provider choice before creating one isolated durable task.
import { Icon } from "./icons";
import { useEffect,useMemo,useState } from "react";
import { post, type Endpoint } from "../api";
import { AGENT_NAME, type Agent } from "../store";
import { IssuePreviewReader,requestId, receiptStatus, type IssuePreview, type Receipt } from "../delivery";

/** An issue is external source text; preview cannot authorize execution or change permissions. */
export function IssueIntake({ ep, open }: { ep: Endpoint; open: (task: string, title: string) => Promise<void> }) {
  const [url, setUrl] = useState(""), [preview, setPreview] = useState<IssuePreview>(), [provider, setProvider] = useState<Agent>("codex"), [base, setBase] = useState("main");
  const [busy, setBusy] = useState(""), [error, setError] = useState(""), [receipt, setReceipt] = useState<Receipt>(), [pending, setPending] = useState<string>();
  const reader=useMemo(()=>new IssuePreviewReader((url)=>post<IssuePreview>(ep,"/api/delivery/issues/preview",{url})),[ep]);
  useEffect(()=>()=>reader.invalidate(),[reader]);
  const frozen=!!pending &&(!receipt ||["reserved","preparing","accepted","queued","running","unconfirmed"].includes(receipt.phase));
  async function inspect() { setBusy("Previewing…"); setError(""); const result=await reader.read(url);if(!result)return;setBusy("");if(result.error){setError(result.error);return;}setPreview(result.preview);setReceipt(undefined);setPending(undefined); }
  async function start() {
    if (!preview) return;
    const input = { preview: preview.id, provider, base }, id = requestId(`issue:${ep.port}`, input); setPending(id); setBusy("Starting…"); setError("");
    try { const receipt = await post<Receipt>(ep, "/api/delivery/tasks/start", { receipt: id, ...input }); setReceipt(receipt); if (receipt.task && ["accepted", "completed"].includes(receipt.phase)) await open(receipt.task, preview.issue.title); } catch (error) { setError(`${error} Check this saved request before starting another copy.`); } finally { setBusy(""); }
  }
  async function status() { if (pending) try { const next = await receiptStatus(ep, pending); setReceipt(next); if (next.task) await open(next.task, preview?.issue.title ?? "Issue task"); } catch (error) { setError(String(error)); } }
  return <section className="delivery-intake" aria-label="Start from a GitHub issue"><form className="intake-row" onSubmit={(event) => { event.preventDefault(); if (url && !busy && !frozen) void inspect(); }}><Icon name="plus" size={14} /><input aria-label="GitHub issue URL" value={url} disabled={frozen} onChange={(event) => { reader.invalidate();setUrl(event.target.value); setPreview(undefined);setBusy(""); }} placeholder="Start from a GitHub issue — paste its URL" />
    <button type="submit" className="ghost" disabled={!!busy || frozen || !url}>{busy === "Previewing…" ? busy : "Preview"}</button></form>
    {preview && <><a href={preview.issue.url}><strong>{preview.issue.title}</strong></a><p className="small muted">{preview.issue.state} · Changes in {preview.change_repository}{preview.parent && ` · Upstream ${preview.parent}`}</p><details><summary>Issue source</summary><pre className="delivery-source">{preview.issue.body || "No description."}</pre>{preview.issue.truncated && <p>Preview shortened. Read the full issue before starting.</p>}</details>
      <div className="delivery-fields"><label>Agent<select value={provider} disabled={!!busy ||frozen} onChange={(event) => setProvider(event.target.value as Agent)}>{Object.entries(AGENT_NAME).map(([id, name]) => <option key={id} value={id}>{name}</option>)}</select></label><label>Local base branch<input className="field" value={base} disabled={!!busy ||frozen} onChange={(event) => setBase(event.target.value)} /></label></div>
      <p className="small muted">Starts a fresh isolated chat task using this source snapshot and normal project permissions.</p><div className="actions"><button className="primary" disabled={!!busy ||frozen} onClick={() => void start()}>{busy === "Starting…" ? busy : "Start isolated task"}</button>{pending && <button onClick={() => void status()}>Check saved start</button>}</div></>}
    {receipt && <p role="status">Start: {receipt.phase}{receipt.error && ` · ${receipt.error}`}</p>}{error && <p className="error" role="alert">{error}</p>}
  </section>;
}
