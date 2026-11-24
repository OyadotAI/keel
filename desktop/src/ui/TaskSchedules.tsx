// Recurrence stays local, explicitly enabled and separate from stopping its active occurrence.
import { useState } from "react";
import { get,post, type Endpoint } from "../api";
import { AGENT_NAME, type Agent } from "../store";
import { deliveryChanged, requestId, forgetRequest, type Schedule, type SchedulePage } from "../delivery";
const SECONDS_PER_MINUTE = 60, DEFAULT_INTERVAL_MINUTES = 1440, DEFAULT_DURATION_MINUTES = 30;
const CONTROLLER_REVISION=0;

/** Definitions are saved paused and every occurrence remains available as a normal task. */
export function RecurringTasks({ ep, page, open }: { ep: Endpoint; page?: SchedulePage; open: (task: string, title: string) => Promise<void> }) {
  const [editing, setEditing] = useState<Schedule | "new">(), [error, setError] = useState(""), [busy, setBusy] = useState("");
  const [extra,setExtra]=useState<SchedulePage>();
  const active=page?.active_occurrence??page?.occurrences.find((occurrence)=>occurrence.id===page.active);
  const history=[...(active?[active]:[]),...new Map([...(page?.occurrences??[]),...(extra?.occurrences??[])].filter((occurrence)=>occurrence.id!==active?.id).map((occurrence)=>[occurrence.id,occurrence])).values()];
  const next=extra?extra.next:page?.next;
  async function more(){try{const history=await get<SchedulePage>(ep,"/api/delivery/schedules",{after:next});setExtra({...history,occurrences:[...(extra?.occurrences??[]),...history.occurrences]});}catch(error){setError(String(error));}}
  async function act(schedule: Pick<Schedule,"id"|"revision">, action: string) {
    setBusy(schedule.id); setError("");
    const receipt = action === "run_now" ? requestId(`schedule:${ep.port}:${schedule.id}`, { revision: schedule.revision }) : null;
    try { await post(ep, `/api/delivery/schedules/${schedule.id}`, { action, revision: schedule.revision, receipt }); if (receipt) forgetRequest(receipt); deliveryChanged(ep); }
    catch (error) { setError(String(error)); } finally { setBusy(""); }
  }
  return <section aria-label="Recurring tasks"><p className="muted">Runs only while this project's daemon and host are running. Missed intervals are skipped. One scheduled occurrence runs per project. Publishing remains your choice.</p>
    {page?.controller_error && <div role="alert"><p className="error">Recurring dispatch paused: {page.controller_error}</p><p className="small">New automatic runs are paused. Active-run observation and Stop remain available. Retry after restoring storage capacity or access.</p><button disabled={!!busy} onClick={()=>void act({id:"dispatcher",revision:CONTROLLER_REVISION},"retry")}>Retry recurring dispatcher</button></div>}
    <button onClick={() => setEditing("new")}>New recurring task</button>{editing && <ScheduleForm key={typeof editing === "string" ? editing : editing.id} ep={ep} schedule={editing === "new" ? undefined : editing} done={() => { setEditing(undefined); deliveryChanged(ep); }} />}
    <ul className="delivery-task-list">{page?.schedules.map((schedule) => <li key={schedule.id}><div><strong>{schedule.name}</strong><p>{schedule.enabled ? page?.controller_error ? "Dispatch paused" : "Enabled" : "Paused"} · Every {schedule.interval / SECONDS_PER_MINUTE} min · Run limit {schedule.duration / SECONDS_PER_MINUTE} min</p>{schedule.next_at && <p className="small">Next: {new Date(schedule.next_at).toLocaleString()}</p>}{schedule.error && <p className="error">{schedule.error}</p>}
      <div className="actions"><button disabled={!!busy} onClick={() => void act(schedule, schedule.enabled ? "pause" : "enable")}>{schedule.enabled ? "Pause future runs" : "Enable"}</button><button disabled={!!busy} onClick={() => void act(schedule, "run_now")}>Run now</button><button disabled={!!busy} onClick={() => setEditing(schedule)}>Edit</button>{active?.schedule === schedule.id && <button disabled={!!busy} onClick={() => void act(schedule, "stop")}>Stop active run</button>}</div></div></li>)}</ul>
    <h3>Occurrence history</h3><ul className="delivery-task-list">{history.map((occurrence) => <li key={occurrence.id}><div><strong>{page?.schedules.find((s) => s.id === occurrence.schedule)?.name ?? "Recurring task"}</strong><p>{occurrence.phase} · {new Date(occurrence.due_at).toLocaleString()}</p>{occurrence.reason && <p className="small muted">{occurrence.reason}</p>}{page?.active === occurrence.id && ["unconfirmed", "interrupted"].includes(occurrence.phase) && <ConfirmStopped disabled={!!busy} confirm={() => {const schedule=page?.schedules.find((s)=>s.id===occurrence.schedule);if(schedule)void act(schedule,"confirm_stopped");}} />}</div>{occurrence.task && <button onClick={() => void open(occurrence.task!, "Recurring task")}>Open task</button>}</li>)}</ul>
    {next && <button onClick={()=>void more()}>More occurrence history</button>}{error && <p className="error" role="alert">{error}</p>}
  </section>;
}
function ScheduleForm({ ep, schedule, done }: { ep: Endpoint; schedule?: Schedule; done: () => void }) {
  const [name, setName] = useState(schedule?.name ?? ""), [prompt, setPrompt] = useState(schedule?.prompt ?? ""), [provider, setProvider] = useState<Agent>(schedule?.provider ?? "codex"), [base, setBase] = useState(schedule?.base ?? "main");
  const [interval, setInterval] = useState(schedule ? schedule.interval / SECONDS_PER_MINUTE : DEFAULT_INTERVAL_MINUTES), [duration, setDuration] = useState(schedule ? schedule.duration / SECONDS_PER_MINUTE : DEFAULT_DURATION_MINUTES), [error, setError] = useState(""), [busy, setBusy] = useState(false);
  async function save() { setBusy(true); try { await post(ep, "/api/delivery/schedules", { id: schedule?.id ?? null, revision: schedule?.revision ?? null, name, prompt, provider, base, interval: interval * SECONDS_PER_MINUTE, duration: duration * SECONDS_PER_MINUTE }); done(); } catch (error) { setError(String(error)); } finally { setBusy(false); } }
  return <form className="delivery-schedule-form" onSubmit={(event) => { event.preventDefault(); void save(); }}><label>Name<input className="field" required value={name} onChange={(event) => setName(event.target.value)} /></label><label>Saved instruction<textarea className="field" required value={prompt} onChange={(event) => setPrompt(event.target.value)} /></label>
    <div className="delivery-fields"><label>Agent<select value={provider} onChange={(event) => setProvider(event.target.value as Agent)}>{Object.entries(AGENT_NAME).map(([id, name]) => <option key={id} value={id}>{name}</option>)}</select></label><label>Local base branch<input className="field" value={base} onChange={(event) => setBase(event.target.value)} /></label><label>Interval in minutes<input className="field" type="number" min="1" value={interval} onChange={(event) => setInterval(Number(event.target.value))} /></label><label>Run limit in minutes<input className="field" type="number" min="1" max="1440" value={duration} onChange={(event) => setDuration(Number(event.target.value))} /></label></div>
    <p className="small muted">Saves paused. Enable it after reviewing the instruction. Editing pauses future runs and leaves an active run unchanged.</p><div className="actions"><button type="submit" className="primary" disabled={busy}>Save paused</button><button type="button" onClick={done}>Cancel</button></div>{error && <p role="alert">{error}</p>}
  </form>;
}

function ConfirmStopped({disabled,confirm}:{disabled:boolean;confirm:()=>void}) {
  const [open,setOpen]=useState(false);
  return open?<div><p className="small">Confirm you inspected the original task and its processes are stopped. This releases its scheduled project slot.</p><div className="actions"><button disabled={disabled} onClick={confirm}>Confirm stopped and release slot</button><button onClick={()=>setOpen(false)}>Cancel</button></div></div>:<button disabled={disabled} onClick={()=>setOpen(true)}>Confirm original run stopped</button>;
}
