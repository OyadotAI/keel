// The team's review before a pull request opens: in Git › Pull request, and docked above the
// composer so a review the agent's own `gh pr create` started is seen from the conversation. It
// advises; Open is reachable from every state.
import { useEffect, useState } from "react";
import type { Endpoint } from "../api";
import { useIde } from "../ide";
import { useRun } from "../runs";
import { useStore } from "../store";
import { typeInto } from "../typers";
import { baseOf, clock, dismissReview, fixPrompt, navigable, openPullRequest, prKey, sorted, startReview, stopReview, useReview, type Finding, type PrRun, type Review } from "../prereview";
import { ApprovalStrip } from "./Approvals";
import { Banner } from "./kit";
import { Icon } from "./icons";
import { Markdown } from "./Markdown";

const MAX_ROWS = 50;
const short = (sha: string) => sha.slice(0, 7);
const branchOf = (r: Review) => (r.wt ? `keel/${r.wt}` : "this branch");

function useNow(on: boolean) {
  const [now, setNow] = useState(Date.now());
  useEffect(() => {
    if (!on) return;
    const t = setInterval(() => setNow(Date.now()), 1000);
    return () => clearInterval(t);
  }, [on]);
  return now;
}

function count(findings: Finding[]) {
  const n = (s: string) => findings.filter((f) => f.severity === s).length;
  const part = (k: number, one: string, many: string) => (k ? `${k} ${k === 1 ? one : many}` : "");
  return [part(n("blocking"), "blocking", "blocking"), part(n("warning"), "warning", "warnings"), part(n("note"), "note", "notes")].filter(Boolean).join(" · ");
}

/// Why there is no verdict, in the person's words; the daemon's own line for anything else.
function noVerdict(cause: string, line: string, base: string): string {
  switch (cause) {
    case "auth": return "Couldn't review: Claude Code isn't logged in on this machine. Run `claude` and log in, then review again.";
    case "base": return `Couldn't fetch \`${base}\` to compare against: ${line}`;
    case "ceiling": return `${line} Nothing was found or cleared.`;
    case "refused": return line;
    default: return `No verdict: ${line}`;
  }
}

function Findings({ lane, findings }: { lane: string; findings: Finding[] }) {
  const rows = sorted(findings);
  const word = (s: string) => (s === "blocking" ? "Blocking" : s === "warning" ? "Warning" : "Note");
  return (
    <div className="review-findings" role="list">
      {rows.slice(0, MAX_ROWS).map((f, i) => (
        <div className="review-finding" role="listitem" key={i}>
          <div>
            <span className={`sev ${f.severity}`}>{word(f.severity)}</span> {f.title}
          </div>
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

interface Props { lane: string; ep: Endpoint; wt?: string; place: "tab" | "dock" }

/// The two buttons every ending offers, and Dismiss in the dock.
function Open({ lane, ep, wt, review, label, primary, again, dismiss }: Props & { review?: Review | null; label: string; primary?: boolean; again?: () => void; dismiss?: boolean }) {
  const pr = useRun<PrRun>(prKey(ep, wt));
  const opening = !!pr?.running;
  const open = () => {
    void stopReview(lane, ep, wt);
    const o = review?.opening;
    openPullRequest(ep, wt, { title: o?.title ?? "", body: o?.body, draft: !!o?.draft, base: review?.base ? baseOf(review) : undefined });
  };
  return <>
    <button className={primary ? "primary" : undefined} disabled={opening} onClick={open}>{opening ? "Opening…" : label}</button>
    {again && <button onClick={again}>Review again</button>}
    {dismiss && <button className="ghost" onClick={() => void dismissReview(lane, ep, wt)}>Dismiss</button>}
  </>;
}

/// What Open did, wherever it was pressed: the link, or what stopped it — a dock that went quiet
/// after "Opening…" left the person unable to tell whether a pull request exists.
function Opened({ ep, wt }: { ep: Endpoint; wt?: string }) {
  const pr = useRun<PrRun>(prKey(ep, wt));
  const lines = pr?.state.lines ?? [];
  const link = lines.map((l) => l.match(/https:\/\/github\.com\/\S+\/pull\/\d+/)?.[0]).find(Boolean);
  if (link) return <p className="small">Opened <a href={link}>{link.replace(/^https:\/\/github\.com\//, "")}</a></p>;
  if (pr?.running) return <p className="small faint">{lines.at(-1) ?? "Opening…"}</p>;
  if (pr?.state.result === "failed") return <Banner tone="error">The pull request did not open: {lines.at(-1) ?? "gh exited with an error"}</Banner>;
  return null;
}

function Running({ r, head, ...p }: Props & { r: Review; head: string }) {
  const now = useNow(true);
  const elapsed = clock(Math.max(0, Math.floor(now / 1000) - r.started));
  const doing =
    r.phase === "waiting" ? "Starts when this turn ends." :
    r.phase === "checking" ? "Checking the branch…" :
    r.phase === "fetching" ? `Fetching \`${r.base}\`…` :
    `${r.by_agent ? "The agent ran `gh pr create`. " : ""}Keel's team is reviewing \`${branchOf(r)}\` at \`${short(r.sha)}\` against \`${r.base}\`${r.by_agent ? " first" : ""}.`;
  return <>
    <div className="review-head"><span className="eyebrow">Keel review</span><strong>Reviewing before the pull request opens</strong><span className="small faint num">{elapsed}</span></div>
    <div className="small"><Markdown text={doing} /></div>
    {head && r.sha && head !== r.sha && <Banner tone="warn">The branch moved to <code>{short(head)}</code> while this ran. The result will be for <code>{short(r.sha)}</code>.</Banner>}
    <div className="actions"><button onClick={() => void stopReview(p.lane, p.ep, p.wt)}>Cancel review</button><Open {...p} review={r} label="Open without review" /></div>
  </>;
}

type Verdicted = Extract<NonNullable<Review["outcome"]>, { kind: "verdict" }>;

/// The verdict word, where it came from, and why: the CI comment's header, in the same words.
function Verdict({ r, o }: { r: Review; o: Verdicted }) {
  const ship = o.verdict === "ship";
  return <>
    <div className="review-head" role="status">
      <Icon name={ship ? "check-circle" : "hand"} size={14} style={{ color: ship ? "var(--add)" : "var(--warn)" }} />
      <strong>{ship ? "Ship" : "Do not ship"}</strong>
      <span className="small faint num">{short(r.sha)} vs {r.base} · {clock(o.took)}{o.cost ? ` · $${o.cost.toFixed(2)}` : ""}</span>
    </div>
    {!ship && o.reason && <div className="small">{o.reason}</div>}
    {o.summary && <div className="small muted">{o.summary}</div>}
  </>;
}

function Finished({ r, head, o, ...p }: Props & { r: Review; head: string; o: Verdicted }) {
  const [open, setOpen] = useState(p.place === "tab");
  const [sent, setSent] = useState(false);
  const moved = !!head && !!r.sha && head !== r.sha;
  const needsWork = o.findings.some((f) => f.severity === "blocking" || f.severity === "warning");
  const dock = p.place === "dock";
  const fix = () => {
    const l = useStore.getState().lanes[p.lane];
    if (!l) return;
    const prompt = fixPrompt(r, o.findings);
    if (l.interface === "terminal") typeInto(p.lane, prompt);
    else void useStore.getState().send(p.lane, prompt);
    setSent(true);
  };
  const again = sent || moved ? () => { setSent(false); void startReview(p.lane, p.ep, p.wt, r.opening); } : undefined;
  return <>
    <Verdict r={r} o={o} />
    {moved && <Banner tone="warn">Reviewed <code>{short(r.sha)}</code>. The branch is at <code>{short(head)}</code> now.</Banner>}
    <div className="small">{o.findings.length ? count(o.findings) : "No findings."}</div>
    {sent && <div className="small">Sent {o.findings.length} findings to the agent. Review again when it's done.</div>}
    <div className="actions">
      {o.findings.length > 0 && !sent && <button className={needsWork ? "primary" : undefined} onClick={fix}>Fix these</button>}
      <Open {...p} review={r} label="Open pull request" primary={!needsWork || sent} again={again} />
      {dock && o.findings.length > 0 && <button className="ghost" aria-expanded={open} onClick={() => setOpen(!open)}>{open ? "Hide findings" : `Show findings (${o.findings.length})`}</button>}
      <button className="ghost" onClick={() => void dismissReview(p.lane, p.ep, p.wt)}>Dismiss</button>
    </div>
    {open && o.findings.length > 0 && <Findings lane={p.lane} findings={o.findings} />}
    {!dock && o.report && <details><summary className="small">Full report</summary><Markdown text={o.report} /></details>}
  </>;
}

/// The review in `place`: the Pull request tab shows findings open, the dock folds them.
export function PreReview(p: Props) {
  const seen = useReview(p.lane);
  const pr = useRun<PrRun>(prKey(p.ep, p.wt));
  const r = seen?.review;
  if (!seen || (!r && !seen.restarted)) return null;
  const o = r?.outcome;
  // "Open without review" cancels the review; the dock stays for as long as the open has
  // something to say, or the push and its result would vanish with the review.
  if (p.place === "dock" && o?.kind === "none" && o.cause === "cancelled" && !pr) return null;
  const dock = p.place === "dock";
  const again = () => void startReview(p.lane, p.ep, p.wt, r?.opening);
  const body =
    !r ? <>
      <Banner tone="error">The review ended when Keel restarted. Nothing was opened.</Banner>
      <div className="actions"><Open {...p} label="Open pull request" primary again={again} dismiss /></div>
    </> :
    !o ? <Running {...p} r={r} head={seen.head} /> :
    o.kind === "none" ? <>
      {o.cause === "cancelled" ? <Banner tone="note">Review cancelled after {clock(o.took)}.</Banner> : <Banner tone="error">{noVerdict(o.cause, o.line, r.base)}</Banner>}
      <div className="actions"><Open {...p} review={r} label="Open pull request" primary again={again} dismiss /></div>
    </> :
    <Finished {...p} r={r} head={seen.head} o={o} />;
  return (
    <div className={dock ? "strip review-strip" : "review-panel"} role="region" aria-label="Pull request review">
      <div className={dock ? "review-row" : undefined}>{body}<Opened ep={p.ep} wt={p.wt} /></div>
    </div>
  );
}

/// What is docked above the lane's composer: its questions first, then the review, which is not
/// waiting for an answer and so never takes focus.
export function LaneDock({ lane }: { lane: string }) {
  const ep = useStore((s) => { const l = s.lanes[lane]; return l ? s.projects[l.project]?.endpoint : undefined; });
  const wt = useStore((s) => s.lanes[lane]?.wt);
  return <>
    <ApprovalStrip lane={lane} />
    {ep && <PreReview lane={lane} ep={ep} wt={wt} place="dock" />}
  </>;
}
