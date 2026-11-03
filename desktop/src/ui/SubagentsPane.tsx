// Read-only child progress uses lifecycle evidence without changing parent terminal ownership.
import { useRef } from "react";
import { useStore } from "../store";
import { terminalLease } from "../typers";
import { childrenOf, childActive, childScope, patchPane, useChildPane, useCompactChildren, useChildReading, SHORT_ID, type Invocation, type ChildOutput } from "../subagents";
import { useChildOutput } from "../subagent-output";
import { MemberAvatar } from "./MemberAvatar";
import { Icon } from "./icons";

const STATUS = { working: "Working", waiting: "Waiting", done: "Done", failed: "Failed", cancelled: "Cancelled", interrupted: "Interrupted", unknown: "Status unknown" };
const shortId = (child: Invocation) => `${child.id.split(":").at(-1)?.slice(0, SHORT_ID) ?? ""}${child.id.startsWith("antigravity:") ? ` · ${child.child?.slice(0, SHORT_ID)}` : ""}`;
const nameOf = (child: Invocation) => child.member || `Subagent · ${shortId(child)}`;
const statusOf = (child: Invocation, connected: boolean) => !connected && childActive(child) ? "unknown" : child.status;

/** Discovery is passive; expansion never spawns, submits, cancels, or focuses a provider. */
export function SubagentsPane({ lane, hidden }: { lane: string; hidden: boolean }) {
  const current = useStore((s) => s.lanes[lane]), ep = useStore((s) => current ? s.projects[current.project]?.endpoint : undefined);
  const snapshot = current && childrenOf(current), scope = childScope(current), view = useChildPane(scope, snapshot);
  const { ref, compact } = useCompactChildren(), header = useRef<HTMLButtonElement>(null);
  const expanded = view.open && (!compact || view.deliberate), selected = snapshot?.invocations.find((child) => child.id === view.selected);
  useChildOutput(lane, scope, selected?.id, ep, !hidden && expanded && !!selected);
  if (!snapshot?.invocations.length) return <section ref={ref} className="subagent-anchor" hidden />;
  const working = snapshot.connected ? snapshot.invocations.filter(childActive).length : 0;
  const done = snapshot.invocations.filter((child) => !childActive(child) && child.status !== "unknown").length;
  const toggle = () => { const focus = ref.current?.contains(document.activeElement); patchPane(scope, { open: !expanded, deliberate: true }); if (expanded && focus) header.current?.focus(); };
  return <section ref={ref} className={`subagent-pane${expanded ? " expanded" : ""}`} aria-label="Subagents" data-reference-target="" onKeyDown={(event) => {
    if (event.key === "Escape") { event.preventDefault(); event.stopPropagation(); terminalLease(lane)?.focus(); }
  }}>
    <button ref={header} className="subagent-pane-toggle" aria-expanded={expanded} onClick={toggle}>
      <Icon name={expanded ? "chevron-down" : "chevron-right"} size={12} /><strong>Subagents</strong><span aria-live="polite">{working} working{done ? ` · ${done} finished` : ""}</span><span className="spacer" /><small>Read only</small>
    </button>
    {expanded && <div className="subagent-pane-body"><div className="subagent-list" aria-label="Subagent invocations">
      {!snapshot.connected && <p className="small muted" role="status">Reconnecting to subagent activity…</p>}
      {snapshot.truncated && <p className="small muted">Showing recent subagents.</p>}
      {snapshot.invocations.map((child) => <ChildRow key={child.id} child={child} connected={snapshot.connected} selected={child.id === view.selected} select={() => patchPane(scope, { selected: child.id })} />)}
    </div>{selected && <ChildProgress child={selected} scope={scope} connected={snapshot.connected} output={view.outputs[selected.id]} />}</div>}
  </section>;
}
function ChildRow({ child, connected, selected, select }: { child: Invocation; connected: boolean; selected: boolean; select: () => void }) {
  const status = statusOf(child, connected), working = status === "working" || status === "waiting";
  return <button className={`subagent-row${selected ? " selected" : ""}`} aria-pressed={selected} aria-label={`${nameOf(child)}, ${STATUS[status]}, invocation ${child.id}`} title={child.id} onClick={select}>
    <MemberAvatar name={child.member ?? "subagent"} working={working} /><span><strong>{nameOf(child)}</strong><small>{STATUS[status]} · {shortId(child)}</small>{child.activity && <small className="muted">{child.activity}</small>}</span>
  </button>;
}
function ChildProgress({ child, scope, connected, output }: { child: Invocation; scope: string; connected: boolean; output?: ChildOutput }) {
  const { ref, scroll, latest } = useChildReading(scope, child.id, output), status = statusOf(child, connected), detail = output?.detail;
  const availability = detail?.availability ?? child.output;
  const empty = availability === "unavailable" ? "This provider reports activity but does not expose this subagent’s output." : "Waiting for output…";
  return <div className="subagent-progress"><header><strong>{nameOf(child)}</strong><span className="small muted">{STATUS[status]} · {shortId(child)}</span></header>
    {output?.disconnected && <p className="small muted" role="status">Output disconnected · reconnecting…</p>}
    {status === "interrupted" && <p className="small muted">The parent session ended before completion was reported.</p>}
    {detail?.truncated && <p className="small muted">Showing recent output.</p>}
    <div ref={ref} className="subagent-output" tabIndex={0} aria-label={`${nameOf(child)} output`} onScroll={scroll}>
      {!detail?.items.length ? <p className="small muted">{empty}</p> : detail.items.map((item, index) => <pre key={index} className={`subagent-output-${item.kind === "tool" ? "tool" : "text"}`}>{item.text}</pre>)}
    </div>{output?.following === false && <button className="subagent-latest small" onClick={latest}>Jump to latest</button>}
  </div>;
}
