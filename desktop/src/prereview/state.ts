// One confirmed snapshot per active lane context feeds the full tab and compact dock.
import { useEffect, useSyncExternalStore } from "react";
import { get, ORDINARY, type Endpoint } from "../api";
import type { ReviewForm, Seen, Snapshot } from "./model";
import { FORM_KEY, FORM_LIMIT } from "./constants";

const seen = new Map<string, Snapshot>();
const listeners = new Set<() => void>();
const requests = new Map<string, number>();
const forms = new Map<string, ReviewForm>();
let restored = false;
const reading = new Map<string, { generation: symbol; promise: Promise<boolean> }>();
const changed = () => listeners.forEach((l) => l());
const context = (ep: Endpoint, wt?: string) => JSON.stringify([ep.port, ep.token, wt]);
const formKey = (lane: string, wt?: string) => JSON.stringify([lane, wt]);

/** Form identity excludes daemon credentials so reconnects preserve entered title/draft choices. */
export function reviewForm(lane: string, wt?: string): ReviewForm {
  const key = formKey(lane, wt);
  if (!restored) {
    restored = true;
    try {
      const saved = JSON.parse(localStorage.getItem(FORM_KEY) ?? "{}");
      for (const [id, value] of Object.entries(saved).slice(-FORM_LIMIT)) {
        const form = value as ReviewForm;
        if (form && typeof form.title === "string" && typeof form.draft === "boolean") forms.set(id, { title: form.title, draft: form.draft, edited: !!form.edited });
      }
    } catch { /* The in-memory draft still survives tab remounts. */ }
  }
  if (!forms.has(key)) forms.set(key, { title: "", draft: false, edited: false });
  return forms.get(key)!;
}

function saveForm(lane: string, wt: string | undefined, value: ReviewForm) {
  forms.set(formKey(lane, wt), value);
  while (forms.size > FORM_LIMIT) forms.delete(forms.keys().next().value!);
  try { localStorage.setItem(FORM_KEY, JSON.stringify(Object.fromEntries(forms))); } catch { /* Keep the current window's form. */ }
}

/** Selecting a new endpoint/checkout invalidates all responses and operations from the old one. */
export function bindReview(lane: string, ep: Endpoint, wt?: string): Snapshot {
  const key = context(ep, wt), before = seen.get(lane);
  if (before?.context === key) return before;
  requests.set(lane, (requests.get(lane) ?? 0) + 1);
  const snapshot: Snapshot = { context: key, generation: Symbol(), review: null, head: "", confirmed: false, loading: true, form: reviewForm(lane, wt) };
  seen.set(lane, snapshot); changed(); return snapshot;
}

/** Late callers can inspect only the context they captured; they cannot rebind it. */
export function currentReview(lane: string, ep: Endpoint, wt?: string): Snapshot | undefined {
  const snapshot = seen.get(lane);
  return snapshot?.context === context(ep, wt) ? snapshot : undefined;
}

/** A context generation remains stable through updates and changes only on replacement. */
export function patchReview(lane: string, captured: Snapshot, patch: Partial<Snapshot>): void {
  const current = seen.get(lane);
  if (current?.generation !== captured.generation) return;
  seen.set(lane, { ...current, ...patch }); changed();
}

/** Public compatibility reading; actions use the stricter endpoint/checkout contract. */
export const reviewOf = (lane: string): Snapshot | undefined => seen.get(lane);

/** Both mounted surfaces share loading, form, operation, and recovery state. */
export function useReview(lane: string, ep?: Endpoint, wt?: string): Snapshot | undefined {
  useEffect(() => { if (ep) { bindReview(lane, ep, wt); void refreshReview(lane, ep, wt); } }, [lane, ep, wt]);
  const snapshot = () => ep ? currentReview(lane, ep, wt) : seen.get(lane);
  return useSyncExternalStore((listener) => { listeners.add(listener); return () => { listeners.delete(listener); }; }, snapshot, snapshot);
}

/** Editing never changes immutable opening metadata captured by an earlier review. */
export function updateReviewForm(lane: string, ep: Endpoint, wt: string | undefined, patch: Partial<Pick<ReviewForm, "title" | "draft">>): void {
  const entry = currentReview(lane, ep, wt) ?? (!seen.has(lane) ? bindReview(lane, ep, wt) : undefined);
  if (!entry) return;
  const form = { ...entry.form, ...patch, edited: true };
  saveForm(lane, wt, form); patchReview(lane, entry, { form });
}

/** Starting a mutation invalidates reads initiated before it, even if their response arrives later. */
export function invalidateReviewReads(lane: string): void { requests.set(lane, (requests.get(lane) ?? 0) + 1); reading.delete(lane); }

function reconciled(lane: string, wt: string | undefined, before: Snapshot, now: Seen): Partial<Snapshot> {
  const wasRunning = !!before.review && !before.review.outcome;
  let form = before.form;
  if (now.review && !form.edited) { form = { title: now.review.opening.title, draft: now.review.opening.draft, edited: false }; saveForm(lane, wt, form); }
  return { ...now, form, loading: false, confirmed: true, delivery: now.review?.id === before.review?.id ? before.delivery : undefined, restarted: !now.review && (wasRunning || !!before.restarted), error: before.error?.action === "read" ? undefined : before.error };
}

async function read(lane: string, ep: Endpoint, wt: string | undefined, captured: Snapshot, timeout: number): Promise<boolean> {
  const ticket = (requests.get(lane) ?? 0) + 1; requests.set(lane, ticket);
  patchReview(lane, captured, { loading: true });
  try {
    const now = await get<Seen>(ep, "/api/prereview", { lane, wt }, timeout);
    const before = currentReview(lane, ep, wt);
    if (before?.generation !== captured.generation || requests.get(lane) !== ticket) return false;
    if (!now || typeof now.head !== "string" || !("review" in now) || (now.review && (!now.review.id || !Number.isFinite(now.review.revision)))) throw new Error("The review server returned incomplete status. Reconnect and refresh it.");
    if (now.review?.id === before.review?.id && now.review && before.review && now.review.revision < before.review.revision) throw new Error("The server returned an older review status. Refresh again.");
    patchReview(lane, before, reconciled(lane, wt, before, now));
    return true;
  } catch (error) {
    if (requests.get(lane) !== ticket) return false;
    patchReview(lane, captured, { loading: false, confirmed: false, error: { action: "read", message: "Couldn’t refresh review status. Last confirmed results are shown below.", details: error instanceof Error ? error.message : String(error) } });
    return false;
  }
}

/** Read failures retain useful findings but revoke their current/confirmed presentation. */
export function refreshReview(lane: string, ep: Endpoint, wt?: string, force = false, timeout = ORDINARY): Promise<boolean> {
  const entry = currentReview(lane, ep, wt) ?? (!seen.has(lane) ? bindReview(lane, ep, wt) : undefined);
  if (!entry) return Promise.resolve(false);
  const pending = reading.get(lane);
  if (!force && pending?.generation === entry.generation) return pending.promise;
  const promise = read(lane, ep, wt, entry, timeout);
  const owner = { generation: entry.generation, promise }; reading.set(lane, owner);
  void promise.finally(() => { if (reading.get(lane) === owner) reading.delete(lane); });
  return promise;
}
