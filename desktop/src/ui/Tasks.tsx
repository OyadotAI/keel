// Persistent tasks and explicit recurring authority are reachable without reopening a conversation first.
import { useState } from "react";
import { get, type Endpoint } from "../api";
import { useStore } from "../store";
import { useDelivery, refreshDelivery, openTask, type TaskPage } from "../delivery";
import { Dialog } from "./kit";
import { Icon } from "./icons";
import { IssueIntake } from "./TaskIssue";
import { RecurringTasks } from "./TaskSchedules";

/** A persistent entry complements Attention even when every task is currently quiet. */
export function TasksButton({ open }: { open: () => void }) {
  return <button className="attention-entry" onClick={open} aria-haspopup="dialog"><Icon name="branch" size={16} /><span>Tasks</span></button>;
}
/** One scroller and a project selector keep all actions reachable at the minimum window size. */
export function TasksDialog({ close, initial }: { close: () => void; initial?: { project: string; recurring: boolean } }) {
  const order = useStore((s) => s.order), projects = useStore((s) => s.projects);
  const active = useStore((s) => s.active ? s.lanes[s.active]?.project : undefined);
  const [chosen, setChosen] = useState(initial?.project ?? active ?? order[0]), [tab, setTab] = useState(initial?.recurring ? "recurring" : "tasks");
  const project = order.includes(chosen) ? chosen : order[0], ep = projects[project]?.endpoint;
  const data = useDelivery(ep), [error, setError] = useState("");
  async function open(task: string, title: string) { if (ep) try { await openTask(project, ep, task, title); close(); } catch (error) { setError(String(error)); } }
  return <Dialog title="Tasks" onClose={close}><div className="delivery-dialog">
    <div className="delivery-dialog-head"><label>Project<select value={project ?? ""} onChange={(event) => { setChosen(event.target.value); setError(""); }}>{order.map((path) => <option key={path} value={path}>{projects[path]?.name ?? path}</option>)}</select></label>
      <div className="actions"><button aria-pressed={tab === "tasks"} onClick={() => setTab("tasks")}>Tasks</button><button aria-pressed={tab === "recurring"} onClick={() => setTab("recurring")}>Recurring</button><button disabled={!ep || data.loading} onClick={() => ep && void refreshDelivery(ep)}>Refresh</button></div></div>
    <div className="delivery-dialog-scroll" key={project}>
      {!ep ? <p>Open a project and connect its daemon to see saved tasks.</p> : tab === "tasks" ? <><IssueIntake ep={ep} open={open} /><TaskRows ep={ep} page={data.tasks} open={open} /></> : <RecurringTasks ep={ep} page={data.schedules} open={open} />}
      {(error || data.error) && <p className="error" role="alert">{error || data.error}</p>}
      {data.loading && <p role="status">Reading saved tasks…</p>}
    </div><div className="actions"><button onClick={close}>Close</button></div>
  </div></Dialog>;
}
function TaskRows({ ep, page, open }: { ep: Endpoint; page?: TaskPage; open: (task: string, title: string) => Promise<void> }) {
  const [extra, setExtra] = useState<TaskPage>(), [error, setError] = useState("");
  const rows = [...(page?.tasks ?? []), ...(extra?.tasks ?? [])], next = extra ? extra.next : page?.next;
  async function more() { try { const nextPage = await get<TaskPage>(ep, "/api/tasks", { after: next }); setExtra({ ...nextPage, tasks: [...(extra?.tasks ?? []), ...nextPage.tasks] }); } catch (error) { setError(String(error)); } }
  return <section aria-label="Saved tasks"><h3>Saved tasks</h3>{page?.owner_error && <p className="error">{page.owner_error}</p>}
    {!rows.length && page && <p className="muted">Start from an issue, save a recurring instruction, or work in a conversation. Its task will appear here.</p>}
    <ul className="delivery-task-list">{rows.map((task) => <li key={task.task}><div><strong>{task.title || "Conversation task"}</strong><p className="small muted">{task.source} · {task.owner.provider}</p><p className="small">{task.unresolved} unresolved corrections{task.check && ` · Last recorded checks: ${task.check.status}`}</p>{task.publication?.url && <a href={task.publication.url}>Pull request</a>}</div><button onClick={() => void open(task.task, task.title)}>Open task</button></li>)}</ul>
    {next && <button onClick={() => void more()}>More tasks</button>}{error && <p role="alert">{error}</p>}
  </section>;
}
