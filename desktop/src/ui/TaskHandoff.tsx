// The Context task section makes shared notes inspectable before any explicit provider switch.
import { AGENT_NAME } from "../store";
import { repositoryChanged, type TaskSaved, type TaskView } from "../handoffs";
import { useHandoff } from "../handoffs/useHandoff";

function Provenance({ saved }: { saved: TaskSaved }) {
  return <dl className="handoff-meta">
    <dt>Source</dt><dd>{AGENT_NAME[saved.source.provider]} · {saved.source.session}</dd>
    <dt>Saved</dt><dd>{new Date(saved.at).toLocaleString()} · revision {saved.revision}</dd>
    <dt>Checkout</dt><dd>{saved.source.checkout}</dd>
    <dt>Branch / HEAD</dt><dd>{saved.branch || "Detached"} · {saved.head || "No commits"}</dd>
  </dl>;
}
function History({ view }: { view: TaskView }) {
  return <details className="context-memory-activity"><summary>Shared task changelog · {view.events.length}</summary>
    <p className="context-note">Saved under the project’s .keel/CHANGELOG.md. {view.truncated && "Earlier events were omitted by the retention limit."}</p>
    <ol className="handoff-history">{view.events.map((e) => <li key={e.sequence}><strong>{e.kind}</strong> · {AGENT_NAME[e.source.provider]}<pre>{e.summary}</pre>{e.next_action && <p>Next: {e.next_action}</p>}</li>)}</ol>
  </details>;
}
/** Saved evidence stays readable even when the source or provider is unavailable. */
function SavedProgress({ view }: { view?: TaskView }) {
  const saved = view?.saved;
  if (!saved) return <p className="context-note">Save this task’s progress to continue with another coding agent.</p>;
  return <>
    <p>{view?.task?.objective || "Objective not captured"}</p>
    {view?.task?.notes.next_action && <p>Next: {view.task.notes.next_action}</p>}
    <Provenance saved={saved} />
    {saved.incomplete && <p className="context-note">Incomplete capture. Recovered the latest saved evidence; final progress may be missing.</p>}
    {view && repositoryChanged(view) && <p className="context-note">Repository state changed since this handoff. The next agent must verify current files and commits. Current: {view.current_branch || "Detached"} · {view.current_head || "No commits"}.</p>}
    {view?.task && view.task.revision > saved.revision && <p className="context-note">Newer progress exists. Continue uses revision {saved.revision}; save the latest handoff to share newer progress.</p>}
    <details className="context-preview"><summary>View handoff</summary><pre tabIndex={0}>{saved.text}</pre></details>
  </>;
}
function ContinuationActions({ h }: { h: ReturnType<typeof useHandoff> }) {
  const { view, busy, provider } = h;
  const blocked = !!busy || !view?.saved || !view.checkout_exists || !!view.ownership_error;
  const currentOwner = view?.task?.owner.session === h.lane?.session;
  return <>
    <div className="handoff-actions">
      <button disabled={!!busy || !view?.task} onClick={() => void h.save()}>{view?.saved ? "Save latest handoff" : "Save handoff"}</button>
      {view?.ownership_error && <button disabled={!!busy} onClick={() => void h.stop()}>Stop source work</button>}
      <button disabled={!!busy} onClick={() => void h.refresh()}>Retry check</button>
    </div>
    {view?.task?.continuation && !currentOwner && <button disabled={!!busy} onClick={() => void h.recover()}>Open continuation</button>}
    {view?.saved && currentOwner && <>
      <ProviderChoice h={h} />
      <button className="primary" disabled={blocked || !provider} onClick={() => void h.proceed()}>Continue with {provider ? AGENT_NAME[provider] : "selected agent"}</button>
      <p className="context-note">Starts a new session using this handoff and the same checkout. Stop source work separately before continuing.</p>
    </>}
  </>;
}
function ProviderChoice({ h }: { h: ReturnType<typeof useHandoff> }) {
  return <label className="handoff-provider">Continue with <select value={h.provider} disabled={!!h.busy} onChange={(e) => h.setProvider(e.target.value as typeof h.provider)}>
    <option value="">Choose an agent</option>{Object.entries(AGENT_NAME).map(([id, name]) => <option key={id} value={id}>{name}</option>)}
  </select></label>;
}
/** The inspected saved revision remains the one used by explicit continuation. */
export function TaskHandoff({ lane }: { lane: string }) {
  const h = useHandoff(lane);
  const { view, busy, error } = h;
  return <details className="context-section task-handoff" open><summary><span>Task handoff</span></summary><div className="context-section-body">
    <p className="context-note">Shared task memory and progress live in the project’s .keel/ folder. Native agent memories stay with their providers.</p>
    {view?.guidance && <p className="context-note" title={`${view.guidance.version} · ${view.guidance.hash}`}>Delegation guidance: {view.guidance.status}. {view.guidance.mechanism}.</p>}
    <SavedProgress view={view} />
    {view && !view.checkout_exists && <p className="context-error">The original checkout is unavailable. Restore or reopen it before continuing.</p>}
    {view?.ownership_error && <p className="context-note">{view.ownership_error}</p>}
    <ContinuationActions h={h} />
    <div role="status" aria-live="polite">{busy}</div>
    {error && <p className="context-error" role="alert">{error}</p>}
    {view && <History view={view} />}
  </div></details>;
}
