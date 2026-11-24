// Persistent tasks and explicit recurring authority are reachable without reopening a conversation first.
import { useState } from "react";
import { get, type Endpoint } from "../api";
import { AGENT_NAME, useStore } from "../store";
import { useDelivery, refreshDelivery, openTask, type TaskPage } from "../delivery";
import type { TaskSummary } from "../delivery/model";
import { Dialog, Empty, Tabs } from "./kit";
import { Icon } from "./icons";
import { AgentMark } from "./AgentMark";
import { IssueIntake } from "./TaskIssue";
import { RecurringTasks } from "./TaskSchedules";

/** A persistent entry complements Attention even when every task is currently quiet. */
export function TasksButton({ open }: { open: () => void }) {
  return <button className="attention-entry" onClick={open} aria-haspopup="dialog"><Icon name="branch" size={16} /><span>Tasks</span></button>;
}

type Tab = "tasks" | "recurring";

/** One scroller and a project selector keep all actions reachable at the minimum window size. */
export function TasksDialog({ close, initial }: { close: () => void; initial?: { project: string; recurring: boolean } }) {
  const order = useStore((s) => s.order), projects = useStore((s) => s.projects);
  const active = useStore((s) => s.active ? s.lanes[s.active]?.project : undefined);
  const [chosen, setChosen] = useState(initial?.project ?? active ?? order[0]), [tab, setTab] = useState<Tab>(initial?.recurring ? "recurring" : "tasks");
  const project = order.includes(chosen) ? chosen : order[0], ep = projects[project]?.endpoint;
  const data = useDelivery(ep), [error, setError] = useState("");
  async function open(task: string, title: string) { if (ep) try { await openTask(project, ep, task, title); close(); } catch (error) { setError(String(error)); } }
  const tabs = [
    { id: "tasks" as const, label: <>Tasks{data.tasks?.tasks.length ? <span className="tab-count">{data.tasks.tasks.length}</span> : null}</> },
    { id: "recurring" as const, label: <>Recurring{data.schedules?.schedules.length ? <span className="tab-count">{data.schedules.schedules.length}</span> : null}</> },
  ];
  return <Dialog title="Tasks" onClose={close}><div className="delivery-dialog">
    <button className="ghost icon-button tasks-close" aria-label="Close" title="Close (Esc)" onClick={close}><Icon name="x" size={14} /></button>
    <div className="tasks-toolbar">
      <Tabs tabs={tabs} value={tab} onChange={setTab} />
      <span className="spacer" />
      {order.length > 1 && <select className="tasks-project" aria-label="Project" value={project ?? ""} onChange={(event) => { setChosen(event.target.value); setError(""); }}>{order.map((path) => <option key={path} value={path}>{projects[path]?.name ?? path}</option>)}</select>}
      <button className="ghost icon-button" aria-label="Refresh" title="Refresh" disabled={!ep || data.loading} onClick={() => ep && void refreshDelivery(ep)}><Icon name="rotate-cw" size={14} /></button>
    </div>
    <div className="delivery-dialog-scroll" key={project}>
      {!ep ? <Empty title="Not connected">Open a project and connect its daemon to see saved tasks.</Empty> : tab === "tasks" ? <><IssueIntake ep={ep} open={open} /><TaskRows ep={ep} page={data.tasks} open={open} /></> : <RecurringTasks ep={ep} page={data.schedules} open={open} />}
      {(error || data.error) && <p className="error" role="alert">{error || data.error}</p>}
      {data.loading && !data.tasks && <p className="muted small" role="status">Reading saved tasks…</p>}
    </div>
  </div></Dialog>;
}

/// A task's title is often the whole prompt it started from, sometimes a bare URL. One line of it
/// reads as a name; the full text stays in the tooltip.
export function taskLabel(title: string): string {
  const line = title.trim().split("\n")[0].replace(/\s+/g, " ");
  if (!line) return "Untitled task";
  try {
    const url = new URL(line);
    const parts = url.pathname.split("/").filter(Boolean);
    return [parts.slice(0, 2).join("/"), parts.slice(2).join(" / ")].filter(Boolean).join(" · ") || url.host;
  } catch {
    return line;
  }
}

function TaskRow({ task, open }: { task: TaskSummary; open: (task: string, title: string) => Promise<void> }) {
  const meta = [AGENT_NAME[task.owner.provider] ?? task.owner.provider, task.source !== "Conversation task" ? task.source : "", task.check ? `checks ${task.check.status}` : ""].filter(Boolean).join(" · ");
  return <li>
    <button className="task-row" onClick={() => void open(task.task, task.title)} title={task.title}>
      <AgentMark agent={task.owner.provider} size={14} />
      <span className="task-main"><span className="task-title">{taskLabel(task.title)}</span><span className="task-meta">{meta}</span></span>
      {task.unresolved > 0 && <span className="task-pill">{task.unresolved} to fix</span>}
      <Icon name="chevron-right" size={14} />
    </button>
    {task.publication?.url && <a className="task-pr" href={task.publication.url}>Pull request <Icon name="external-link" size={11} /></a>}
  </li>;
}

function TaskRows({ ep, page, open }: { ep: Endpoint; page?: TaskPage; open: (task: string, title: string) => Promise<void> }) {
  const [extra, setExtra] = useState<TaskPage>(), [error, setError] = useState("");
  const rows = [...(page?.tasks ?? []), ...(extra?.tasks ?? [])], next = extra ? extra.next : page?.next;
  async function more() { try { const nextPage = await get<TaskPage>(ep, "/api/tasks", { after: next }); setExtra({ ...nextPage, tasks: [...(extra?.tasks ?? []), ...nextPage.tasks] }); } catch (error) { setError(String(error)); } }
  return <section aria-label="Saved tasks" className="tasks-saved">
    {page?.owner_error && <p className="error">{page.owner_error}</p>}
    {!rows.length && page && <Empty title="No saved tasks yet">Start from an issue above, save a recurring instruction, or work in a conversation — its task appears here.</Empty>}
    {rows.length > 0 && <ul className="task-list">{rows.map((task) => <TaskRow key={task.task} task={task} open={open} />)}</ul>}
    {next && <button className="ghost tasks-more" onClick={() => void more()}>Show more</button>}{error && <p className="error" role="alert">{error}</p>}
  </section>;
}
