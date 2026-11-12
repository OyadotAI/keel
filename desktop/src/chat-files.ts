// Checkout identities keep file references distinct from uploaded files and other lanes' files.
import type { Lane } from "./store";

/** A relative file in one checkout; the daemon validates it again before provider submission. */
export interface FileReference { project: string; wt?: string; path: string }

/** Capture only checkout identity, so a later lane switch cannot retarget the dragged file. */
export function fileReference(lane: { project: string; wt?: string }, path: string): FileReference {
  return { project: lane.project, wt: lane.wt, path };
}

/** Refuse foreign checkouts rather than showing an identically named but unrelated file. */
export function sameCheckout(a: { project: string; wt?: string }, b: { project: string; wt?: string }): boolean {
  return a.project === b.project && (a.wt ?? "") === (b.wt ?? "");
}

/** Compare reference identity without changing spaces, Unicode, or the path's spelling. */
export function sameReference(a: FileReference, b: FileReference): boolean {
  return sameCheckout(a, b) && a.path === b.path;
}

/** An accepted message clears only the draft fields it actually owned before the async send. */
export function composerAfterSend(current: Lane, original: Lane, owned: boolean, sent: Pick<Lane, "attachments" | "references">): Partial<Lane> {
  if (!owned) return {};
  return {
    draft: current.draft === original.draft ? "" : current.draft,
    attachments: current.attachments === original.attachments && matchingAttachments(original.attachments, sent.attachments) ? [] : current.attachments,
    references: current.references === original.references && matchingReferences(original.references, sent.references) ? [] : current.references,
  };
}

function matchingAttachments(a: Lane["attachments"] = [], b: Lane["attachments"] = []): boolean {
  return a.length === b.length && a.every((item, index) => item.path === b[index].path && item.name === b[index].name && item.mime === b[index].mime);
}

function matchingReferences(a: FileReference[] = [], b: FileReference[] = []): boolean {
  return a.length === b.length && a.every((item, index) => sameReference(item, b[index]));
}

type Unsent = { text: string; attachments?: Lane["attachments"]; references?: FileReference[] };

/** Messages that will not be sent go back into the composer, after whatever is being typed, the
 *  way the CLI hands queued input back on Esc. Stop used to empty the queue and a discarded
 *  submission had nowhere to go, so either way what was typed was simply gone. */
export function backToComposer(lane: Lane, unsent: Unsent[]): Partial<Lane> {
  if (!unsent.length) return {};
  const texts = [lane.draft ?? "", ...unsent.map((u) => u.text)].filter((t) => t.trim());
  return {
    draft: texts.join("\n\n"),
    attachments: [...(lane.attachments ?? []), ...unsent.flatMap((u) => u.attachments ?? [])],
    references: [...(lane.references ?? []), ...unsent.flatMap((u) => u.references ?? [])],
  };
}

/** The daemon accepted receipt `id`. A live receipt is cleared; a discarded one is cleared too, and its prompt leaves the composer only if the person has not edited it since Discard put it back. */
export function acceptedReceipt(current: Lane, id: string): Partial<Lane> {
  if (current.receipt?.id === id) return { receipt: undefined };
  if (current.discarded?.id !== id) return {};
  return { discarded: undefined, ...(current.draft === current.discarded.prompt ? { draft: "" } : {}) };
}

/** A new submission must not overwrite an external owner's turn or an unconfirmed receipt. */
export function submissionBlock(lane: Lane, prompt: string): string | undefined {
  if (lane.elsewhere) return "This session is running outside Keel. Stop it there before resuming here.";
  if (lane.receipt && lane.receipt.prompt !== prompt) return "The previous submission has not been confirmed. Retry it before sending another message.";
}
