// All review and dock publication actions share the persisted draft and durable publication service.
import { get, post, type Endpoint } from "../api";
import type { TaskView, PrForm, Receipt } from "./model";
import { prDraft, savedPrDraft } from "./drafts";
import { requestId, watchReceipt } from "./actions";
import { refreshDelivery } from "./state";

/** A retry from another surface updates the same receipt without an older view regressing completion. */
export function publicationReceipt(receipt:Receipt|undefined,operations:Receipt[]=[]):Receipt|undefined {
  return receipt?.phase==="completed"?receipt:operations.find(operation=>operation.id===receipt?.id)??receipt;
}

/** Explicit review/publication reads preserve the current local edit over an older saved form. */
export async function prepareTaskForm(ep: Endpoint, lane: string) {
  let view = await get<TaskView | null>(ep, "/api/delivery/by-lane", { lane });
  if (!view) return undefined;
  if (!view.record.form.seeded) {
    await post<PrForm>(ep, `/api/tasks/${view.record.task}/form/seed`, {});
    view = await get<TaskView>(ep, `/api/tasks/${view.record.task}`);
  }
  const checkout = (view.owner ?? view.record.owner).checkout;
  const draft = prDraft(view.record.task, checkout, view.record.form);
  return { view, form: draft.form };
}
/** Dock and review actions cannot fall back to a second mutable-current-branch publisher. */
export async function publishTask(ep: Endpoint, lane: string, prepared: { view: TaskView; form: PrForm }) {
  const { view, form } = prepared;
  if (!view.owner || !view.identity || view.owner_error || view.control_error) throw new Error(view.owner_error ?? view.control_error ?? "Task checkout is unavailable.");
  const input = { owner: view.owner, identity: view.identity, revision: view.record.revision, form }, receipt = requestId(`publish:${view.record.task}`, input);
  await post<Receipt>(ep, `/api/tasks/${view.record.task}/publication`, { ...input, receipt });
  const status = await watchReceipt(ep, receipt, lane);
  if (status.phase !== "completed") throw new Error(status.error ?? `Publication is ${status.phase}; inspect its saved request.`);
  savedPrDraft(view.record.task, view.owner.checkout, form); await refreshDelivery(ep, lane); return true;
}
