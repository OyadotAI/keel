// Tab and dock show the same pending operation, failure, and explicit recovery action.
import { dismissReview, openPullRequest, refreshReview, sendFindings, stopReview, usePrRun, type Review, type Scope, type Snapshot } from "../prereview";
import { useStore } from "../store";
import { fixPrompt } from "../prereview";
import { terminalLease, typeInto } from "../typers";

const LABELS = { start: "Starting review…", cancel: "Cancelling review…", dismiss: "Closing review…", fix: "Sending findings…", open: "Preparing pull request…" };

/** A known picker or a disconnected terminal cannot falsely accept a submitted fix request. */
export async function deliverFindings(lane: string, review: Review): Promise<"sent" | "queued" | false> {
  const current = useStore.getState().lanes[lane];
  if (!current) return false;
  if (current.elsewhere) throw new Error("Open this session here before sending findings.");
  if (current.pending.length || current.attention) throw new Error("Answer the current question or approval before sending findings.");
  const prompt = fixPrompt(review, review.outcome?.kind === "verdict" ? review.outcome.findings : []);
  if (current.interface === "terminal") {
    const lease = terminalLease(lane);
    return lease && typeInto(lane, prompt, lease) ? "sent" : false;
  }
  const queued = current.running || !!current.submitting;
  return await useStore.getState().send(lane, prompt) ? queued ? "queued" : "sent" : false;
}

/** Recovery is explicit; an uncertain start first checks status instead of creating another run. */
export function PreReviewFeedback({ scope, snapshot }: { scope: Scope; snapshot?: Snapshot }) {
  const publication = usePrRun(scope.ep, scope.wt);
  if (!snapshot) return null;
  const { operation, error } = snapshot;
  if (operation) return <p className="small muted" role="status">{LABELS[operation]}</p>;
  if (!error) return null;
  const retry = () => {
    const { lane, ep, wt } = scope;
    if (error.action === "fix" && snapshot.review) return sendFindings(lane, ep, wt, snapshot.review, () => deliverFindings(lane, snapshot.review!));
    if (error.action === "cancel") return stopReview(lane, ep, wt);
    if (error.action === "dismiss") return dismissReview(lane, ep, wt);
    if (error.action === "open") return openPullRequest(ep, wt, { ...snapshot.form, ...publication?.state.target }, lane);
    return refreshReview(lane, ep, wt, true);
  };
  const label = error.action === "fix" ? "Retry sending" : error.action === "cancel" ? "Retry cancellation" : error.action === "dismiss" ? "Retry closing" : error.action === "open" ? "Retry opening" : "Retry connection";
  return <div className="review-error" role="alert"><strong>{error.message}</strong>
    <button onClick={() => void retry()}>{label}</button>
    <details><summary className="small">Technical details</summary><pre className="code">{error.details}</pre></details>
  </div>;
}

/** Structured causes provide useful next steps without guessing from arbitrary provider output. */
export function reviewFailure(cause: string, line: string): string {
  const explanations: Record<string, string> = {
    auth: "Claude Code isn’t signed in. Open Claude Code in a terminal, sign in, then review again.",
    base: "The review base is unavailable. Check the remote and connection, then review again.",
    ceiling: "The review reached its time limit. No verdict was produced; review again when ready.",
    wait: "The review could not start while this checkout was changing. Finish or stop the current turn, then review again.",
  };
  if (cause === "start" && /not found|no such file|os error 2|ENOENT/i.test(line)) return "Claude Code could not be started. Install it, open it in a terminal, and sign in before reviewing again.";
  return explanations[cause] ?? line;
}
