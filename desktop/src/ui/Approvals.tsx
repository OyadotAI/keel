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
import { BROWSER_CHOICE } from "../browser-preference";

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

/** The programs a session rule covers, as words: `grep, head and echo`, not `Bash(grep *), …`.
 *  The exact rules stay in the button's title. */
function sessionNames(rules: string[]): string {
  const names = rules.map((r) => /^Bash\((\S+) \*\)$/.exec(r)?.[1] ?? r);
  return names.length < 2 ? names.join("") : `${names.slice(0, -1).join(", ")} and ${names.at(-1)}`;
}

function Row({ lane, p, count }: { lane: string; p: Pending; count: number }) {
  const answer = useStore((s) => s.answer);
  const [all, setAll] = useState(false);
  if (p.tool === "AskUserQuestion") return <Questions lane={lane} p={p} />;
  if (p.tool === "ExitPlanMode") return <PlanApproval lane={lane} p={p} />;
  if (p.tool === BROWSER_CHOICE) return <BrowserChoice lane={lane} p={p} />;
  if (p.provider) return <RuntimeRequest lane={lane} p={p} />;
  const lines = p.command.split("\n");
  const long = lines.length > 6 || p.command.length > COMMAND_GLANCE_CHARS;
  return (
    <div className="strip-row command-approval">
      <div className="strip-title">
        <Icon name="hand" className="hand" />
        <span style={{ flex: 1 }}>{who(p)} wants to run {p.tool === "Bash" ? "a command" : p.tool}</span>
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
    if (!receipt.ok) throw new Error(receipt.reason ?? "That request is no longer pending.");
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
  return <div className="strip-row runtime-approval"><div className="strip-title"><Icon name="hand" />{p.tool}</div>
    <div className="approval-context">
    {p.command && <pre className="code">{p.command}</pre>}
    {typeof p.input.reason === "string" && <p>{p.input.reason}</p>}
    {typeof p.input.url === "string" && /^https?:\/\//.test(p.input.url) && <button onClick={() => void openUrl(String(p.input.url))}>Open verification page</button>}
    <details><summary>Request details</summary><pre className="code">{JSON.stringify(p.input, null, 2)}</pre></details>
    {!!schema && <><pre className="code">{JSON.stringify(schema, null, 2)}</pre><textarea aria-label="Requested fields as JSON" value={content} onChange={(e) => setContent(e.target.value)} /></>}
    </div>
    {error && <p role="alert">{error}</p>}
    <div className="actions">{p.choices?.map((choice, i) => <button key={i} disabled={sending} className={i === 0 ? "primary" : undefined} onClick={() => void answer(choice.value)}>{choice.label}</button>)}</div>
  </div>;
}
