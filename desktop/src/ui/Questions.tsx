// Question writing stays in lane state while the card follows the provider's actual lifetime.
import { useStore, type Pending } from "../store";
import { emptyQuestion, questions, questionAnswer, recoveryMessage } from "../question-drafts";
import { Icon } from "./icons";

/** Options and custom responses are submitted only by the explicit send action. */
export function Questions({ lane, p }: { lane: string; p: Pending }) {
  const saved = useStore((s) => s.lanes[lane]?.questionDrafts?.[p.id]);
  const draft = saved ?? emptyQuestion(p);
  const rows = questions(p);
  const complete = rows.length > 0 && rows.every((_, i) => !!questionAnswer(draft, i).trim());
  const open = rows.findIndex((_, i) => !questionAnswer(draft, i).trim());
  const store = useStore.getState();
  function keys(e: React.KeyboardEvent<HTMLDivElement>) {
    if (e.metaKey || e.ctrlKey || e.altKey || (e.target instanceof HTMLElement && e.target.closest("input, textarea, [contenteditable=true]"))) return;
    const option = open >= 0 ? rows[open].options?.[Number(e.key) - 1] : undefined;
    if (option && !draft.sending) { e.preventDefault(); store.pickQuestion(lane, p, open, option.label); return; }
    if (e.key !== "ArrowRight" && e.key !== "ArrowLeft") return;
    const buttons = [...e.currentTarget.querySelectorAll<HTMLElement>(".actions button")];
    const at = buttons.indexOf(document.activeElement as HTMLElement);
    buttons[(at + (e.key === "ArrowRight" ? 1 : -1) + buttons.length) % buttons.length]?.focus();
    e.preventDefault();
  }
  return <div className="strip-row question-card" onKeyDown={keys}>
    <div className="strip-title"><Icon name="hand" className="hand" />The agent is asking</div>
    {rows.map((q, i) => {
      const value = draft.values[i];
      return <div className="question-field" key={q.id ?? i}>
        <div className="question-title">{q.question}</div>
        <div className="actions">{(q.options ?? []).map((option, at) => {
          const selected = value?.mode === "options" && value.selected.includes(option.label);
          return <button key={option.label} title={option.description} aria-pressed={!!selected} className={selected ? "primary" : undefined} disabled={draft.sending} onClick={() => store.pickQuestion(lane, p, i, option.label)}>{i === open && at < 9 && <kbd>{at + 1}</kbd>}{option.label}</button>;
        })}</div>
        <textarea className="field question-answer" rows={3} aria-label={`Your answer: ${q.question}`} placeholder="Or write your own answer…" value={value?.custom ?? ""} disabled={draft.sending}
          onFocus={() => { if (value?.custom) store.writeQuestion(lane, p, i, value.custom); }} onChange={(e) => store.writeQuestion(lane, p, i, e.target.value)} />
        {value?.mode === "options" && value.custom.trim() && <p className="small muted">The selected option will be sent. Your written answer is saved above.</p>}
      </div>;
    })}
    {draft.error && <p role="alert" className="small error">{draft.error}</p>}
    {rows.length > 0 && <button className="primary" disabled={!complete || draft.sending} onClick={() => void store.answerQuestion(lane, p)}>{draft.sending ? "Sending…" : draft.error ? "Retry answer" : complete ? "Send answers" : `Answer all ${rows.length} questions`}</button>}
  </div>;
}

/** Closed requests preserve writing without contributing to the lane's live waiting indicator. */
export function QuestionRecoveries({ lane }: { lane: string }) {
  const drafts = useStore((s) => s.lanes[lane]?.questionDrafts);
  const pending = useStore((s) => s.lanes[lane]?.pending);
  const loaded = useStore((s) => s.lanes[lane]?.loaded);
  if (!loaded) return null;
  const closed = Object.values(drafts ?? {}).filter((draft) => !pending?.some((p) => p.id === draft.request.id));
  return <>{closed.map((draft) => <div className="strip-row question-recovery" key={draft.request.id}>
    <div className="strip-title">{draft.sending ? "Confirming answer delivery…" : draft.submitted && draft.error ? "This question closed before delivery could be confirmed." : "This question closed before your answer was delivered."}</div>
    <pre className="code">{recoveryMessage(draft)}</pre>
    {!draft.sending && <div className="actions"><button onClick={() => useStore.getState().useQuestionAsMessage(lane, draft.request.id)}>Use as message</button><button onClick={() => useStore.getState().dismissQuestion(lane, draft.request.id)}>Dismiss</button></div>}
  </div>)}</>;
}
