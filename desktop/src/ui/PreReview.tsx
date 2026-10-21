// The team's review before a pull request opens: in Git › Pull request, and docked above the
// composer so a review the agent's own `gh pr create` started is seen from the conversation. It
// advises; Open is reachable from every state until a pull request exists, and never after —
// a second click made a second pull request. Every state can be closed: the dock's × is an icon
// placed where nothing can squeeze it, because a text button in `.ghost` was clipped to "smi".
import { useEffect, useState, type ReactNode } from "react";
import { openUrl } from "@tauri-apps/plugin-opener";
import type { Endpoint } from "../api";
import { useIde } from "../ide";
import { clear, useRun } from "../runs";
import { useStore } from "../store";
import { typeInto } from "../typers";
import { baseOf, clock, dismissReview, fixPrompt, navigable, openPullRequest, prKey, prLabel, prLink, sorted, startReview, stopReview, useReview, type Finding, type PrRun, type Review } from "../prereview";
import { ApprovalStrip } from "./Approvals";
import { Banner, focusTerminal } from "./kit";
import { Icon } from "./icons";
import { Markdown } from "./Markdown";

const MAX_ROWS = 50;
const short = (sha: string) => sha.slice(0, 7);
const branchOf = (r: Review) => (r.wt ? `keel/${r.wt}` : "this branch");
/// A pull request run that has finished is put away when the review is: left in memory, its
/// "opened" or "failed" line stood over the next review. One still pushing is never touched
/// (`clear` refuses it).
const settle = (ep: Endpoint, wt: string | undefined) => clear(prKey(ep, wt));

/// A control that unmounts under focus hands it to the composer rather than to nothing.
const after = () => requestAnimationFrame(() => document.activeElement === document.body && focusTerminal());

type Verdicted = Extract<NonNullable<Review["outcome"]>, { kind: "verdict" }>;
interface Props { lane: string; ep: Endpoint; wt?: string; place: "tab" | "dock" }

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
  return [part(n("blocking"), "blocking", "blocking"), part(n("warning"), "warning", "warnings"), part(n("note"), "note", "notes")].filter(Boolean).join(" · ") || "No findings.";
}

/// Why there is no verdict, in the person's words; the daemon's own line for anything else.
function noVerdict(cause: string, line: string): string {
  switch (cause) {
    case "auth": return "Claude Code isn't logged in on this machine. Run `claude` in a terminal and log in, then review again.";
    case "base": return `${line} Check the remote and your connection, then review again.`;
    case "ceiling": return `${line} Nothing was found or cleared.`;
    default: return line;
  }
}

/// Icon, title, and the quiet facts beside it. The title is the one thing read aloud on change.
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

/// Opens the pull request with what the review knew: the person's title, the agent's words.
function useOpen({ lane, ep, wt }: Props, review?: Review | null) {
  return () => {
    void stopReview(lane, ep, wt);
    const o = review?.opening;
    openPullRequest(ep, wt, { title: o?.title ?? "", body: o?.body, draft: !!o?.draft, base: review?.base ? baseOf(review) : undefined });
  };
}

/// The pull request's address, opened in the person's browser rather than inside Keel's window.
export function PrLink({ link }: { link: string }) {
  const label = prLabel(link);
  return (
    <button className="link" title={link} aria-label={`Open pull request ${label} on GitHub`} onClick={() => void openUrl(link)}>
      {label} <Icon name="external-link" size={12} />
    </button>
  );
}

/// The × every state has. Running, it cancels first; opening, the push carries on regardless.
function Close({ lane, ep, wt, running }: Props & { running?: boolean }) {
  const close = () => {
    if (running) void stopReview(lane, ep, wt);
    settle(ep, wt);
    void dismissReview(lane, ep, wt);
    after();
  };
  return (
    <button className="ghost review-close" aria-label={running ? "Cancel review and close" : "Dismiss review"} title={running ? "Cancel review and close" : "Dismiss"} onClick={close}>
      <Icon name="x" size={14} />
    </button>
  );
}

/// The findings, folded in the dock and open in the tab, behind one toggle.
function useFold(place: Props["place"]) {
  const [open, setOpen] = useState(place === "tab");
  const toggle = (n: number) => n > 0 && place === "dock" && (
    <button className="link" aria-expanded={open} onClick={() => setOpen(!open)}>{open ? "Hide findings" : `Show findings (${n})`}</button>
  );
  return { open, toggle };
}

export function Running({ r, head, ...p }: Props & { r: Review; head: string }) {
  const now = useNow(true);
  const open = useOpen(p, r);
  const elapsed = clock(Math.max(0, Math.floor(now / 1000) - r.started));
  const title = r.phase === "waiting" ? "Review starts when this turn ends" : "Reviewing before the pull request opens";
  const doing =
    r.phase === "waiting" ? "" :
    r.phase === "checking" ? "Checking the branch…" :
    r.phase === "fetching" ? `Fetching \`${r.base}\`…` :
    `${r.by_agent ? "The agent ran `gh pr create`. " : ""}Keel's team is reviewing \`${branchOf(r)}\` at \`${short(r.sha)}\` against \`${r.base}\`${r.by_agent ? " first" : ""}.`;
  return <>
    <Head eyebrow="Keel review" title={title} meta={elapsed} />
    {doing && <div className="review-body"><Markdown text={doing} /></div>}
    {head && r.sha && head !== r.sha && <Banner tone="warn">The branch moved to <code>{short(head)}</code> while this ran. The result will be for <code>{short(r.sha)}</code>.</Banner>}
    <div className="actions">
      <button onClick={() => void stopReview(p.lane, p.ep, p.wt)}>Cancel review</button>
      <button onClick={open}>Open without review</button>
    </div>
    <Close {...p} running />
  </>;
}

export function Finished({ r, head, o, ...p }: Props & { r: Review; head: string; o: Verdicted }) {
  const [sent, setSent] = useState(false);
  const { open: shown, toggle } = useFold(p.place);
  const open = useOpen(p, r);
  const ship = o.verdict === "ship";
  const moved = !!head && !!r.sha && head !== r.sha;
  const again = () => { setSent(false); void startReview(p.lane, p.ep, p.wt, r.opening); };
  const fix = () => {
    const l = useStore.getState().lanes[p.lane];
    if (!l) return;
    const prompt = fixPrompt(r, o.findings);
    if (l.interface === "terminal") typeInto(p.lane, prompt);
    else void useStore.getState().send(p.lane, prompt);
    setSent(true);
  };
  const stale = sent || moved;
  return <>
    <Head icon={ship ? "check-circle" : "hand"} color={ship ? "var(--add)" : "var(--warn)"} title={ship ? "Ship" : "Do not ship"} meta={`${short(r.sha)} vs ${r.base} · ${clock(o.took)}${o.cost ? ` · $${o.cost.toFixed(2)}` : ""}`} />
    {!ship && o.reason && <div className="review-body">{o.reason}</div>}
    {o.summary && <div className="review-body"><span className="muted">{o.summary}</span></div>}
    {moved && <Banner tone="warn">Reviewed <code>{short(r.sha)}</code>. The branch is at <code>{short(head)}</code> now.</Banner>}
    <div className="review-body"><span className="muted">{sent ? `Sent ${o.findings.length} findings to the agent. Review again when it has finished.` : count(o.findings)}</span></div>
    <VerdictActions ship={ship} stale={stale} findings={o.findings.length} again={again} fix={fix} open={open} toggle={toggle(o.findings.length)} />
    {shown && o.findings.length > 0 && <Findings lane={p.lane} findings={o.findings} />}
    {p.place === "tab" && o.report && <details><summary className="small">Full report</summary><Markdown text={o.report} /></details>}
    <Close {...p} />
  </>;
}

/// After a verdict: the one thing to do next is the primary — review again once the branch has
/// moved or fixes went out, fix what blocks shipping, or open what shipped.
function VerdictActions({ ship, stale, findings, again, fix, open, toggle }: { ship: boolean; stale: boolean; findings: number; again: () => void; fix: () => void; open: () => void; toggle: ReactNode }) {
  const fixable = !stale && findings > 0;
  return (
    <div className="actions">
      {stale && <button className="primary" onClick={again}>Review again</button>}
      {fixable && !ship && <button className="primary" onClick={fix}>Fix these</button>}
      <button className={!stale && ship ? "primary" : undefined} onClick={open}>{ship || stale ? "Open pull request" : "Open anyway"}</button>
      {fixable && ship && <button onClick={fix}>Fix these</button>}
      {toggle}
    </div>
  );
}

/// No verdict: the review could not run, was cancelled, or Keel restarted under it.
export function Ended({ r, ...p }: Props & { r?: Review | null }) {
  const o = r?.outcome?.kind === "none" ? r.outcome : undefined;
  const open = useOpen(p, r);
  const again = () => void startReview(p.lane, p.ep, p.wt, r?.opening);
  const cancelled = o?.cause === "cancelled";
  const title = !r ? "Review interrupted" : cancelled ? "Review cancelled" : "Couldn't review";
  const body = !r ? "Keel restarted while this ran, so there's no verdict. Nothing was opened." : cancelled ? "Nothing was opened." : noVerdict(o?.cause ?? "", o?.line ?? "");
  return <>
    <Head icon={cancelled ? "minus-circle" : "x-circle"} color={cancelled ? "var(--faint)" : "var(--del)"} title={title} meta={o ? `after ${clock(o.took)}` : undefined} />
    <div className="review-body"><Markdown text={body} /></div>
    <div className="actions">
      <button className={!r || o?.cause === "base" || o?.cause === "refused" ? "primary" : undefined} onClick={again}>Review again</button>
      {/* Refused by the same check Open runs: it could only fail the same way. */}
      {o?.cause !== "refused" && <button onClick={open}>Open without review</button>}
    </div>
    <Close {...p} />
  </>;
}

/// The pull request, once Open has been pressed: on its way, opened, or stopped.
export function PullRequestLine({ run, r, ...p }: Props & { run: { state: PrRun; running: boolean }; r?: Review | null }) {
  const link = prLink(run);
  const open = useOpen(p, r);
  const o = r?.outcome;
  const { open: shown, toggle } = useFold(p.place);
  const findings = o?.kind === "verdict" ? o.findings : [];
  if (link) {
    const said = o?.kind === "verdict" ? `${o.verdict === "ship" ? "Ship" : "Do not ship"} · ${short(r?.sha ?? "")}` : "without review";
    return <>
      <Head icon="check-circle" color="var(--add)" title={run.state.result === "failed" ? "Pull request already open" : "Pull request opened"} meta={said} />
      <div className="review-pr"><PrLink link={link} /></div>
      {findings.length > 0 && <div className="actions">{toggle(findings.length)}</div>}
      {shown && findings.length > 0 && <Findings lane={p.lane} findings={findings} />}
      <Close {...p} />
    </>;
  }
  if (run.running) return <>
    <Head eyebrow="Keel review" title="Opening the pull request…" />
    <div className="review-pr small faint mono">{run.state.lines.at(-1) ?? ""}</div>
    <Close {...p} />
  </>;
  return <>
    <Banner tone="error">The pull request didn't open: {run.state.lines.at(-1) ?? "gh exited with an error"}</Banner>
    <div className="actions">
      <button className="primary" onClick={open}>Try again</button>
      {(!o || o.kind === "none") && <button onClick={() => { settle(p.ep, p.wt); void startReview(p.lane, p.ep, p.wt, r?.opening); }}>Review again</button>}
    </div>
    <Close {...p} />
  </>;
}

/// The review in `place`: the Pull request tab shows findings open, the dock folds them.
export function PreReview(p: Props) {
  const seen = useReview(p.lane);
  const pr = useRun<PrRun>(prKey(p.ep, p.wt));
  const r = seen?.review;
  if (!seen || (!r && !seen.restarted)) return null;
  const o = r?.outcome;
  const body: ReactNode =
    pr ? <PullRequestLine {...p} run={pr} r={r} /> :
    !r || o?.kind === "none" ? <Ended {...p} r={r} /> :
    !o ? <Running {...p} r={r} head={seen.head} /> :
    <Finished {...p} r={r} head={seen.head} o={o} />;
  const dock = p.place === "dock";
  // Esc goes back to the composer; it never dismisses — the review is not waiting on an answer.
  const keys = (e: React.KeyboardEvent) => { if (e.key === "Escape") { e.stopPropagation(); focusTerminal(); } };
  return (
    <div className={dock ? "strip review-strip" : "review-panel"} role="region" aria-label="Pull request review" onKeyDown={keys}>
      <div className="review-row">{body}</div>
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
