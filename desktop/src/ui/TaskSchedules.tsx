// Recurrence stays local, explicitly enabled and separate from stopping its active occurrence.
import { useState } from "react";
import { get, post, type Endpoint } from "../api";
import { AGENT_NAME, type Agent } from "../store";
import { deliveryChanged, requestId, forgetRequest, type Occurrence, type Schedule, type SchedulePage } from "../delivery";
import { Floating } from "./Menu";
import { Icon } from "./icons";

const SECONDS_PER_MINUTE = 60, DEFAULT_INTERVAL_MINUTES = 1440, DEFAULT_DURATION_MINUTES = 30;
const CONTROLLER_REVISION = 0;
/// The units an interval is written in, largest first so 1440 minutes reads as "1 day".
const UNITS = [{ id: "days", minutes: 1440 }, { id: "hours", minutes: 60 }, { id: "minutes", minutes: 1 }] as const;
type Unit = typeof UNITS[number]["id"];

/// "1 day", "6 hours", "45 minutes": an interval in the largest unit that divides it.
export function every(seconds: number): string {
  const minutes = Math.round(seconds / SECONDS_PER_MINUTE);
  const unit = UNITS.find((u) => minutes % u.minutes === 0) ?? UNITS[2];
  const n = minutes / unit.minutes;
  return `${n} ${n === 1 ? unit.id.slice(0, -1) : unit.id}`;
}

type Act = (schedule: Pick<Schedule, "id" | "revision">, action: string) => Promise<void>;

/** Definitions are saved off and every occurrence remains available as a normal task. */
export function RecurringTasks({ ep, page, open }: { ep: Endpoint; page?: SchedulePage; open: (task: string, title: string) => Promise<void> }) {
  const [editing, setEditing] = useState<Schedule | "new">(), [error, setError] = useState(""), [busy, setBusy] = useState("");
  const [extra, setExtra] = useState<SchedulePage>();
  const active = page?.active_occurrence ?? page?.occurrences.find((occurrence) => occurrence.id === page.active);
  const history = [...(active ? [active] : []), ...new Map([...(page?.occurrences ?? []), ...(extra?.occurrences ?? [])].filter((occurrence) => occurrence.id !== active?.id).map((occurrence) => [occurrence.id, occurrence])).values()];
  const next = extra ? extra.next : page?.next;
  async function more() { try { const history = await get<SchedulePage>(ep, "/api/delivery/schedules", { after: next }); setExtra({ ...history, occurrences: [...(extra?.occurrences ?? []), ...history.occurrences] }); } catch (error) { setError(String(error)); } }
  const act: Act = async (schedule, action) => {
    setBusy(schedule.id); setError("");
    const receipt = action === "run_now" ? requestId(`schedule:${ep.port}:${schedule.id}`, { revision: schedule.revision }) : null;
    try { await post(ep, `/api/delivery/schedules/${schedule.id}`, { action, revision: schedule.revision, receipt }); if (receipt) forgetRequest(receipt); deliveryChanged(ep); }
    catch (error) { setError(String(error)); } finally { setBusy(""); }
  };
  if (editing) return <ScheduleForm key={typeof editing === "string" ? editing : editing.id} ep={ep} schedule={editing === "new" ? undefined : editing} done={() => { setEditing(undefined); deliveryChanged(ep); }} />;
  const schedules = page?.schedules ?? [];
  return <section aria-label="Recurring tasks" className="recurring">
    <div className="recurring-head"><p>A saved instruction, run on a schedule while Oya Factory is open.</p><button className="recurring-new" onClick={() => setEditing("new")}><Icon name="plus" size={13} />New</button></div>
    {page?.controller_error && <div className="recurring-halt" role="alert"><p><strong>Recurring dispatch paused:</strong> {page.controller_error}</p><p className="small muted">New runs wait; an active run can still be stopped.</p><button disabled={!!busy} onClick={() => void act({ id: "dispatcher", revision: CONTROLLER_REVISION }, "retry")}>Retry recurring dispatcher</button></div>}
    {schedules.length === 0 && page && <p className="recurring-empty">Nothing scheduled yet.</p>}
    <ul className="task-list">{schedules.map((schedule) => <ScheduleRow key={schedule.id} schedule={schedule} halted={!!page?.controller_error} running={active?.schedule === schedule.id} busy={!!busy} act={act} edit={() => setEditing(schedule)} />)}</ul>
    {history.length > 0 && <details className="delivery-section recurring-history"><summary>History <span className="tab-count">{history.length}</span></summary>
      <ul className="task-list">{history.map((occurrence) => <OccurrenceRow key={occurrence.id} occurrence={occurrence} page={page} busy={!!busy} act={act} open={open} />)}</ul>
      {next && <button className="link" onClick={() => void more()}>More occurrence history</button>}
    </details>}
    {error && <p className="error" role="alert">{error}</p>}
  </section>;
}

/// One schedule: its name and cadence, On/Off, and the rest under ⋯. Stopping an active run stays
/// on the row — it is the one control that must never be a menu away.
function ScheduleRow({ schedule, halted, running, busy, act, edit }: { schedule: Schedule; halted: boolean; running: boolean; busy: boolean; act: Act; edit: () => void }) {
  const [menu, setMenu] = useState<DOMRect | null>(null);
  const state = schedule.enabled ? halted ? "Dispatch paused" : "On" : "Off";
  const meta = [`Every ${every(schedule.interval)}`, AGENT_NAME[schedule.provider] ?? schedule.provider, schedule.next_at && schedule.enabled ? `next ${new Date(schedule.next_at).toLocaleString([], { weekday: "short", hour: "numeric", minute: "2-digit" })}` : ""].filter(Boolean).join(" · ");
  return <li className="schedule-row">
    <span className="task-main"><span className="task-title">{schedule.name}</span><span className="task-meta">{meta}</span>{schedule.error && <span className="error small">{schedule.error}</span>}</span>
    {running && <button disabled={busy} onClick={() => void act(schedule, "stop")}>Stop active run</button>}
    <button className={`schedule-toggle${schedule.enabled ? " on" : ""}`} role="switch" aria-checked={schedule.enabled} aria-label={`${schedule.name}: ${state}`} disabled={busy} onClick={() => void act(schedule, schedule.enabled ? "pause" : "enable")}>{state}</button>
    <button className="ghost" aria-label={`More for ${schedule.name}`} aria-haspopup="menu" onClick={(e) => setMenu(e.currentTarget.getBoundingClientRect())}><Icon name="more" size={15} /></button>
    {menu && <Floating anchor={menu} onClose={() => setMenu(null)}>
      <button disabled={busy} onClick={() => { setMenu(null); void act(schedule, "run_now"); }}>Run now</button>
      <button onClick={() => { setMenu(null); edit(); }}>Edit…</button>
    </Floating>}
  </li>;
}

function OccurrenceRow({ occurrence, page, busy, act, open }: { occurrence: Occurrence; page?: SchedulePage; busy: boolean; act: Act; open: (task: string, title: string) => Promise<void> }) {
  const schedule = page?.schedules.find((s) => s.id === occurrence.schedule);
  const recover = page?.active === occurrence.id && ["unconfirmed", "interrupted"].includes(occurrence.phase);
  return <li className="schedule-row">
    <span className="task-main"><span className="task-title">{schedule?.name ?? "Recurring task"}</span><span className="task-meta">{occurrence.phase} · {new Date(occurrence.due_at).toLocaleString()}</span>{occurrence.reason && <span className="task-meta">{occurrence.reason}</span>}
      {recover && <ConfirmStopped disabled={busy} confirm={() => { if (schedule) void act(schedule, "confirm_stopped"); }} />}</span>
    {occurrence.task && <button className="link" onClick={() => void open(occurrence.task!, "Recurring task")}>Open</button>}
  </li>;
}

function ScheduleForm({ ep, schedule, done }: { ep: Endpoint; schedule?: Schedule; done: () => void }) {
  const initial = schedule ? schedule.interval / SECONDS_PER_MINUTE : DEFAULT_INTERVAL_MINUTES;
  const firstUnit = UNITS.find((u) => initial % u.minutes === 0) ?? UNITS[2];
  const [name, setName] = useState(schedule?.name ?? ""), [prompt, setPrompt] = useState(schedule?.prompt ?? ""), [provider, setProvider] = useState<Agent>(schedule?.provider ?? "codex"), [base, setBase] = useState(schedule?.base ?? "main");
  const [count, setCount] = useState(initial / firstUnit.minutes), [unit, setUnit] = useState<Unit>(firstUnit.id);
  const [duration, setDuration] = useState(schedule ? schedule.duration / SECONDS_PER_MINUTE : DEFAULT_DURATION_MINUTES), [error, setError] = useState(""), [busy, setBusy] = useState(false);
  const minutes = count * (UNITS.find((u) => u.id === unit)?.minutes ?? 1);
  async function save() { setBusy(true); try { await post(ep, "/api/delivery/schedules", { id: schedule?.id ?? null, revision: schedule?.revision ?? null, name, prompt, provider, base, interval: minutes * SECONDS_PER_MINUTE, duration: duration * SECONDS_PER_MINUTE }); done(); } catch (error) { setError(String(error)); } finally { setBusy(false); } }
  return <form className="schedule-form" onSubmit={(event) => { event.preventDefault(); void save(); }}>
    <h3>{schedule ? "Edit recurring task" : "New recurring task"}</h3>
    <label>Name<input className="field" required autoFocus value={name} onChange={(event) => setName(event.target.value)} placeholder="Nightly dependency check" /></label>
    <label>Instruction<textarea className="field" required value={prompt} onChange={(event) => setPrompt(event.target.value)} placeholder="What the agent should do each time" /></label>
    <div className="schedule-grid">
      <label>Agent<select className="field" value={provider} onChange={(event) => setProvider(event.target.value as Agent)}>{Object.entries(AGENT_NAME).map(([id, name]) => <option key={id} value={id}>{name}</option>)}</select></label>
      <label>Base branch<input className="field" value={base} onChange={(event) => setBase(event.target.value)} /></label>
      <label>Every<span className="schedule-every"><input className="field" type="number" min="1" value={count} onChange={(event) => setCount(Number(event.target.value))} aria-label="Interval" /><select className="field" value={unit} onChange={(event) => setUnit(event.target.value as Unit)} aria-label="Interval unit">{UNITS.map((u) => <option key={u.id} value={u.id}>{u.id}</option>)}</select></span></label>
      <label>Stop after (minutes)<input className="field" type="number" min="1" max="1440" value={duration} onChange={(event) => setDuration(Number(event.target.value))} /></label>
    </div>
    {error && <p className="error" role="alert">{error}</p>}
    <div className="schedule-foot"><p className="small muted">Saved off — turn it on when you're ready.</p><button type="button" className="link" onClick={done}>Cancel</button><button type="submit" className="primary" disabled={busy}>{busy ? "Saving…" : "Save"}</button></div>
  </form>;
}

function ConfirmStopped({ disabled, confirm }: { disabled: boolean; confirm: () => void }) {
  const [open, setOpen] = useState(false);
  return open ? <span className="task-meta">Confirm you inspected the original task and its processes are stopped. This releases its scheduled slot. <button className="link" disabled={disabled} onClick={confirm}>Confirm stopped and release slot</button><button className="link" onClick={() => setOpen(false)}>Cancel</button></span> : <button className="link" disabled={disabled} onClick={() => setOpen(true)}>Confirm original run stopped</button>;
}
