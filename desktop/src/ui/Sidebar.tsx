import { revealItemInDir } from "@tauri-apps/plugin-opener";
import { useMemo, useState } from "react";
import { open } from "@tauri-apps/plugin-dialog";
import { useStore, type Agent, type Session } from "../store";
import { Floating } from "./Menu";
import { LaneViewSwitch } from "./LaneViewSwitch";
import { SetupBadge } from "./SetupWarnings";
import { Icon } from "./icons";
import { label } from "../keys";
import { cycle } from "./App";
import { activity } from "./StatusBar";
import type { Lane } from "../store";
import type { Turn } from "../reduce";

// The last line of a finished turn, worked out once per turn object: `reduce.ts` copies a turn
// whenever it changes, so the object is the cache key, and a row reading it on every store
// update — every 16 ms while another lane streams — costs a lookup rather than a split.
const said = new WeakMap<Turn, string>();
function lastLine(t: Turn): string {
  const hit = said.get(t);
  if (hit !== undefined) return hit;
  let out = "";
  for (let i = t.steps.length - 1; i >= 0 && !out; i--) {
    const b = t.blocks[t.steps[i].slice(2)];
    if (b?.kind !== "say") continue;
    out = (b.text.split("\n").map((x) => x.replace(/[#*`>_]/g, "").trim()).filter(Boolean).at(-1) ?? "").slice(0, 120);
  }
  said.set(t, out);
  return out;
}

/// The line under a lane's name: what it is doing while it works, and the last thing it said once
/// it stops — so the rail answers "where is each of them" without opening any of them.
function lastWord(l: Lane): { text: string; tone: string } {
  const a = activity(l);
  if (a.tone !== "idle") return { text: a.label, tone: a.tone };
  const t = l.conv.turns.at(-1);
  if (t?.gate?.status === "failed") return { text: "Checks failed", tone: "bad" };
  return { text: t ? lastLine(t) : "", tone: "idle" };
}

export async function pickProject() {
  const path = await open({ directory: true, multiple: false, title: "Open a project" });
  if (typeof path === "string") await useStore.getState().openProject(path);
}

export function Sidebar() {
  const order = useStore((s) => s.order);
  return (
    <nav className="sidebar" aria-label="Projects">
      <div className="sidebar-actions">
        <button className="open" onClick={pickProject} title={`Open a project folder (${label({ key: "o" })})`}>
          <Icon name="folder" size={17} /> Open project…
        </button>
        <button className="ghost" onClick={() => useStore.setState({ creating: true, settings: false })} aria-label="New project" title="New project from a template, or clone from GitHub">
          <Icon name="plus" size={17} />
        </button>
        <button className="ghost" onClick={() => useStore.setState((s) => ({ extensions: !s.extensions, settings: false, creating: false }))} aria-label="Extensions" title="Extensions — skills, subagents, MCP servers, plugins">
          <Icon name="puzzle" size={17} />
        </button>
        <button className="ghost" onClick={() => useStore.setState((s) => ({ settings: !s.settings }))} aria-label="Settings" title={`Settings (${label({ key: "," })})`}>
          <Icon name="gear" size={17} />
        </button>
      </div>
      <div className="sidebar-head eyebrow">Projects</div>
      <div className="sidebar-list">
        {order.map((p) => (
          <ProjectRow key={p} path={p} />
        ))}
        {order.length === 0 && <div className="small faint" style={{ padding: "4px 8px" }}>No projects open.</div>}
      </div>
    </nav>
  );
}

function ProjectRow({ path }: { path: string }) {
  const p = useStore((s) => s.projects[path]);
  const needs = useStore((s) => (s.projects[path]?.lanes ?? []).filter((l) => (s.lanes[l]?.pending.length ?? 0) > 0).length);
  const busy = useStore((s) => (s.projects[path]?.lanes ?? []).filter((l) => s.lanes[l]?.running).length);
  const [menu, setMenu] = useState<{ at: DOMRect; kind: "new" | "more" } | null>(null);
  const [ctx, setCtx] = useState<DOMRect | null>(null);
  if (!p) return null;
  const { newLane, select, closeProject } = useStore.getState();
  const toggle = () => useStore.setState((s) => (s.projects[path] ? { projects: { ...s.projects, [path]: { ...s.projects[path], collapsed: !s.projects[path].collapsed } } } : s));
  const make = (isolated: boolean, agent: Agent) => {
    setMenu(null);
    select(newLane(path, isolated, agent));
  };
  return (
    <section className="project">
      <div className="project-head" title={path} onContextMenu={(e) => (e.preventDefault(), setCtx(pointer(e)))}>
        <button className="ghost" onClick={toggle} aria-expanded={!p.collapsed} aria-label={`${p.collapsed ? "Expand" : "Collapse"} ${p.name}`}>
          <Icon name={p.collapsed ? "chevron-right" : "chevron-down"} size={12} />
        </button>
        <span className="project-name">{p.name}</span>
        <SetupBadge project={path} />
        {needs > 0 && <span className="small" style={{ color: "var(--warn)" }}>· {needs} needs you</span>}
        {busy > 0 && (
          <span className="busy-label" title={`${busy} lane${busy === 1 ? "" : "s"} running a turn`}>
            <span className="busy-dot" /> {busy} running
          </span>
        )}
        {p.starting && <span className="small faint">starting…</span>}
        <button className="ghost" onClick={(e) => setMenu({ at: e.currentTarget.getBoundingClientRect(), kind: "new" })} aria-label={`New lane in ${p.name}`} title="New lane — Claude Code or Codex">
          <Icon name="plus" size={14} />
        </button>
        <button className="ghost hover-only" onClick={(e) => setMenu({ at: e.currentTarget.getBoundingClientRect(), kind: "more" })} aria-label={`${p.name} actions`}>
          <Icon name="more" size={14} />
        </button>
      </div>
      {p.error && (
        <div className="small error" style={{ padding: "0 8px 4px 28px" }}>
          {p.error}{" "}
          <button className="link small" onClick={() => void useStore.getState().ensure(path).catch(() => undefined)}>
            Retry
          </button>
        </div>
      )}
      {menu?.kind === "new" && (
        <Floating anchor={menu.at} onClose={() => setMenu(null)} width={260}>
          <div className="eyebrow" style={{ padding: "4px 8px" }}>Claude Code</div>
          <button onClick={() => make(false, "claude")}>
            <Icon name="terminal" size={14} /> In the project's tree <kbd style={{ marginLeft: "auto" }}>{label({ key: "n" })}</kbd>
          </button>
          <button onClick={() => make(true, "claude")}>
            <Icon name="branch" size={14} /> On its own branch <kbd style={{ marginLeft: "auto" }}>{label({ key: "n", shift: true })}</kbd>
          </button>
          <div className="sep" />
          <div className="eyebrow" style={{ padding: "4px 8px" }}>Codex</div>
          <button onClick={() => make(false, "codex")}>
            <Icon name="terminal" size={14} /> In the project's tree
          </button>
          <button onClick={() => make(true, "codex")}>
            <Icon name="branch" size={14} /> On its own branch
          </button>
        </Floating>
      )}
      {menu?.kind === "more" && (
        <Floating anchor={menu.at} onClose={() => setMenu(null)}>
          <button onClick={() => (setMenu(null), void closeProject(path))}>Close project (its daemon stops; branches stay)</button>
        </Floating>
      )}
      {ctx && (
        <Floating anchor={ctx} onClose={() => setCtx(null)} width={250}>
          <button onClick={() => (setCtx(null), make(false, "claude"))}>
            <Icon name="terminal" size={14} /> New Claude Code lane <kbd style={{ marginLeft: "auto" }}>{label({ key: "n" })}</kbd>
          </button>
          <button onClick={() => (setCtx(null), make(true, "claude"))}>
            <Icon name="branch" size={14} /> New lane on its own branch <kbd style={{ marginLeft: "auto" }}>{label({ key: "n", shift: true })}</kbd>
          </button>
          <button onClick={() => (setCtx(null), make(false, "codex"))}>
            <Icon name="terminal" size={14} /> New Codex lane
          </button>
          <div className="sep" />
          <button onClick={() => (setCtx(null), void revealItemInDir(path))}>
            <Icon name="folder" size={14} /> {REVEAL}
          </button>
          <button onClick={() => (setCtx(null), void navigator.clipboard.writeText(path))}>
            <Icon name="copy" size={14} /> Copy path
          </button>
          <button onClick={() => (setCtx(null), toggle())}>
            <Icon name={p.collapsed ? "chevron-down" : "chevron-right"} size={14} /> {p.collapsed ? "Expand" : "Collapse"}
          </button>
          <div className="sep" />
          <button className="danger" onClick={() => (setCtx(null), void closeProject(path))}>
            <Icon name="x" size={14} /> Close project
          </button>
        </Floating>
      )}
      {!p.collapsed && p.lanes.map((l) => <LaneRow key={l} id={l} />)}
      {!p.collapsed && <History path={path} sessions={p.sessions} />}
    </section>
  );
}

/// ↑↓ on a focused lane moves to the next one and keeps focus in the list, so the rail can be
/// walked without the mouse.
function arrows(e: React.KeyboardEvent) {
  const by = e.key === "ArrowDown" ? 1 : e.key === "ArrowUp" ? -1 : 0;
  if (!by || e.metaKey || e.ctrlKey || e.altKey || e.shiftKey) return;
  e.preventDefault();
  cycle(by);
  requestAnimationFrame(() => (document.querySelector('.sidebar .lane[aria-current="true"]') as HTMLElement | null)?.focus());
}

/// The editor is in the lane's header, so the lane is selected first.
function startRename(id: string) {
  useStore.getState().select(id);
  useStore.setState({ renaming: id });
}

function LaneRow({ id }: { id: string }) {
  const l = useStore((s) => s.lanes[id]);
  const active = useStore((s) => s.active === id);
  const failed = useStore((s) => s.lanes[id]?.conv.turns.at(-1)?.gate?.status === "failed");
  // Two strings, so a row re-renders when its own line changes and not on every op.
  const sub = useStore((s) => {
    const w = s.lanes[id] ? lastWord(s.lanes[id]) : { text: "", tone: "idle" };
    return `${w.tone}|${w.text}`;
  });
  const subTone = sub.slice(0, sub.indexOf("|"));
  const subText = sub.slice(sub.indexOf("|") + 1);
  const [ctx, setCtx] = useState<DOMRect | null>(null);
  const root = useStore((s) => (l ? s.projects[l.project]?.path : undefined));
  if (!l) return null;
  const asking = l.pending.length;
  const { select } = useStore.getState();
  // Merge, discard and close confirm for the active lane, so the menu selects this one first.
  const ask = (what: "merge" | "discard" | "close") => {
    setCtx(null);
    select(id);
    useStore.setState({ asking: what });
  };
  const checkout = root && (l.wt ? `${root}/.keel/worktrees/${l.wt}` : root);
  return (
    <div className="lane-row">
    <button className={`lane ${active ? "active" : ""}`} onClick={() => select(id)} onDoubleClick={() => startRename(id)} onKeyDown={(e) => (e.key === "F2" ? (e.preventDefault(), startRename(id)) : arrows(e))} onContextMenu={(e) => (e.preventDefault(), setCtx(pointer(e)))} aria-current={active} title={`${l.wt ? `keel/${l.wt}` : "Shares the project's working tree"} — double-click or F2 to rename; ↑↓ or ${label({ key: "ArrowUp", alt: true })}/${label({ key: "ArrowDown", alt: true })} to move between lanes`}>
      <span className="marker">
        {asking ? <Icon name="hand" size={11} style={{ color: "var(--warn)" }} /> : failed ? <Icon name="x-circle" size={11} style={{ color: "var(--del)" }} /> : l.running ? <span className="busy-dot" /> : null}
      </span>
      <span className="lane-text">
        <span className="lane-title">{l.title}</span>
        {subText && <span className={`lane-sub ${subTone}`}>{subText}</span>}
      </span>
      {l.isolated && <Icon name="branch" size={11} style={{ color: "var(--faint)" }} />}
      {l.agent === "codex" && <span className="faint" style={{ fontSize: 10 }}>codex</span>}
      {asking > 0 && <span className="count-pill">{asking}</span>}
    </button>
    <button className="ghost lane-close" aria-label={`Close ${l.title}`} title={`Close lane (${label({ key: "w" })})`} onClick={() => ask("close")}>
      <Icon name="x" size={12} />
    </button>
    {ctx && (
      <Floating anchor={ctx} onClose={() => setCtx(null)} width={250}>
        <LaneViewSwitch lane={id} menu onSwitch={() => { setCtx(null); select(id); }} />
        <div className="sep" />
        <button onClick={() => (setCtx(null), startRename(id))}>
          <Icon name="pencil" size={14} /> Rename… <kbd style={{ marginLeft: "auto" }}>F2</kbd>
        </button>
        {checkout && (
          <button onClick={() => (setCtx(null), void revealItemInDir(checkout))}>
            <Icon name="folder" size={14} /> {REVEAL}
          </button>
        )}
        {l.wt && (
          <button onClick={() => (setCtx(null), void navigator.clipboard.writeText(`keel/${l.wt}`))}>
            <Icon name="branch" size={14} /> Copy branch name
          </button>
        )}
        <button onClick={() => (setCtx(null), void navigator.clipboard.writeText(`${l.agent === "codex" ? "codex resume" : "claude --resume"} ${l.session}`))}>
          <Icon name="copy" size={14} /> Copy resume command
        </button>
        <div className="sep" />
        {l.wt && <button onClick={() => ask("merge")}>Merge lane…</button>}
        {l.wt && (
          <button className="danger" onClick={() => ask("discard")}>
            Discard lane…
          </button>
        )}
        <button onClick={() => ask("close")}>
          <Icon name="x" size={14} /> Close lane <kbd style={{ marginLeft: "auto" }}>{label({ key: "w" })}</kbd>
        </button>
      </Floating>
    )}
    </div>
  );
}

/// Conversations in this project, newest first — titles only — that are not already open as a
/// lane. Opening one makes it a lane that resumes it.
function History({ path, sessions }: { path: string; sessions?: Session[] }) {
  const resume = useStore((s) => s.resume);
  // A string, so the selector's answer compares equal when nothing changed.
  const openKey = useStore((s) => (s.projects[path]?.lanes ?? []).map((l) => s.lanes[l]?.session ?? "").join(","));
  const openIds = useMemo(() => new Set(openKey.split(",")), [openKey]);
  const [open, setOpen] = useState(false);
  const [all, setAll] = useState(false);
  const [ctx, setCtx] = useState<{ at: DOMRect; session: Session } | null>(null);
  if (!sessions) return null;
  const list = [...sessions].filter((s) => !openIds.has(s.id)).sort((a, b) => (b.last_active ?? "").localeCompare(a.last_active ?? ""));
  if (!list.length) return null;
  const shown = all ? list : list.slice(0, 5);
  return (
    <div className="history">
      <button className="lane history-head" style={{ paddingLeft: 20 }} onClick={() => setOpen(!open)} aria-expanded={open}>
        <span className="eyebrow">History · {list.length}</span>
        <Icon name={open ? "chevron-down" : "chevron-right"} size={11} style={{ color: "var(--faint)" }} />
      </button>
      {open &&
        shown.map((s) => (
          <button key={s.id} className="lane" onClick={() => resume(path, s)} onContextMenu={(e) => (e.preventDefault(), setCtx({ at: pointer(e), session: s }))} title={`Resume — ${s.messages} messages${s.branch ? ` · ${s.branch}` : ""}`}>
            <span className="marker">{s.busy && <span className="busy-dot" />}</span>
            <span className="lane-title faint">{s.title || "Untitled session"}</span>
            <span className="mono faint" style={{ fontSize: 10 }}>
              {ago(s.last_active)}
            </span>
          </button>
        ))}
      {ctx && (
        <Floating anchor={ctx.at} onClose={() => setCtx(null)} width={250}>
          <button onClick={() => (setCtx(null), resume(path, ctx.session))}>
            <Icon name="terminal" size={14} /> Resume in a lane
          </button>
          <button onClick={() => (setCtx(null), void navigator.clipboard.writeText(`claude --resume ${ctx.session.id}`))}>
            <Icon name="copy" size={14} /> Copy resume command
          </button>
        </Floating>
      )}
      {open && list.length > shown.length && (
        <button className="lane faint small" style={{ paddingLeft: 28 }} onClick={() => setAll(true)}>
          Show all {list.length}
        </button>
      )}
    </div>
  );
}

const REVEAL = navigator.userAgent.includes("Mac") ? "Reveal in Finder" : "Show in Explorer";

/// Where a right-click happened, as the zero-size anchor `Floating` opens at.
function pointer(e: { clientX: number; clientY: number }): DOMRect {
  return new DOMRect(e.clientX, e.clientY, 0, 0);
}

function ago(stamp?: string | null): string {
  if (!stamp) return "";
  const s = (Date.now() - Date.parse(stamp)) / 1000;
  if (!Number.isFinite(s)) return "";
  if (s < 60) return "now";
  if (s < 3600) return `${Math.floor(s / 60)}m`;
  if (s < 86400) return `${Math.floor(s / 3600)}h`;
  return `${Math.floor(s / 86400)}d`;
}
