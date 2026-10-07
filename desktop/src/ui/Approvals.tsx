import { useState } from "react";
import { useStore, type Pending } from "../store";
import { Icon } from "./icons";
import { label } from "../keys";
import { focusTerminal } from "./kit";
import { post } from "../api";
import { openUrl } from "@tauri-apps/plugin-opener";
import { Markdown } from "./Markdown";

/// The answered row unmounts under the focused button; focus would land on nothing.
const after = () => requestAnimationFrame(() => document.activeElement === document.body && focusTerminal());

interface Question {
  id?: string;
  question: string;
  options?: { label: string; description?: string }[];
  multiSelect?: boolean;
}

/// A refused command is a question, docked under the terminal so it neither hides the CLI's own
/// pending-tool line nor lives in a panel that can be closed. Focus is never taken from the
/// terminal: ⌘⇧A, ⌘⇧S and ⌘⇧D answer the oldest from anywhere, and ⌘⇧A focuses a question.
export function ApprovalStrip({ lane }: { lane: string }) {
  const pending = useStore((s) => s.lanes[lane]?.pending);
  if (!pending?.length) return null;
  return (
    <div className="strip" role="region" aria-label="Waiting for your answer">
      {pending.map((p, i) => (
        <Row key={p.id} lane={lane} p={p} count={i === 0 ? pending.length : 0} />
      ))}
    </div>
  );
}

function Row({ lane, p, count }: { lane: string; p: Pending; count: number }) {
  const answer = useStore((s) => s.answer);
  const [all, setAll] = useState(false);
  if (p.tool === "AskUserQuestion") return <Questions lane={lane} p={p} />;
  if (p.tool === "ExitPlanMode") return <PlanApproval lane={lane} p={p} />;
  if (p.provider) return <RuntimeRequest lane={lane} p={p} />;
  const lines = p.command.split("\n");
  return (
    <div className="strip-row">
      <div className="strip-title">
        <Icon name="hand" className="hand" />
        <span style={{ flex: 1 }}>The agent wants to run {p.tool === "Bash" ? "a command" : p.tool}</span>
        {count > 1 && <span className="small faint">{count} waiting</span>}
      </div>
      {p.command && (
        <pre className="code" style={{ maxHeight: all ? 320 : undefined }}>
          $ {all ? p.command : lines.slice(0, 6).join("\n")}
          {!all && lines.length > 6 && "\n…"}
        </pre>
      )}
      {!all && lines.length > 6 && (
        <button className="link small" onClick={() => setAll(true)}>
          Show all
        </button>
      )}
      <div className="actions">
        <button className="primary" onClick={() => (void answer(lane, p, "allow"), after())}>
          Allow once <kbd>{label({ key: "a", shift: true })}</kbd>
        </button>
        {p.rules.length > 0 && (
          <button onClick={() => (void answer(lane, p, "allow", undefined, "session"), after())} title="Remembered for this conversation only. The palette has “Always allow” for the whole project.">
            Allow <code>{p.rules.join(", ")}</code> this session <kbd>{label({ key: "s", shift: true })}</kbd>
          </button>
        )}
        <button onClick={() => (void answer(lane, p, "deny"), after())}>
          Deny <kbd>{label({ key: "d", shift: true })}</kbd>
        </button>
      </div>
    </div>
  );
}

function PlanApproval({ lane, p }: { lane: string; p: Pending }) {
  const answer = useStore((s) => s.answer);
  const writtenPlan = useStore((s) => {
    const turn = s.lanes[lane]?.conv.turns.at(-1);
    const write = Object.values(turn?.calls ?? {}).reverse().find((call) => (call.tool === "Write" || call.tool === "Edit") && call.state === "ok" && typeof call.input?.file_path === "string" && /[\\/]\.claude[\\/]plans[\\/]/.test(call.input.file_path));
    return write?.tool === "Write" && typeof write.input?.content === "string" ? write.input.content : undefined;
  });
  const [sending, setSending] = useState(false);
  const plan = typeof p.input.plan === "string" ? p.input.plan : writtenPlan;
  const permissions = Array.isArray(p.input.allowedPrompts) ? p.input.allowedPrompts : [];
  async function decide(decision: "allow" | "deny") {
    setSending(true);
    await answer(lane, p, decision);
    setSending(false);
    after();
  }
  return <div className="strip-row plan-approval">
    <div className="approval-eyebrow">Your approval</div>
    <div className="strip-title">Ready to implement</div>
    <p className="approval-description">Review the plan before allowing the agent to leave plan mode and start making changes.</p>
    {plan && <details className="approval-plan" open><summary>Implementation plan</summary><Markdown text={plan} /></details>}
    {permissions.length > 0 && <details className="approval-plan"><summary>Requested permissions</summary><ul>{permissions.map((p, i) => <li key={i}>{typeof p?.prompt === "string" ? p.prompt : JSON.stringify(p)}</li>)}</ul></details>}
    <div className="actions">
      <button className="primary" disabled={sending} onClick={() => void decide("allow")}>Approve plan <kbd>{label({ key: "a", shift: true })}</kbd></button>
      <button disabled={sending} onClick={() => void decide("deny")}>Stay in plan mode <kbd>{label({ key: "d", shift: true })}</kbd></button>
    </div>
  </div>;
}

function Questions({ lane, p }: { lane: string; p: Pending }) {
  const answer = useStore((s) => s.answer);
  const questions = (p.input.questions as Question[] | undefined) ?? [];
  const [picked, setPicked] = useState<Record<number, string>>({});
  const [selected, setSelected] = useState<Record<number, string[]>>({});
  const complete = questions.length > 0 && questions.every((_, i) => !!picked[i]?.trim());
  function send(chosen: Record<number, string>) {
    if (p.provider) {
      void runtimeAnswer(lane, p, { answers: Object.fromEntries(questions.map((q, i) => [p.provider === "codex" ? q.id ?? q.question : q.question, chosen[i]])) });
      return;
    }
    void answer(lane, p, "deny", questions.map((q, i) => `${q.question}: ${chosen[i]}`).join("\n"));
    after();
  }
  function pick(i: number, option: string) {
    if (questions[i].multiSelect) {
      const current = selected[i] ?? [];
      const next = current.includes(option) ? current.filter((o) => o !== option) : [...current, option];
      setSelected({ ...selected, [i]: next });
      setPicked({ ...picked, [i]: next.join(", ") });
      return;
    }
    const next = { ...picked, [i]: option };
    setPicked(next);
  }
  // 1–9 pick from the first question still open; ←→ walk the options. Only while focus is here,
  // so the digits never leave the terminal.
  const open = questions.findIndex((_, i) => picked[i] === undefined);
  function keys(e: React.KeyboardEvent<HTMLDivElement>) {
    if (e.metaKey || e.ctrlKey || e.altKey || (e.target instanceof HTMLElement && e.target.closest("input, textarea, [contenteditable=true]"))) return;
    const n = Number(e.key);
    const option = open >= 0 && n >= 1 ? questions[open].options?.[n - 1] : undefined;
    if (option) return (e.preventDefault(), pick(open, option.label));
    if (e.key !== "ArrowRight" && e.key !== "ArrowLeft") return;
    const all = [...e.currentTarget.querySelectorAll<HTMLElement>(".actions button")];
    const at = all.indexOf(document.activeElement as HTMLElement);
    all[(at + (e.key === "ArrowRight" ? 1 : -1) + all.length) % all.length]?.focus();
    e.preventDefault();
  }
  return (
    <div className="strip-row" onKeyDown={keys}>
      <div className="strip-title">
        <Icon name="hand" className="hand" />
        The agent is asking
      </div>
      {questions.map((q, i) => (
        <div key={i} style={{ marginBottom: 8 }}>
          <div style={{ fontWeight: 500 }}>{q.question}</div>
          <div className="actions">
            {(q.options ?? []).map((o, n) => (
              <button
                key={o.label}
                title={o.description}
                className={(q.multiSelect ? selected[i]?.includes(o.label) : picked[i] === o.label) ? "primary" : undefined}
                aria-pressed={q.multiSelect ? !!selected[i]?.includes(o.label) : picked[i] === o.label}
                onClick={() => pick(i, o.label)}
              >
                {i === open && n < 9 && <kbd>{n + 1}</kbd>}
                {o.label}
              </button>
            ))}
          </div>
          <input className="field" aria-label={`Your answer: ${q.question}`} placeholder="Or write your own answer…" value={picked[i] ?? ""} onChange={(e) => { setSelected((old) => ({ ...old, [i]: [] })); setPicked((old) => ({ ...old, [i]: e.target.value })); }} />
        </div>
      ))}
      {questions.length > 0 && (
        <button className="primary" disabled={!complete} onClick={() => send(picked)}>
          {complete ? "Send answers" : `Answer all ${questions.length} questions`}
        </button>
      )}
    </div>
  );
}

async function runtimeAnswer(lane: string, p: Pending, answer: unknown) {
  const s = useStore.getState();
  const l = s.lanes[lane];
  const ep = l && s.projects[l.project]?.endpoint;
  if (!ep) return;
  try { await post(ep, "/api/chat/control", { lane, method: "answer", id: p.id, answer }); }
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
  return <div className="strip-row"><div className="strip-title"><Icon name="hand" />{p.tool}</div>
    {p.command && <pre className="code">{p.command}</pre>}
    {typeof p.input.reason === "string" && <p>{p.input.reason}</p>}
    {typeof p.input.url === "string" && /^https?:\/\//.test(p.input.url) && <button onClick={() => void openUrl(String(p.input.url))}>Open verification page</button>}
    <details><summary>Request details</summary><pre className="code">{JSON.stringify(p.input, null, 2)}</pre></details>
    {!!schema && <><pre className="code">{JSON.stringify(schema, null, 2)}</pre><textarea aria-label="Requested fields as JSON" value={content} onChange={(e) => setContent(e.target.value)} /></>}
    {error && <p role="alert">{error}</p>}
    <div className="actions">{p.choices?.map((choice, i) => <button key={i} disabled={sending} className={i === 0 ? "primary" : undefined} onClick={() => void answer(choice.value)}>{choice.label}</button>)}</div>
  </div>;
}
