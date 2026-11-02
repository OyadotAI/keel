// PR form ownership outlives tab remounts and stays visible through every review transition.
import type { Endpoint } from "../api";
import { useStore } from "../store";
import { openPullRequest, reviewForm, startReview, updateReviewForm, usePrRun, useReview } from "../prereview";
import { formPresentation } from "../prereview";
import { PreReview } from "./PreReview";

/** Review is advisory and separate from opening; both actions preserve the person's current form. */
export function PullRequest({ ep, wt, lane }: { ep: Endpoint; wt?: string; lane: string }) {
  const snapshot = useReview(lane, ep, wt), current = useStore((s) => s.lanes[lane]?.git?.branch);
  const opened = usePrRun(ep, wt), { run, review, pending, branch, base, hasResult } = formPresentation(snapshot, opened, current);
  const form = snapshot?.form ?? reviewForm(lane, wt);
  return <section className="pr-workflow" aria-label="Pull request">
    <div className="pr-target"><strong>{branch} → {base}</strong>{review?.sha && <span className="small muted">Reviewed commit {review.sha.slice(0, 7)}</span>}</div>
    <label className="small">Pull request title<input className="field" value={form.title} onChange={(e) => updateReviewForm(lane, ep, wt, { title: e.target.value })} placeholder="From commits when left empty" aria-label="Pull request title" /></label>
    <label className="small pr-draft"><input type="checkbox" checked={form.draft} onChange={(e) => updateReviewForm(lane, ep, wt, { draft: e.target.checked })} /> Open as a draft</label>
    <p className="small muted">Reviews run locally using Claude Code. Claude Code must be installed and signed in, including for Codex and Antigravity lanes.</p>
    <p className="small muted">Reviewing does not open a pull request. You choose when to open it using GitHub CLI (gh).</p>
    <PreReview lane={lane} ep={ep} wt={wt} place="tab" />
    {!hasResult && <div className="actions"><button className="primary" disabled={pending || !snapshot?.confirmed} onClick={() => void startReview(lane, ep, wt)}>{snapshot?.operation === "start" ? "Starting review…" : "Review branch"}</button><button disabled={pending || !snapshot?.confirmed} onClick={() => void openPullRequest(ep, wt, form, lane)}>Open without review</button></div>}
    {snapshot?.loading && !hasResult && !snapshot.operation && <p className="small muted" role="status">Reading review status…</p>}
    {!!run?.state.lines.length && <details><summary className="small">Technical details</summary><pre className="code">{run.state.lines.join("\n")}</pre></details>}
  </section>;
}
