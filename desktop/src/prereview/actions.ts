// Review actions share one synchronous pending owner across tab, dock, reconnects, and retries.
import { HttpError, post, type Endpoint } from "../api";
import type { Operation, Review, Scope, Snapshot } from "./model";
import { bindReview, currentReview, invalidateReviewReads, patchReview, refreshReview, reviewOf, updateReviewForm } from "./state";
import { beginOpening, clearOpening, prRun, type OpeningInput } from "./opening";
import { STOP_WAIT, STOP_POLL, MIN_REQUEST_WAIT } from "./constants";

const MESSAGES: Record<Operation, string> = { start: "Review start wasn’t confirmed.", cancel: "Couldn’t cancel the review.", dismiss: "Couldn’t dismiss the review.", fix: "Couldn’t send findings.", open: "Couldn’t open the pull request." };
const entry = (s: Scope) => currentReview(s.lane, s.ep, s.wt);
const refresh = (s: Scope, timeout?: number) => refreshReview(s.lane, s.ep, s.wt, true, timeout);

async function action(scope: Scope, operation: Operation, work: (snapshot: Snapshot) => Promise<boolean>): Promise<boolean> {
  const snapshot = entry(scope) ?? (!reviewOf(scope.lane) ? bindReview(scope.lane, scope.ep, scope.wt) : undefined);
  if (!snapshot || snapshot.operation || prRun(scope.ep, scope.wt)?.running) return false;
  invalidateReviewReads(scope.lane);
  patchReview(scope.lane, snapshot, { operation, error: undefined });
  try { return await work(snapshot); }
  catch (error) {
    patchReview(scope.lane, snapshot, { error: { action: operation, message: MESSAGES[operation], details: error instanceof Error ? error.message : String(error) } });
    return false;
  } finally { patchReview(scope.lane, snapshot, { operation: undefined }); }
}

/** Re-running uses the person's current form, not an earlier review's frozen opening metadata. */
export function startReview(lane: string, ep: Endpoint, wt?: string, form?: { title?: string; draft?: boolean }): Promise<boolean> {
  const scope = { lane, ep, wt };
  if (form) updateReviewForm(lane, ep, wt, form);
  return action(scope, "start", async (before) => {
    if (!before.confirmed && !await refresh(scope)) return false;
    const source = entry(scope);
    if (!source || (source.review && !source.review.outcome)) return false;
    patchReview(lane, before, { confirmed: false });
    try {
      const result = await post<{ started: boolean }>(ep, "/api/prereview/start", { lane, title: source.form.title, draft: source.form.draft, body: source.review?.opening.body ?? "" }, { wt });
      if (!result?.started) throw new Error("The server did not accept the review request.");
    } catch (error) {
      const read = await refresh(scope), now = entry(scope);
      if (read && now?.review && (now.review.id !== before.review?.id || (error instanceof HttpError && error.status === 409 && !now.review.outcome))) { clearOpening(ep, wt); return true; }
      throw error;
    }
    clearOpening(ep, wt);
    return refresh(scope);
  });
}

async function settled(scope: Scope, review: Review | null): Promise<boolean> {
  if (!review || review.outcome) return true;
  const until = Date.now() + STOP_WAIT;
  const stopped = await post<{ id?: string; finished: boolean; missing?: boolean; cancel_requested: boolean }>(scope.ep, "/api/prereview/stop", { lane: scope.lane, id: review.id }, { wt: scope.wt }, STOP_WAIT);
  if (stopped.id !== review.id) throw new Error("The review changed. Refresh its status before continuing.");
  for (;;) {
    if (Date.now() >= until) throw new Error("The review is still stopping. Wait for it to finish, then retry.");
    if (!await refresh(scope, Math.max(MIN_REQUEST_WAIT, until - Date.now()))) return false;
    const now = entry(scope);
    if (!now) return false;
    if (!now.review) return true;
    if (now.review.id !== review.id) throw new Error("A different review started. Its results remain open.");
    if (now.review.outcome) return true;
    await new Promise((resolve) => setTimeout(resolve, STOP_POLL));
  }
}

/** A stop remains pending until a read confirms that exact worker's terminal outcome. */
export function stopReview(lane: string, ep: Endpoint, wt?: string): Promise<boolean> {
  const scope = { lane, ep, wt };
  return action(scope, "cancel", async (snapshot) => settled(scope, snapshot.review));
}

/** A failed stop/dismiss leaves the review, findings, and form visible for recovery. */
export function dismissReview(lane: string, ep: Endpoint, wt?: string): Promise<boolean> {
  const scope = { lane, ep, wt };
  return action(scope, "dismiss", async (snapshot) => {
    const publication = prRun(ep, wt);
    if (!await settled(scope, snapshot.review)) return false;
    const current = entry(scope);
    if (!current) return false;
    const result = await post<{ dismissed: boolean }>(ep, "/api/prereview/dismiss", { lane, id: current.review?.id ?? snapshot.review?.id }, { wt });
    if (!result?.dismissed) throw new Error("The server did not confirm dismissal.");
    if (!await refresh(scope)) return false;
    const after = entry(scope);
    if (after?.review) throw new Error("A review is still present. Its results remain open.");
    if (after) patchReview(lane, after, { restarted: false, delivery: undefined });
    if (publication && prRun(ep, wt) === publication && !publication.running) clearOpening(ep, wt);
    return true;
  });
}

/** Only accepted delivery changes the finding state; queued work is labelled separately. */
export function sendFindings(lane: string, ep: Endpoint, wt: string | undefined, review: Review, deliver: () => Promise<"sent" | "queued" | false>): Promise<boolean> {
  const scope = { lane, ep, wt };
  return action(scope, "fix", async (snapshot) => {
    if (!snapshot.confirmed || snapshot.review?.id !== review.id || (snapshot.head && snapshot.head !== review.sha)) throw new Error("Refresh or review this branch again before sending findings.");
    const result = await deliver();
    if (!result) throw new Error("The agent did not accept the findings. Reconnect its terminal or chat, then retry sending.");
    patchReview(lane, snapshot, { delivery: { id: review.id, state: result } });
    return true;
  });
}

/** The backend also enforces stop/reconciliation, so legacy callers cannot bypass those rules. */
export function openPullRequest(ep: Endpoint, wt: string | undefined, opening: OpeningInput, lane?: string): Promise<boolean> {
  if (!lane) return Promise.resolve(beginOpening(ep, wt, opening));
  const scope = { lane, ep, wt };
  return action(scope, "open", async () => {
    if (!await refresh(scope)) return false;
    const current = entry(scope);
    if (!current || !await settled(scope, current.review)) return false;
    const ready = entry(scope);
    if (!ready) return false;
    return beginOpening(ep, wt, { ...opening, body: opening.body ?? ready.review?.opening.body, title: ready.form.title, draft: ready.form.draft, head: opening.head ?? (ready.head || undefined), branch: opening.branch ?? (ready.target?.branch || undefined), base: opening.base ?? ready.target?.base });
  });
}
