// A single durable window draft is shared by every PR surface and keeps edits through remounts.
import { useSyncExternalStore } from "react";
import type { PrForm } from "./model";
interface Draft { form: PrForm; dirty: boolean }
const KEY = "keel.delivery.forms.v1", drafts = new Map<string, Draft>(), listeners = new Set<() => void>();
try { for (const [key, draft] of Object.entries(JSON.parse(localStorage.getItem(KEY) ?? "{}"))) { const value = draft as Draft; if (typeof value.form?.body === "string" && typeof value.form.title === "string") drafts.set(key, value); } } catch { /* Server drafts stay available. */ }
const keyOf = (task: string, checkout: string) => JSON.stringify([checkout, task]);
function save(key: string, draft: Draft) {
  drafts.set(key, draft); try { localStorage.setItem(KEY, JSON.stringify(Object.fromEntries(drafts))); } catch { /* Keep the live edit. */ }
  listeners.forEach((listener) => listener());
}
/** The daemon's saved form replaces only a clean draft, preserving unsent edits on refresh. */
export function prDraft(task: string, checkout: string, form: PrForm): Draft {
  const key = keyOf(task, checkout), before = drafts.get(key);
  if (!before || (!before.dirty && JSON.stringify(before.form) !== JSON.stringify(form))) drafts.set(key, { form, dirty: false });
  return drafts.get(key)!;
}
/** Both PR views edit the same cached value; publication captures that value independently. */
export function editPrDraft(task: string, checkout: string, form: PrForm) { save(keyOf(task, checkout), { form: { ...form, edited: true }, dirty: true }); }
/** Confirmation clears only the exact saved draft, preserving any newer keystrokes. */
export function savedPrDraft(task: string, checkout: string, form: PrForm) {
  const key = keyOf(task, checkout); if (JSON.stringify(drafts.get(key)?.form) === JSON.stringify(form)) save(key, { form, dirty: false });
}
/** Shared draft updates do not require a lane or terminal remount. */
export function usePrDraft(task: string, checkout: string, form: PrForm) {
  const snapshot = () => prDraft(task, checkout, form);
  return useSyncExternalStore((listener) => { listeners.add(listener); return () => { listeners.delete(listener); }; }, snapshot, snapshot);
}
