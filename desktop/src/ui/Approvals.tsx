// Permission cards and question recovery share the lane dock without hiding terminal prompts.
import { useState } from "react";
import { createPortal } from "react-dom";
import { useStore, type Pending } from "../store";
import { Icon } from "./icons";
import { label } from "../keys";
import { Dialog, focusTerminal } from "./kit";
import { post } from "../api";
import { openUrl } from "@tauri-apps/plugin-opener";
import { Markdown } from "./Markdown";
import { Questions, QuestionRecoveries } from "./Questions";
import { shownTool } from "../browser";
import { BROWSER_CHOICE } from "../browser-preference";
import { ANSWER_WORDS, approvalWords } from "../approval-words";
import { checkoutOf, usePlain } from "../plain";

/// The answered row unmounts under the focused button; focus would land on nothing.
const after = () => requestAnimationFrame(() => document.activeElement === document.body && focusTerminal());

/// A refused command is a question, docked under the terminal so it neither hides the CLI's own
/// pending-tool line nor lives in a panel that can be closed. Focus is never taken from the
/// terminal: ⌘⇧A, ⌘⇧S and ⌘⇧D answer the oldest from anywhere, and ⌘⇧A focuses a question.
export function ApprovalStrip({ lane }: { lane: string }) {
  const pending = useStore((s) => s.lanes[lane]?.pending);
  const saved = useStore((s) => !!Object.keys(s.lanes[lane]?.questionDrafts ?? {}).length);
  if (!pending?.length && !saved) return null;
  return (
    <div className="strip" role="region" aria-label={pending?.length ? "Waiting for your answer" : "Saved question answers"}>
      {pending?.map((p, i) => (
        <Row key={p.id} lane={lane} p={p} count={i === 0 ? pending.length : 0} />
      ))}
      <QuestionRecoveries lane={lane} />
    </div>
  );
}

/** A subagent's call is labelled as one: its steps never appear in the turn it came from. */
const who = (p: Pending) => (p.subagent ? "A subagent of this turn" : "The agent");

/** What a non-command call carries — an MCP tool's arguments, a fetch's URL — at a length a card can draw. */
function Arguments({ input }: { input: Record<string, unknown> }) {
  if (!input || !Object.keys(input).length) return null;
  const text = JSON.stringify(input, null, 2);
  // Whole, in a scrolling box: a card that cut the arguments short would ask for approval of
  // something the person cannot read. The daemon refuses calls too large to show.
  return <pre className="code" style={{ maxHeight: 200, overflow: "auto" }}>{text}</pre>;
}

/** Characters of a one-line command shown before "Show all": a long line wraps into a wall. */
const COMMAND_GLANCE_CHARS = 240;

/** What a session rule covers, in a chat lane's words: the programs (`npm`), the server
 *  (`github`), or the folder (`changes in src`). */
function ruleNames(rules: string[]): string {
  const named = rules.map((r) => /^Bash\((\S+) \*\)$/.exec(r)?.[1] ?? /^mcp__([^_]+(?:_[^_]+)*)(?:__.*)?$/.exec(r)?.[1] ?? /^(?:Edit|Write)\((.+?)\/?\*?\)$/.exec(r)?.[1]?.replace(/^/, "changes in ") ?? r);
  return named.length < 2 ? named.join("") : `${named.slice(0, -1).join(", ")} and ${named.at(-1)}`;
}

/** The programs a session rule covers, as words: `grep, head and echo`, not `Bash(grep *), …`.
 *  The exact rules stay in the button's title. */
function sessionNames(rules: string[]): string {
  const names = rules.map((r) => /^Bash\((\S+) \*\)$/.exec(r)?.[1] ?? r);
  return names.length < 2 ? names.join("") : `${names.slice(0, -1).join(", ")} and ${names.at(-1)}`;
}

function Row({ lane, p, count }: { lane: string; p: Pending; count: number }) {
  const answer = useStore((s) => s.answer);
  const plain = usePlain(lane);
  const [all, setAll] = useState(false);
  if (p.tool === "AskUserQuestion") return <Questions lane={lane} p={p} />;
  if (p.tool === "ExitPlanMode") return <PlanApproval lane={lane} p={p} />;
  if (p.tool === BROWSER_CHOICE) return <BrowserChoice lane={lane} p={p} />;
  // A form to fill or a page to visit (Codex's elicitations) keeps its own card: its fields and its
  // "Open verification page" button are the request.
  if (plain && !(p.provider && (p.input.requestedSchema || typeof p.input.url === "string"))) return <PlainRequest lane={lane} p={p} count={count} />;
  if (p.provider) return <RuntimeRequest lane={lane} p={p} />;
  const lines = p.command.split("\n");
  const long = lines.length > 6 || p.command.length > COMMAND_GLANCE_CHARS;
  return (
    <div className="strip-row command-approval">
      <div className="strip-title">
        <Icon name="hand" className="hand" />
        <span style={{ flex: 1 }}>{who(p)} {p.tool === "Bash" ? "wants to run a command" : `wants to use ${shownTool(p.tool)}`}</span>
        {count > 1 && <span className="small faint">{count} waiting</span>}
      </div>
      {p.reason && <p className="small muted">{p.reason}</p>}
      {!p.command && <Arguments input={p.input} />}
      {p.command && (
        <pre className={`code approval-command${all ? " is-open" : ""}`}>$ {all ? p.command : lines.slice(0, 6).join("\n")}{!all && lines.length > 6 && "\n…"}</pre>
      )}
      {!all && long && (
        <button className="link small approval-more" onClick={() => setAll(true)}>
          Show all
        </button>
      )}
      <div className="actions">
        <button className="primary" onClick={() => (void answer(lane, p, "allow"), after())}>
          Allow once <kbd>{label({ key: "a", shift: true })}</kbd>
        </button>
        {/* A card asked because a deny rule can't be checked asks every time: a session rule would do nothing. */}
        {p.rules.length > 0 && !p.reason && (
          <button onClick={() => (void answer(lane, p, "allow", undefined, "session"), after())} title={`${p.rules.join(", ")} — remembered for this conversation only. The palette has “Always allow” for the whole project.`}>
            Allow {sessionNames(p.rules)} this session <kbd>{label({ key: "s", shift: true })}</kbd>
          </button>
        )}
        <button className="approval-deny" onClick={() => (void answer(lane, p, "deny"), after())}>
          Deny <kbd>{label({ key: "d", shift: true })}</kbd>
        </button>
      </div>
    </div>
  );
}

/** A chat lane's card: what will happen, in a sentence a first-time builder can answer, with the
 *  exact command behind Details — open from the start when the sentence alone is not enough. The
 *  answers are the same ones the terminal card sends; only the words differ. */
function PlainRequest({ lane, p, count }: { lane: string; p: Pending; count: number }) {
  const answer = useStore((s) => s.answer);
  const root = useStore((s) => (s.lanes[lane] ? checkoutOf(s.lanes[lane]) : undefined));
  const [sending, setSending] = useState(false);
  const said = approvalWords(p, root);
  const shown = p.command || (typeof p.input.command === "string" ? p.input.command : Array.isArray(p.input.command) ? p.input.command.join(" ") : "");
  // Codex's App Server offers its own decisions; Keel's cards answer allow, session or deny.
  const options: { value: unknown; label: string }[] = p.provider && p.choices?.length
    ? p.choices.map((c) => ({ value: c.value, label: typeof c.value === "string" ? ANSWER_WORDS[c.value] ?? c.label : c.label }))
    // The session answer covers the rules the daemon offers (`npm *`, a whole MCP server), which
    // can be wider than the sentence, so the button names them.
    : [{ value: "allow", label: ANSWER_WORDS.allow }, ...(p.rules.length > 0 && !p.reason ? [{ value: "session", label: `Allow ${ruleNames(p.rules)} for this chat` }] : []), { value: "deny", label: ANSWER_WORDS.deny }];
  async function choose(value: unknown) {
    setSending(true);
    if (p.provider) await runtimeAnswer(lane, p, { decision: value, content: null });
    else await answer(lane, p, value === "deny" ? "deny" : "allow", undefined, value === "session" ? "session" : "once");
    setSending(false);
    after();
  }
  const keys: Record<string, string> = { allow: "a", accept: "a", session: "s", acceptForSession: "s", deny: "d", decline: "d" };
  return <div className="strip-row command-approval">
    <div className="strip-title"><Icon name="hand" className="hand" /><span style={{ flex: 1 }}>{said.title}</span>{count > 1 && <span className="small faint">{count} waiting</span>}</div>
    {p.reason && <p className="small muted">{p.reason}</p>}
    {!p.reason && typeof p.input.reason === "string" && <p className="small muted">{p.input.reason}</p>}
    <details className="approval-details" open={said.open}>
      <summary>Details</summary>
      {shown ? <pre className="code approval-command is-open">$ {shown}</pre> : <Arguments input={p.input} />}
    </details>
    <div className="actions">{options.map((o, i) => {
      // ⇧⌘S answers a Keel card's session rule; a runtime card has none, so it shows no shortcut.
      const key = typeof o.value === "string" && !(p.provider && keys[o.value] === "s") ? keys[o.value] : undefined;
      return <button key={i} disabled={sending} className={i === 0 ? "primary" : o.value === "deny" || o.value === "decline" ? "approval-deny" : undefined} onClick={() => void choose(o.value)}>
        {o.label}{key && <> <kbd>{label({ key, shift: true })}</kbd></>}
      </button>;
    })}</div>
  </div>;
}

/** Where this lane browses, asked on the agent's first browser step and saved for the lane. When Oya
 * Browser isn't installed its button installs it; the agent's step waits and then runs there. */
function BrowserChoice({ lane, p }: { lane: string; p: Pending }) {
  const answer = useStore((s) => s.answer);
  const [sending, setSending] = useState(false);
  const installed = p.input.oya === "installed";
  async function choose(choice: "embedded" | "local_oya" | "install_oya") {
    setSending(true);
    await answer(lane, p, "allow", choice);
    setSending(false);
    after();
  }
  return <div className="strip-row browser-choice-card">
    <div className="strip-title"><Icon name="hand" className="hand" /><span style={{ flex: 1 }}>{who(p)} wants to use a browser. Where should this lane browse?</span></div>
    <div className="actions">
      <button className="primary" disabled={sending} onClick={() => void choose("embedded")} title="A browser in Keel's Browser tab. Sign-ins stay with this project.">
        Inside Keel <kbd>{label({ key: "a", shift: true })}</kbd>
      </button>
      <button disabled={sending} onClick={() => void choose(installed ? "local_oya" : "install_oya")} title={installed ? "Uses the Oya app on your computer and the accounts you're already signed into." : "Downloads and installs Oya Browser, then uses it for this lane."}>
        {installed ? "Your Oya Browser" : "Install Oya Browser"}
      </button>
    </div>
    <p className="small muted">
      <strong>Inside Keel</strong>: a browser in the Browser tab; sign-ins stay with this project.{" "}
      <strong>{installed ? "Oya Browser" : "Install Oya Browser"}</strong>: {installed ? "the Oya app on your computer, with the accounts you're already signed into." : "installs the Oya app, then this lane browses there with your own sign-ins."}{" "}
      Saved for this lane — change it any time in the Browser tab.
    </p>
  </div>;
}

function PlanApproval({ lane, p }: { lane: string; p: Pending }) {
  const answer = useStore((s) => s.answer);
  const writtenPlan = useStore((s) => {
    const turn = s.lanes[lane]?.conv.turns.at(-1);
    const write = Object.values(turn?.calls ?? {}).reverse().find((call) => (call.tool === "Write" || call.tool === "Edit") && call.state === "ok" && typeof call.input?.file_path === "string" && /[\\/]\.claude[\\/]plans[\\/]/.test(call.input.file_path));
    return write?.tool === "Write" && typeof write.input?.content === "string" ? write.input.content : undefined;
  });
  const [sending, setSending] = useState(false);
  const [reading, setReading] = useState(false);
  const plan = typeof p.input.plan === "string" ? p.input.plan : writtenPlan;
  const permissions = Array.isArray(p.input.allowedPrompts) ? p.input.allowedPrompts : [];
  async function decide(decision: "allow" | "deny") {
    setSending(true);
    await answer(lane, p, decision);
    setSending(false);
    after();
  }
  const approve = <button className="primary" disabled={sending} onClick={() => void decide("allow")}>Approve plan <kbd>{label({ key: "a", shift: true })}</kbd></button>;
  const stay = <button disabled={sending} onClick={() => void decide("deny")}>Stay in plan mode <kbd>{label({ key: "d", shift: true })}</kbd></button>;
  return <div className="strip-row plan-approval">
    <header className="plan-approval-header">
      <div><div className="approval-eyebrow">Your approval</div><div className="strip-title">Ready to implement</div></div>
      {plan && <button className="link" onClick={() => setReading(true)}><Icon name="maximize" size={14} />Read full plan</button>}
    </header>
    <p className="approval-description">Review the plan, then approve to start making changes.</p>
    <div className="plan-approval-body" tabIndex={0} aria-label="Implementation plan">
      {plan ? <Markdown text={plan} /> : <p className="muted">No plan text was provided. Review the conversation before approving.</p>}
      {permissions.length > 0 && <details className="approval-plan"><summary>Requested permissions</summary><ul>{permissions.map((p, i) => <li key={i}>{typeof p?.prompt === "string" ? p.prompt : JSON.stringify(p)}</li>)}</ul></details>}
    </div>
    <div className="actions plan-approval-actions">{approve}{stay}</div>
    {reading && plan && createPortal(<Dialog title="Review plan" onClose={() => setReading(false)}>
      <div className="plan-reader-content" tabIndex={0}><Markdown text={plan} />{permissions.length > 0 && <details><summary>Requested permissions</summary><ul>{permissions.map((p, i) => <li key={i}>{typeof p?.prompt === "string" ? p.prompt : JSON.stringify(p)}</li>)}</ul></details>}</div>
      <div className="actions plan-reader-actions"><button onClick={() => setReading(false)}>Back to conversation</button><span style={{ flex: 1 }} />{stay}{approve}</div>
    </Dialog>, document.body)}
  </div>;
}

async function runtimeAnswer(lane: string, p: Pending, answer: unknown) {
  const s = useStore.getState();
  const l = s.lanes[lane];
  const ep = l && s.projects[l.project]?.endpoint;
  if (!ep) return;
  try {
    const receipt = await post<{ ok: boolean; reason?: string }>(ep, "/api/chat/control", { lane, method: "answer", id: p.id, answer });
    if (!receipt.ok) {
      // Nothing is waiting on it any more: the card goes, as a stale question's does, rather than
      // staying to fail on every click until the turn ends.
      useStore.setState((s) => s.lanes[lane] ? { lanes: { ...s.lanes, [lane]: { ...s.lanes[lane], pending: s.lanes[lane].pending.filter((x) => x.id !== p.id), error: receipt.reason ?? "That request is no longer waiting for an answer." } } } : s);
      return;
    }
  }
  catch (error) { useStore.setState((s) => ({ lanes: { ...s.lanes, [lane]: { ...s.lanes[lane], error: `Could not deliver your answer: ${error}` } } })); }
}

function RuntimeRequest({ lane, p }: { lane: string; p: Pending }) {
  const [content, setContent] = useState("{}");
  const [error, setError] = useState("");
  const [sending, setSending] = useState(false);
  const schema = p.input.requestedSchema;
  async function answer(decision: unknown) {
    let value;
    try { value = schema && decision === "accept" ? JSON.parse(content) : null; }
    catch { setError("Enter valid JSON for the requested fields."); return; }
    setSending(true);
    await runtimeAnswer(lane, p, { decision, content: value });
    setSending(false);
  }
  return <div className="strip-row runtime-approval"><div className="strip-title"><Icon name="hand" />{shownTool(p.tool)}</div>
    <div className="approval-context">
    {p.command && <pre className="code">{p.command}</pre>}
    {typeof p.input.reason === "string" && <p>{p.input.reason}</p>}
    {typeof p.input.url === "string" && /^https?:\/\//.test(p.input.url) && <button onClick={() => void openUrl(String(p.input.url))}>Open verification page</button>}
    <details><summary>Request details</summary><pre className="code">{JSON.stringify(p.input, null, 2)}</pre></details>
    {!!schema && <><pre className="code">{JSON.stringify(schema, null, 2)}</pre><textarea aria-label="Requested fields as JSON" value={content} onChange={(e) => setContent(e.target.value)} /></>}
    </div>
    {error && <p role="alert">{error}</p>}
    {/* A request that offers no choices still has to be answerable: a card with no buttons held the turn until it timed out. */}
    <div className="actions">{p.choices?.length ? p.choices.map((choice, i) => <button key={i} disabled={sending} className={i === 0 ? "primary" : undefined} onClick={() => void answer(choice.value)}>{choice.label}</button>) : <>{!!schema && <button className="primary" disabled={sending} onClick={() => void answer("accept")}>Send</button>}<button disabled={sending} onClick={() => void answer("decline")}>Decline</button></>}</div>
  </div>;
}
