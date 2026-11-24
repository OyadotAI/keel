// Delivery keeps source, execution, check identity and publication as independently inspectable facts.
import { useState } from "react";
import { type Endpoint } from "../api";
import { useStore } from "../store";
import { mutateTask, requestId, receiptStatus, refreshDelivery, watchReceipt, type Receipt, type TaskView } from "../delivery";
import { DeliveryCorrections } from "./DeliveryCorrections";
import { DeliveryPublication } from "./DeliveryPublication";
import { TaskHandoff } from "./TaskHandoff";

/** Evidence remains readable when ownership is unavailable, with mutations disabled until recovery. */
export function Delivery({ ep, lane, view, ask }: { ep: Endpoint; lane: string; view: TaskView; ask: (what: "merge" | "discard") => void }) {
  const l = useStore((s) => s.lanes[lane]), [error, setError] = useState(""), [busy, setBusy] = useState(false), [operation, setOperation] = useState<Receipt>();
  const record = view.record, check = record.checks.at(-1);
  const unavailable = !!view.owner_error || !!view.control_error;
  async function runChecks() {
    if (!view.owner) return; setBusy(true); setError("");
    const input = { owner: view.owner }, receipt = requestId(`checks:${record.task}`, { ...input, identity: view.identity, previous: check?.id });
    try { setOperation(await mutateTask<Receipt>(ep, view, "checks", { ...input, receipt })); setOperation(await watchReceipt(ep, receipt, lane)); }
    catch (error) { setError(`${error} Saved check request: ${receipt}`); } finally { setBusy(false); }
  }
  async function inspect(id: string) { try { setOperation(await receiptStatus(ep, id)); await refreshDelivery(ep, lane); } catch (error) { setError(String(error)); } }
  async function retry(receipt: Receipt) { try { await mutateTask(ep,view,`publication/${receipt.id}/retry`,{});setOperation(await watchReceipt(ep,receipt.id,lane)); } catch(error) {setError(String(error));} }
  async function stop(receipt: Receipt) { try {await mutateTask(ep,view,`checks/${receipt.id}/stop`,{});await refreshDelivery(ep,lane);}catch(error){setError(String(error));} }
  return <div className="delivery-panel">
    <DeliveryHeading view={view} /><DeliveryFacts view={view} running={!!l?.running} ended={!!l?.conv.turns.at(-1)?.ended} />
    {(view.owner_error || view.control_error || view.evidence_error) && <p className="error">{view.owner_error || view.control_error || view.evidence_error}</p>}
    <div className="actions"><button disabled={unavailable || busy || l?.running} onClick={() => void runChecks()}>{busy ? "Checking…" : "Run project checks"}</button><button onClick={() => void refreshDelivery(ep, lane)}>Refresh evidence</button>{l?.wt && <><button disabled={unavailable || !view.checks_passed || !!l.running} onClick={() => ask("merge")}>Merge…</button><button disabled={!!l.running} onClick={() => ask("discard")}>Discard…</button></>}</div>
    <CheckDetails view={view} />
    <DeliveryCorrections ep={ep} lane={lane} view={view} /><DeliveryPublication ep={ep} lane={lane} view={view} />
    <SavedOperations view={view} inspect={inspect} retry={retry} stop={stop} />
    {operation && <p role="status">{operation.kind}: {operation.phase}{operation.error && ` · ${operation.error}`}</p>}{error && <p className="error" role="alert">{error}</p>}<TaskHandoff lane={lane} />
  </div>;
}

function DeliveryHeading({ view }: { view: TaskView }) {
  const record = view.record, source = record.source.snapshot;
  return <header><p className="eyebrow">Delivery</p><h2>{record.title || "Conversation task"}</h2>{source ? <a href={source.url}>{source.repository}#{source.number}</a> : record.source.kind === "recurring" && <p className="small">Recurring: {record.source.name} · revision {record.source.revision}</p>}{source && <details><summary>Saved issue source</summary><pre className="delivery-source">{source.body}</pre></details>}<p className="small muted">Current owner: {view.owner?.provider ?? "unconfirmed"} · {view.owner?.lane ?? "unavailable"}</p></header>;
}
function DeliveryFacts({ view, running, ended }: { view: TaskView; running: boolean; ended: boolean }) {
  const check = view.record.checks.at(-1);
  const operation = view.operations?.find((operation) => ["issue","batch","scheduled"].includes(operation.kind));
  const execution = operation ? `${operation.phase}${operation.error ? ` · ${operation.error}` : ""}` : ended ? "Response ended" : "Idle or not observed";
  const checks = view.checks_current ? `${check?.status} · current checkout` : check ? `${check.status} · historical or unconfirmed` : "Not recorded";
  const facts = { Execution: running ? "Running" : execution, "Project checks": checks, "Checked commit": check?.before?.head.slice(0,12) ?? "Unknown", "Local commit": view.identity?.head.slice(0,12) ?? "Unavailable", "Published commit": view.record.publication?.actual_head?.slice(0,12) ?? "Not confirmed", Corrections: `${view.record.corrections.filter((c) => c.resolution === "unresolved").length} unresolved` };
  return <dl className="delivery-facts">{Object.entries(facts).map(([name,value]) => <div key={name} className="delivery-fact"><dt>{name}</dt><dd>{value}</dd></div>)}</dl>;
}
function CheckDetails({ view }: { view: TaskView }) {
  const check = view.record.checks.at(-1);
  return check && <details><summary>Recorded check details</summary><p><code>{check.command ?? "Command unknown"}</code> · {check.completed_at ? new Date(check.completed_at).toLocaleString() : "Completion unconfirmed"}</p>{check.warning && <p>{check.warning}</p>}{check.problems.map((problem,index) => <p key={index}>{problem.file}:{problem.line} · {problem.message}</p>)}</details>;
}
function SavedOperations({ view, inspect, retry, stop }: { view: TaskView; inspect: (id: string) => Promise<void>; retry: (receipt: Receipt) => Promise<void>; stop: (receipt: Receipt) => Promise<void> }) {
  return !!view.operations?.length && <details><summary>Saved operations</summary>{view.operations.map((receipt) => <div className="delivery-operation" key={receipt.id}><div><span>{receipt.kind}: {receipt.phase}{receipt.error && ` · ${receipt.error}`}</span>{receipt.target && <p className="small">Captured {receipt.target.head.slice(0,12)} · {receipt.target.branch} → {receipt.target.base} · {receipt.target.repository}</p>}</div><div className="actions"><button onClick={() => void inspect(receipt.id)}>Check status</button>{receipt.kind === "publication" && receipt.phase !== "completed" && <button onClick={() => void retry(receipt)}>Retry captured publication</button>}{receipt.kind === "check" && receipt.phase === "running" && <button onClick={() => void stop(receipt)}>Stop this check</button>}</div></div>)}</details>;
}
