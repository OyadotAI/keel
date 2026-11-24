// One editable PR form feeds advisory review and explicit immutable publication.
import { useEffect, useState } from "react";
import { post, type Endpoint } from "../api";
import { useStore } from "../store";
import { usePrDraft, editPrDraft, savedPrDraft, mutateTask, requestId, watchReceipt, refreshDelivery,publicationReceipt, type TaskView, type Receipt, type PrForm } from "../delivery";
import { startReview, useReview } from "../prereview";
import { PreReview } from "./PreReview";

/** Source and factual verification are seeded once; later review reads cannot overwrite edits. */
export function DeliveryPublication({ ep, lane, view }: { ep: Endpoint; lane: string; view: TaskView }) {
  const record = view.record, checkout = (view.owner ?? record.owner).checkout, wt = view.owner?.wt ?? undefined;
  const draft = usePrDraft(record.task, checkout, record.form), form = draft.form;
  const [error, setError] = useState(""), [busy, setBusy] = useState(""), [receipt, setReceipt] = useState<Receipt>();
  const review = useReview(lane, ep, wt), running = useStore((s) => s.lanes[lane]?.running);
  const unavailable = !!view.owner_error || !!view.control_error;
  const observed=publicationReceipt(receipt,view.operations);
  useEffect(() => { if (!record.form.seeded && !unavailable) void post(ep, `/api/tasks/${record.task}/form/seed`, {}).then(() => refreshDelivery(ep, lane)).catch((error) => setError(String(error))); }, [ep, lane, record.task, record.form.seeded, unavailable]);
  async function save() { await mutateTask(ep, view, "form", { revision: record.revision, form }); savedPrDraft(record.task, checkout, form); }
  async function act(label: string, work: () => Promise<unknown>) { setBusy(label); setError(""); try { await work(); } catch (error) { setError(String(error)); } finally { setBusy(""); } }
  async function publish() {
    if (!view.owner || !view.identity) return;
    const input = { revision: record.revision, owner: view.owner, identity: view.identity, form }, id = requestId(`publish:${record.task}`, input);
    const receipt = await mutateTask<Receipt>(ep, view, "publication", { receipt: id, ...input }); setReceipt(receipt); setReceipt(await watchReceipt(ep, id, lane));
  }
  const edit = (patch: Partial<typeof form>) => editPrDraft(record.task, checkout, { ...form, ...patch });
  return <section className="delivery-publication" aria-label="Pull request"><details className="delivery-section" open={!!record.publication}><summary>Pull request <span className="mono">{record.branch || "task branch"} → {form.base || record.base}</span></summary>
    <label>Title<input className="field" value={form.title} onChange={(event) => edit({ title: event.target.value })} aria-label="Pull request title" /></label><label>Body<textarea className="field delivery-pr-body" value={form.body} onChange={(event) => edit({ body: event.target.value })} aria-label="Pull request body" /></label>
    <label className="pr-draft"><input type="checkbox" checked={form.draft} onChange={(event) => edit({ draft: event.target.checked })} />Open as a draft</label><p className="small muted">{draft.dirty ? "Draft edits retained in this window. Save to the task or publish to preserve them with the shared task." : "Draft saved with this task."} Review is advisory; publication is your choice.</p>
    <PublicationControls busy={busy} unavailable={unavailable} dirty={draft.dirty} running={!!running} reviewPending={!!review?.operation} view={view} form={form} save={() => act("Saving…", save)} review={() => act("Reviewing…", async () => { await save(); await startReview(lane,ep,wt); })} publish={() => act("Publishing…",publish)} />
    <PreReview lane={lane} ep={ep} wt={wt} place="tab" />
    <SavedAdvisory view={view} />
    <PublicationResult view={view} />
    </details>
    {busy && <p role="status">{busy}</p>}{observed && <p role="status">Publication: {observed.phase}{observed.error && ` · ${observed.error}`}</p>}{error && <p className="error" role="alert">{error}</p>}
  </section>;
}

function PublicationControls({ busy, unavailable, dirty, running, reviewPending, view, form, save, review, publish }: { busy: string; unavailable: boolean; dirty: boolean; running: boolean; reviewPending: boolean; view: TaskView; form: PrForm; save: () => Promise<void>; review: () => Promise<void>; publish: () => Promise<void> }) {
  return <><div className="actions"><button disabled={!!busy || unavailable || !dirty} onClick={() => void save()}>Save draft</button><button disabled={!!busy || unavailable || running || reviewPending} onClick={() => void review()}>Review branch</button><button className="primary" disabled={!!busy || unavailable || running || !view.identity || !form.seeded} onClick={() => void publish()}>{busy === "Publishing…" ? busy : view.record.publication?.url ? "Update pull request" : "Publish captured commit"}</button></div><p className="small muted">Publication captures commit {view.identity?.head.slice(0,12) ?? "unavailable"}. Current checks: {view.checks_current ? view.record.checks.at(-1)?.status : "unconfirmed or historical"}. {view.record.corrections.filter((c) => c.resolution === "unresolved").length} unresolved corrections.</p></>;
}
function PublicationResult({ view }: { view: TaskView }) {
  const publication = view.record.publication;
  return publication && <p>{publication.url && <a href={publication.url}>Open pull request</a>} · {publication.status} · requested {publication.intent.identity.head.slice(0,12)} · published {publication.actual_head?.slice(0,12) ?? "unconfirmed"}{publication.error && ` · ${publication.error}`}</p>;
}
function SavedAdvisory({view}:{view:TaskView}) {
  const review=view.record.reviews?.at(-1);
  if(!review)return <p className="small muted">Saved advisory review: not recorded.</p>;
  const current=review.confirmed &&!view.control_error &&review.sha===view.identity?.head;
  return <details><summary>Saved advisory review: {review.verdict} · {current?"current commit":"historical or unconfirmed"}</summary><p className="small">{review.provider} · {review.sha.slice(0,12)} against {review.base} · {review.completed_at?new Date(review.completed_at).toLocaleString():"Completion unconfirmed"}</p><pre className="delivery-source">{review.report}</pre></details>;
}
