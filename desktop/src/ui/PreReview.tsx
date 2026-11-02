// A full PR review tab and compact dock share confirmed facts, pending actions, and recovery.
import { useEffect, useState } from "react";
import { openUrl } from "@tauri-apps/plugin-opener";
import type { Endpoint } from "../api";
import { useIde } from "../ide";
import { useStore } from "../store";
import { baseOf, clock, dismissReview, openPullRequest, prLabel, prLink, sorted, navigable, refreshReview, startReview, stopReview, sendFindings, useReview, usePrRun, prForTarget, type Review, type Finding, type PrRun } from "../prereview";
import { ApprovalStrip } from "./Approvals";
import { Banner, focusTerminal } from "./kit";
import { Icon } from "./icons";
import { Markdown } from "./Markdown";
import { deliverFindings, PreReviewFeedback, reviewFailure } from "./PreReviewFeedback";
import { verdictPresentation } from "../prereview";

const MAX_ROWS = 50;
const short = (sha: string) => sha.slice(0, 7);
type Verdicted = Extract<NonNullable<Review["outcome"]>, { kind: "verdict" }>;
interface Props { lane: string; ep: Endpoint; wt?: string; place: "tab" | "dock" }
const after = () => requestAnimationFrame(() => document.activeElement === document.body && focusTerminal());
function useNow() {
  const [now, setNow] = useState(Date.now());
  useEffect(() => { const timer = setInterval(() => setNow(Date.now()), 1000); return () => clearInterval(timer); }, []);
  return now;
}
function count(findings: Finding[]) {
  return ["blocking", "warning", "note"].map((severity) => {
    const n = findings.filter((f) => f.severity === severity).length;
    return n ? `${n} ${severity}${n !== 1 && severity !== "blocking" ? "s" : ""}` : "";
  }).filter(Boolean).join(" · ") || "No findings.";
}
function useControls(p: Props, review?: Review | null) {
  const snapshot = useReview(p.lane, p.ep, p.wt), pr = usePrRun(p.ep, p.wt);
  const busy = !!snapshot?.operation || !!pr?.running;
  const open = () => void openPullRequest(p.ep, p.wt, { title: snapshot?.form.title ?? "", draft: !!snapshot?.form.draft, base: review ? baseOf(review) : undefined }, p.lane);
  const again = () => void startReview(p.lane, p.ep, p.wt);
  return { snapshot, busy, open, again };
}
function Head({ icon, color, eyebrow, title, meta }: { icon?: "check-circle" | "hand" | "x-circle" | "minus-circle"; color?: string; eyebrow?: string; title: string; meta?: string }) {
  return (
    <div className="review-head">
      {icon && <Icon name={icon} size={14} style={{ color }} />}
      {eyebrow && <span className="eyebrow">{eyebrow}</span>}
      <strong aria-live="polite">{title}</strong>
      {meta && <span className="small faint num">{meta}</span>}
    </div>
  );
}

function Findings({ lane, findings }: { lane: string; findings: Finding[] }) {
  const rows = sorted(findings);
  const word = (s: string) => (s === "blocking" ? "Blocking" : s === "warning" ? "Warning" : "Note");
  return (
    <div className="review-findings" role="list">
      {rows.slice(0, MAX_ROWS).map((f, i) => (
        <div className="review-finding" role="listitem" key={i}>
          <div><span className={`sev ${f.severity}`}>{word(f.severity)}</span> {f.title}</div>
          {navigable(f.path) ? (
            <button className="link where" data-file={f.path} onClick={() => useIde.getState().navigate(lane, f.path, f.line || undefined)}>
              {f.path}{f.line ? `:${f.line}` : ""}
            </button>
          ) : (
            <span className="where">{f.path}{f.line ? `:${f.line}` : ""}</span>
          )}
          {f.body && (
            <details>
              <summary className="small faint">Details</summary>
              <div className="small">{f.body}</div>
            </details>
          )}
        </div>
      ))}
      {rows.length > MAX_ROWS && <div className="small faint">+{rows.length - MAX_ROWS} more in the full report</div>}
    </div>
  );
}

/** Opening a known PR is an explicit user action; it never creates another one. */
export function PrLink({ link }: { link: string }) {
  const label = prLabel(link);
  return (
    <button className="link" title={link} aria-label={`Open pull request ${label} on GitHub`} onClick={() => void openUrl(link)}>
      {label} <Icon name="external-link" size={12} />
    </button>
  );
}


/** Closing waits for the selected worker to finish and for the daemon to confirm dismissal. */
function Close(p: Props & { running?: boolean }) {
  const snapshot = useReview(p.lane, p.ep, p.wt);
  const publication = usePrRun(p.ep, p.wt);
  const close = async () => { if (await dismissReview(p.lane, p.ep, p.wt)) after(); };
  const label = p.running ? "Cancel review and close" : "Dismiss review";
  return <button className="ghost review-close" aria-label={label} title={label} disabled={!!snapshot?.operation || !!publication?.running} onClick={() => void close()}><Icon name="x" size={14} /></button>;
}

function useFold(place: Props["place"]) {
  const [open, setOpen] = useState(place === "tab");
  const toggle = (n: number) => n > 0 && place === "dock" && <button className="link" aria-expanded={open} onClick={() => setOpen(!open)}>{open ? "Hide findings" : `Show findings (${n})`}</button>;
  return { open, toggle };
}

/** Phase text describes the work without requiring the person to decode command output. */
export function Running({ r, head, ...p }: Props & { r: Review; head: string }) {
  const now = useNow(), { busy, open } = useControls(p, r);
  const elapsed = clock(Math.max(0, Math.floor(now / 1000) - r.started));
  const phase = r.cancel_requested ? "Cancelling review…" : r.phase === "waiting" ? "Waiting for the current turn to finish." : r.phase === "checking" ? "Checking branch…" : r.phase === "fetching" ? `Fetching ${r.base}…` : `Reviewing ${short(r.sha)} against ${r.base}…`;
  return <><Head eyebrow="Keel review · Claude Code" title={phase} meta={elapsed} />
    {head && r.sha && head !== r.sha && <Banner tone="warn">The branch moved to <code>{short(head)}</code>. This review is for <code>{short(r.sha)}</code>.</Banner>}
    <div className="actions"><button disabled={busy} onClick={() => void stopReview(p.lane, p.ep, p.wt)}>Cancel review</button><button disabled={busy} onClick={open}>Open without review</button></div>
    <p className="small muted">Opening without review waits for cancellation to finish before opening.</p><Close {...p} running /></>;
}

/** A false or failed delivery leaves findings actionable rather than claiming they were sent. */
export function Finished({ r, head, o, ...p }: Props & { r: Review; head: string; o: Verdicted }) {
  const { snapshot, busy, open, again } = useControls(p, r), { open: shown, toggle } = useFold(p.place);
  const view = verdictPresentation(r, o, head, snapshot);
  const fix = () => void sendFindings(p.lane, p.ep, p.wt, r, () => deliverFindings(p.lane, r));
  return <><Head icon={view.icon} color={view.color} title={view.title} meta={view.meta} />
    {o.reason && <p className="review-body">{o.reason}</p>}{o.summary && <p className="review-body muted">{o.summary}</p>}
    {view.moved && <Banner tone="warn">Results are for <code>{short(r.sha)}</code>; this branch is now at <code>{short(head)}</code>.</Banner>}
    <p className="review-body">{view.delivery ?? count(o.findings)}</p>
    <div className="actions">
      {view.stale && <button className="primary" disabled={busy || view.unconfirmed} onClick={again}>Review again</button>}
      {!view.stale && !!o.findings.length && <button className={!view.ship ? "primary" : undefined} disabled={busy || !snapshot?.confirmed} onClick={fix}>Fix these</button>}
      <button className={!view.stale && view.ship ? "primary" : undefined} disabled={busy || view.unconfirmed} onClick={open}>{view.ship || view.stale ? "Open pull request" : "Open anyway"}</button>{toggle(o.findings.length)}
    </div>
    {shown && !!o.findings.length && <Findings lane={p.lane} findings={o.findings} />}
    {p.place === "tab" && o.report && <details><summary className="small">Full report</summary><Markdown text={o.report} /></details>}<Close {...p} /></>;
}

/** Cancellation, missing state, and a reviewer failure never become a shipping verdict. */
export function Ended({ r, ...p }: Props & { r?: Review | null }) {
  const { busy, open, again } = useControls(p, r);
  const o = r?.outcome?.kind === "none" ? r.outcome : undefined, cancelled = o?.cause === "cancelled";
  const title = !r ? "Review interrupted" : cancelled ? "Review cancelled" : "Couldn’t review";
  const text = !r ? "The running review is no longer available. No verdict was confirmed." : cancelled ? "Nothing was opened." : reviewFailure(o?.cause ?? "", o?.line ?? "");
  return <><Head icon={cancelled ? "minus-circle" : "x-circle"} color={cancelled ? "var(--faint)" : "var(--del)"} title={title} meta={o ? `after ${clock(o.took)}` : undefined} />
    <div className="review-body"><Markdown text={text} /></div>
    <div className="actions"><button className="primary" disabled={busy} onClick={again}>Review again</button>{o?.cause !== "refused" && <button disabled={busy} onClick={open}>Open without review</button>}</div>
    {o?.line && <details><summary className="small">Technical details</summary><pre className="code">{o.line}</pre></details>}<Close {...p} /></>;
}

/** An existing/created link wins over a lost response; retry first reconciles the immutable target. */
export function PullRequestLine({ run, r, ...p }: Props & { run: { state: PrRun; running: boolean }; r?: Review | null }) {
  const { busy, snapshot, again } = useControls(p, r), link = prLink(run), o = r?.outcome;
  const retry = () => void openPullRequest(p.ep, p.wt, { title: snapshot?.form.title ?? "", draft: !!snapshot?.form.draft, ...run.state.target }, p.lane);
  if (link) return <><Head icon="check-circle" color="var(--add)" title={run.state.existing || run.state.result === "failed" ? "Pull request already open" : "Pull request opened"} meta={run.state.target?.branch} /><div className="review-pr"><PrLink link={link} /></div>{run.state.actualHead && run.state.target?.head && run.state.actualHead !== run.state.target.head && <p className="small muted">Pull request head {short(run.state.actualHead)} · Requested commit {short(run.state.target.head)}</p>}<Close {...p} /></>;
  if (run.running) return <><Head eyebrow="Pull request" title="Opening pull request…" /><p className="small muted" role="status">{run.state.lines.at(-1) ?? "Checking target and existing pull requests…"}</p><Close {...p} /></>;
  return <><Banner tone="error">Couldn’t open the pull request. {run.state.error ?? run.state.lines.at(-1) ?? "The server did not confirm completion."}</Banner>
    <div className="actions"><button className="primary" disabled={busy} onClick={retry}>Retry opening</button>{(!o || o.kind === "none") && <button disabled={busy} onClick={again}>Review again</button>}</div><Close {...p} /></>;
}

/** Passive status changes never move focus or discard the person's draft and useful results. */
export function PreReview(p: Props) {
  const snapshot = useReview(p.lane, p.ep, p.wt), opened = usePrRun(p.ep, p.wt);
  const revision = useStore((s) => s.lanes[p.lane]?.reviewRevision);
  const reviewId = snapshot?.review?.id;
  useEffect(() => { if (reviewId) void refreshReview(p.lane, p.ep, p.wt, true); }, [p.lane, p.ep, p.wt, revision, reviewId]);
  const run = prForTarget(opened, snapshot?.target?.branch), r = snapshot?.review, o = r?.outcome;
  if (!snapshot || (!r && !snapshot.restarted && !run && !snapshot.operation && !snapshot.error)) return null;
  const body = run ? <PullRequestLine {...p} run={run} r={r} /> : r && o?.kind === "verdict" ? <Finished {...p} r={r} head={snapshot.head} o={o} /> : r && !o ? <Running {...p} r={r} head={snapshot.head} /> : r || snapshot.restarted ? <Ended {...p} r={r} /> : null;
  const keys = (e: React.KeyboardEvent) => { if (e.key === "Escape") { e.stopPropagation(); focusTerminal(); } };
  return <div className={p.place === "dock" ? "strip review-strip" : "review-panel"} role="region" aria-label="Pull request review" onKeyDown={keys}>
    <div className="review-row"><PreReviewFeedback scope={p} snapshot={snapshot} />{body}</div>
  </div>;
}

/** Questions retain their priority above a review, which never claims to need an answer. */
export function LaneDock({ lane }: { lane: string }) {
  const ep = useStore((s) => { const l = s.lanes[lane]; return l ? s.projects[l.project]?.endpoint : undefined; });
  const wt = useStore((s) => s.lanes[lane]?.wt);
  return <><ApprovalStrip lane={lane} />{ep && <PreReview lane={lane} ep={ep} wt={wt} place="dock" />}</>;
}
