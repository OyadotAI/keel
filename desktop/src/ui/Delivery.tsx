// Delivery keeps source, execution, check identity and publication as independently inspectable facts.
import { useState } from "react";
import { type Endpoint } from "../api";
import { useStore } from "../store";
import { mutateTask, requestId, receiptStatus, refreshDelivery, watchReceipt, type Receipt, type TaskView } from "../delivery";
import { DeliveryCorrections } from "./DeliveryCorrections";
import { DeliveryPublication } from "./DeliveryPublication";
import { TaskHandoff } from "./TaskHandoff";
import { Floating } from "./Menu";
import { Icon } from "./icons";
import { agentName } from "../store";

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
    <DeliveryHeading view={view} />
    <div className="delivery-status-row"><CheckStatus view={view} />
      <button disabled={unavailable || busy || l?.running} onClick={() => void runChecks()}>{busy ? "Checking…" : "Run checks"}</button>
      <DeliveryMenu refresh={() => void refreshDelivery(ep, lane)} merge={l?.wt ? { disabled: unavailable || !view.checks_passed || !!l.running, run: () => ask("merge") } : undefined} discard={l?.wt ? { disabled: !!l.running, run: () => ask("discard") } : undefined} /></div>
    {(view.owner_error || view.control_error || view.evidence_error) && <p className="error">{view.owner_error || view.control_error || view.evidence_error}</p>}
    <details className="delivery-section"><summary>Details</summary><DeliveryFacts view={view} running={!!l?.running} ended={!!l?.conv.turns.at(-1)?.ended} /><CheckDetails view={view} /><SavedOperations view={view} inspect={inspect} retry={retry} stop={stop} /></details>
    <DeliveryCorrections ep={ep} lane={lane} view={view} /><DeliveryPublication ep={ep} lane={lane} view={view} />
    {operation && <p role="status">{operation.kind}: {operation.phase}{operation.error && ` · ${operation.error}`}</p>}{error && <p className="error" role="alert">{error}</p>}<TaskHandoff lane={lane} />
  </div>;
}

function DeliveryHeading({ view }: { view: TaskView }) {
  const record = view.record, source = record.source.snapshot;
  return <header className="delivery-head"><h2>{record.title || "Conversation task"}</h2><p className="delivery-meta">{source ? <a href={source.url}>{source.repository}#{source.number}</a> : record.source.kind === "recurring" ? <span>Recurring: {record.source.name}</span> : <span>Conversation</span>}<span>·</span><span>{view.owner ? agentName(view.owner.provider) : "Owner unconfirmed"}</span></p>{source && <details className="delivery-section"><summary>Issue source</summary><pre className="delivery-source">{source.body}</pre></details>}</header>;
}

/// The one line that says where the task stands: did the project's checks pass, on this checkout.
function CheckStatus({ view }: { view: TaskView }) {
  const check = view.record.checks.at(-1);
  const tone = !check ? "none" : check.status === "passed" ? (view.checks_current ? "pass" : "stale") : "fail";
  const words = !check ? "No checks yet" : view.checks_current ? `Checks ${check.status}` : `Checks ${check.status} on an earlier commit`;
  return <span className={`check-status ${tone}`}><span className="check-dot" aria-hidden="true" />{words}</span>;
}

/// Refreshing, merging and discarding are rare next to running checks: one click away under ⋯.
function DeliveryMenu({ refresh, merge, discard }: { refresh: () => void; merge?: { disabled: boolean; run: () => void }; discard?: { disabled: boolean; run: () => void } }) {
  const [at, setAt] = useState<DOMRect | null>(null);
  const pick = (run: () => void) => () => { setAt(null); run(); };
  return <><button className="ghost" aria-label="More task actions" aria-haspopup="menu" onClick={(e) => setAt(e.currentTarget.getBoundingClientRect())}><Icon name="more" size={15} /></button>
    {at && <Floating anchor={at} onClose={() => setAt(null)}>
      <button onClick={pick(refresh)}>Refresh evidence</button>
      {merge && <button disabled={merge.disabled} onClick={pick(merge.run)}>Merge…</button>}
      {discard && <><div className="sep" /><button className="danger-item" disabled={discard.disabled} onClick={pick(discard.run)}>Discard…</button></>}
    </Floating>}</>;
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
