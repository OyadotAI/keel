// A question's written answer outlives its card, lane mounts, reconnects, and uncertain delivery.
import type { Pending } from "./store";

/** Provider question identifiers remain separate from display labels that may repeat. */
export interface Question { id?: string; question: string; options?: { label: string; description?: string }[]; multiSelect?: boolean }
/** Custom writing is retained when the person temporarily selects a suggested option. */
export interface QuestionValue { custom: string; selected: string[]; mode: "custom" | "options" }
/** Only confirmed delivery or explicit dismissal removes a recoverable response. */
export interface QuestionDraft { request: Pending; values: Record<number, QuestionValue>; sending?: boolean; error?: string; closed?: boolean; submitted?: unknown }
export type QuestionDrafts = Record<string, QuestionDraft>;

/** Missing provider fields do not turn an invalid question into a submit button. */
export function questions(request: Pending): Question[] {
  return Array.isArray(request.input.questions) ? request.input.questions.filter((q): q is Question => !!q && typeof q.question === "string") : [];
}

/** Create empty values only on editing, without accidentally treating a preselection as an answer. */
export function emptyQuestion(request: Pending): QuestionDraft { return { request, values: {} }; }

/** Select explicit options while retaining previously composed custom text. */
export function pickQuestion(draft: QuestionDraft, index: number, option: string): QuestionDraft {
  const old = draft.values[index] ?? { custom: "", selected: [], mode: "options" };
  const multi = questions(draft.request)[index]?.multiSelect;
  const selected = multi ? old.selected.includes(option) ? old.selected.filter((o) => o !== option) : [...old.selected, option] : [option];
  return { ...draft, error: undefined, submitted: undefined, values: { ...draft.values, [index]: { ...old, selected, mode: "options" } } };
}

/** Typing or focusing custom text restores that answer without deleting remembered option picks. */
export function writeQuestion(draft: QuestionDraft, index: number, custom: string): QuestionDraft {
  const old = draft.values[index] ?? { custom: "", selected: [], mode: "custom" };
  return { ...draft, error: undefined, submitted: undefined, values: { ...draft.values, [index]: { ...old, custom, mode: "custom" } } };
}

/** The currently chosen response, including multiline text exactly as the person wrote it. */
export function questionAnswer(draft: QuestionDraft, index: number): string {
  const value = draft.values[index];
  return !value ? "" : value.mode === "custom" ? value.custom : value.selected.join(", ");
}

/** A recovery message includes partial writing as well as any selected options. */
export function recoveryMessage(draft: QuestionDraft): string {
  return questions(draft.request).flatMap((q, i) => {
    const value = draft.values[i];
    if (!value) return [];
    const answer = [questionAnswer(draft, i), value.mode === "options" ? value.custom : ""].filter((a) => a.trim()).join("\n");
    return answer.trim() ? [`${q.question}\n${answer}`] : [];
  }).join("\n\n");
}

function canonical(value: unknown): string {
  if (Array.isArray(value)) return `[${value.map(canonical).join(",")}]`;
  if (value && typeof value === "object") return "{" + Object.entries(value).sort(([a], [b]) => a.localeCompare(b)).map(([key, item]) => `${JSON.stringify(key)}:${canonical(item)}`).join(",") + "}";
  return JSON.stringify(value) ?? "undefined";
}

/** An authoritative receipt clears only the exact submitted answer, never unrelated unsent text. */
export function acknowledgeQuestion(drafts: QuestionDrafts | undefined, id: string, answer: unknown): QuestionDrafts | undefined {
  const draft = drafts?.[id];
  if (!draft || draft.submitted === undefined || canonical(draft.submitted) !== canonical(answer)) return drafts;
  return Object.fromEntries(Object.entries(drafts ?? {}).filter(([key]) => key !== id));
}

/** Live resolution removes waiting separately from acknowledgement of the person's exact answer. */
export function resolvedQuestion(current: import("./store").Lane, patch: Partial<import("./store").Lane>, resolved: { id: string; answered?: boolean; answer?: unknown }): Partial<import("./store").Lane> {
  return {
    pending: (patch.pending ?? current.pending).filter((p) => p.id !== resolved.id),
    ...(resolved.answered ? { questionDrafts: acknowledgeQuestion(patch.questionDrafts ?? current.questionDrafts, resolved.id, resolved.answer) } : {}),
  };
}
