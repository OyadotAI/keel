// Durable request IDs survive lost responses and remounts for status-only recovery.
import { get, post, type Endpoint } from "../api";
import type { Owner, Receipt, TaskView } from "./model";
import { refreshDelivery, deliveryChanged } from "./state";
const SAVED = "keel.delivery.receipts.v1";
const requests = new Map<string, string>();
try { for (const [key, value] of Object.entries(JSON.parse(localStorage.getItem(SAVED) ?? "{}"))) if (typeof value === "string") requests.set(key, value); } catch { /* Explicit actions can still obtain a durable server receipt. */ }
/** Identical intent reuses its receipt; changed inputs receive a new one. */
export function requestId(scope: string, input: unknown) {
  const key = JSON.stringify([scope, input]);
  if (!requests.has(key)) { requests.set(key, crypto.randomUUID()); try { localStorage.setItem(SAVED, JSON.stringify(Object.fromEntries(requests))); } catch { /* Keep the live request. */ } }
  return requests.get(key)!;
}
/** A confirmed repeatable action may obtain a fresh ID on the person's next request. */
export function forgetRequest(id: string) {
  for (const [key, value] of requests) if (value === id) requests.delete(key);
  try { localStorage.setItem(SAVED, JSON.stringify(Object.fromEntries(requests))); } catch { /* In-memory confirmation still prevents duplicate live requests. */ }
}
/** Recovery only reads a saved operation and never launches an agent. */
export const receiptStatus = (ep: Endpoint, id: string) => get<Receipt>(ep, `/api/delivery/receipts/${encodeURIComponent(id)}`);
/** Opening a saved owner never creates a checkout or submits a prompt. */
export const recoverOwner = (ep: Endpoint, task: string) => get<Owner>(ep, `/api/tasks/${encodeURIComponent(task)}/recover`);
/** Optimistic revisions reject concurrent edits without replacing the local draft. */
export async function mutateTask<T>(ep: Endpoint, view: TaskView, path: string, input: unknown) {
  const result = await post<T>(ep, `/api/tasks/${encodeURIComponent(view.record.task)}/${path}`, input); deliveryChanged(ep); return result;
}
/** Watches only one named operation while active, independently of tab remounts. */
export async function watchReceipt(ep: Endpoint, id: string, lane?: string): Promise<Receipt> {
  let receipt = await receiptStatus(ep, id);
  while (["reserved", "preparing", "running", "accepted", "stopping"].includes(receipt.phase)) {
    await new Promise((resolve) => setTimeout(resolve, 1000)); receipt = await receiptStatus(ep, id); deliveryChanged(ep);
  }
  if (lane) await refreshDelivery(ep, lane); return receipt;
}
