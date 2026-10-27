// Projects, lanes, and team members share the same observed activity indicators.
import { revealItemInDir } from "@tauri-apps/plugin-opener";
import { useEffect, useMemo, useState, type ReactNode } from "react";
import { open } from "@tauri-apps/plugin-dialog";
import { resumeCommand, sessionForLane } from "../lane-names";
import { AGENT_NAME, useStore, type Agent, type Session } from "../store";
import { get, type Endpoint } from "../api";
import { Floating } from "./Menu";
import { LaneViewSwitch } from "./LaneViewSwitch";
import { SetupBadge } from "./SetupWarnings";
import { Icon } from "./icons";
import { label } from "../keys";
import { cycle } from "./App";
import { activity, waiting, workingMembers } from "../activity";
import { BrandMark } from "./BrandMark";
import { MemberAvatar } from "./MemberAvatar";
import { Working } from "./Working";
import { Git } from "./Git";
import { useIde, EMPTY_IDE } from "../ide";
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

/// Extra activity detail for the conversation's tooltip, keeping sidebar rows to one line.
function lastWord(l: Lane): string {
  const a = activity(l);
  if (a.tone !== "idle") return a.label;
  const t = l.conv.turns.at(-1);
  if (t?.gate?.status === "failed") return "Checks failed";
  return t ? lastLine(t) : "";
}

export async function pickProject() {
  const path = await open({ directory: true, multiple: false, title: "Open a project" });
  if (typeof path === "string") await useStore.getState().openProject(path);
}

export function Sidebar({ explorer, showCode }: { explorer?: ReactNode; showCode?: () => void }) {
  const order = useStore((s) => s.order);
  const project = useStore((s) => (s.active ? s.lanes[s.active]?.project : undefined) ?? s.order[0]);
  const [conversationsOpen, setConversationsOpen] = useState(true);
  const section = useIde((s) => s.sidebar);
  const setSection = useIde((s) => s.setSidebar);
  const lane = useStore((s) => s.active);
  const preparing = useStore((s) => lane ? !!s.lanes[lane]?.isolated && !s.lanes[lane]?.wt : false);
  const selected = useIde((s) => lane ? s.lanes[lane] ?? EMPTY_IDE : EMPTY_IDE);
  const changed = useStore((s) => lane ? s.lanes[lane]?.git?.changes.length ?? 0 : 0);
  return (
    <nav className="sidebar" aria-label="Workspace">
      <div className="workspace-brand"><span className="brand-symbol"><BrandMark /></span><strong>Keel</strong></div>
      <div className="sidebar-actions">
        <button className="open" onClick={pickProject} title={`Open a project folder (${label({ key: "o" })})`}>
          <Icon name="folder" size={17} /> Open project…
        </button>
        <button className="ghost" onClick={() => useStore.setState({ creating: true, settings: false })} aria-label="New project" title="New project from a template, or clone from GitHub">
          <Icon name="plus" size={17} />
        </button>
      </div>
      {explorer && <div className="ide-sidebar-tabs" role="group" aria-label="Sidebar view">
        <button aria-pressed={section === "explorer"} onClick={() => setSection("explorer")}><Icon name="folder" size={13} />Explorer</button>
        <button title="Source Control" aria-label={`Source Control${changed ? ` (${changed} changed)` : ""}`} aria-pressed={section === "git"} onClick={() => setSection("git")}><Icon name="branch" size={13} />Git{changed > 0 && <small>{changed}</small>}</button>
        <button aria-pressed={section === "conversations"} onClick={() => setSection("conversations")}><Icon name="chat" size={13} />Conversations</button>
      </div>}
      {explorer && <div className="ide-explorer-slot" hidden={section !== "explorer"}>{explorer}</div>}
      {explorer && lane && <div className="ide-git-slot" hidden={section !== "git"}>{preparing ? <p className="ide-note">Open the terminal to prepare this lane’s checkout.</p> : <Git key={lane} lane={lane} onOpenCommit={showCode} selection={{ path: selected.selected, staged: selected.staged }} onOpenDiff={(path, staged) => { useIde.getState().open(lane, path, "diff", true, staged); showCode?.(); }} />}</div>}
      <div className="sidebar-list" style={explorer && section !== "conversations" ? { display: "none" } : undefined}>
        <section className="sidebar-section" aria-label="Conversations">
          <button className="sidebar-section-toggle" onClick={() => setConversationsOpen((open) => !open)} aria-expanded={conversationsOpen} aria-controls="sidebar-conversations">
            <Icon name={conversationsOpen ? "chevron-down" : "chevron-right"} size={12} />Conversations
          </button>
          <div id="sidebar-conversations" hidden={!conversationsOpen}>
            {order.map((p) => <ProjectRow key={p} path={p} />)}
            {order.length === 0 && <p className="sidebar-empty">Open a project to get started.</p>}
          </div>
        </section>
        {project && <Members key={project} project={project} />}
      </div>
      <div className="sidebar-footer">
        <button onClick={() => useStore.setState((s) => ({ extensions: !s.extensions, settings: false, creating: false }))}><Icon name="puzzle" size={16} />Extensions</button>
        <button onClick={() => useStore.setState((s) => ({ settings: !s.settings }))} title={`Settings (${label({ key: "," })})`}><Icon name="gear" size={16} />Settings</button>
      </div>
    </nav>
  );
}

interface Member { name: string; display_name: string; avatar?: string | null }
const MEMBERS_VISIBLE = 10;

function Members({ project }: { project: string }) {
  const [members, setMembers] = useState<Member[]>([]);
  const [loaded, setLoaded] = useState(false);
  const [loadError, setLoadError] = useState(false);
  const [expanded, setExpanded] = useState(true);
  const [showAll, setShowAll] = useState(false);
  const projectName = useStore((s) => s.projects[project]?.name);
  const workingNames = useStore((s) => {
    const names = new Set<string>();
    for (const id of s.projects[project]?.lanes ?? []) {
      const lane = s.lanes[id];
      if (!lane) continue;
      const session = sessionForLane(lane, s.projects[project]?.sessions);
      for (const name of workingMembers(lane, session)) names.add(name);
    }
    return [...names].sort().join("\n");
  });
  const working = new Set(workingNames.split("\n"));
  useEffect(() => {
    let live = true;
    setLoaded(false);
    const refresh = () => void useStore.getState().ensure(project).then((ep: Endpoint) => get<Member[]>(ep, "/api/agents/profiles")).then((list) => {
      if (live) { setMembers(list); setLoaded(true); setLoadError(false); }
    }, () => { if (live) { setMembers([]); setLoaded(true); setLoadError(true); } });
    window.addEventListener("keel:subagents-changed", refresh);
    refresh();
    return () => { live = false; window.removeEventListener("keel:subagents-changed", refresh); };
  }, [project]);

  function tag(member: Member) {
    const store = useStore.getState();
    let lane = store.active ? store.lanes[store.active] : undefined;
    if (!lane || lane.project !== project || lane.agent !== "claude" || lane.interface === "terminal") {
      lane = (store.projects[project]?.lanes ?? []).map((id) => store.lanes[id]).find((candidate) => candidate?.agent === "claude" && candidate.interface !== "terminal");
    }
    if (!lane) {
      const id = store.newLane(project, false, "claude", "chat");
      store.select(id);
      lane = useStore.getState().lanes[id];
    } else {
      store.select(lane.id);
    }
    const draft = lane?.draft ?? "";
    store.draft(lane!.id, `${draft}${draft && !/\s$/.test(draft) ? " " : ""}@${member.name} `);
    requestAnimationFrame(() => document.querySelector<HTMLTextAreaElement>(".chat-composer textarea")?.focus());
  }

  if (!loaded) return null;
  const shown = showAll ? members : members.slice(0, MEMBERS_VISIBLE);
  return <section className="sidebar-section member-section" aria-label={`Keel Agents in ${projectName ?? project}`}>
    <button className="sidebar-section-toggle" onClick={() => setExpanded((open) => !open)} aria-expanded={expanded} aria-controls="sidebar-members" title={`Keel Agents in ${projectName ?? project}`}>
      <Icon name={expanded ? "chevron-down" : "chevron-right"} size={12} />Keel Agents
    </button>
    <div id="sidebar-members" hidden={!expanded}>
      {members.length === 0 && <div className="member-empty">
        <p>{loadError ? "Couldn’t load your members." : "Install subagents to start tagging your team."}</p>
        <button onClick={() => {
          if (loadError) window.dispatchEvent(new Event("keel:subagents-changed"));
          else useStore.setState({ extensions: true, extensionsSection: "agents", settings: false, creating: false });
        }}>{loadError ? "Retry" : "Add team"}<Icon name={loadError ? "chevron-right" : "plus"} size={12} /></button>
      </div>}
      {shown.map((member) => <button className={`member-row${working.has(member.name) ? " is-working" : ""}`} key={member.name} onClick={() => tag(member)} aria-label={`Tag ${member.display_name || member.name}${working.has(member.name) ? ", working" : ""}`} title={`@${member.name}${working.has(member.name) ? " · Working" : ""} · click to tag`}>
        <MemberAvatar name={member.name} displayName={member.display_name} avatar={member.avatar} working={working.has(member.name)} />
        <span className="member-name">{member.display_name || member.name}</span>
        {working.has(member.name) ? <Working /> : <span className="member-action" aria-hidden="true">@</span>}
      </button>)}
      {members.length > MEMBERS_VISIBLE && <button className="member-more" onClick={() => setShowAll((all) => !all)} aria-expanded={showAll}>{showAll ? "Show less" : `Show ${members.length - MEMBERS_VISIBLE} more`}</button>}
      {members.length > 0 && <p className="member-hint">Type <span>@</span> to mention a member</p>}
    </div>
  </section>;
}

function ProjectRow({ path }: { path: string }) {
  const p = useStore((s) => s.projects[path]);
  const needs = useStore((s) => (s.projects[path]?.lanes ?? []).filter((l) => s.lanes[l] && waiting(s.lanes[l]) > 0).length);
  const busy = useStore((s) => (s.projects[path]?.lanes ?? []).filter((l) => s.lanes[l] && activity(s.lanes[l]).tone === "live").length);
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
        <button className="ghost project-toggle" onClick={toggle} aria-expanded={!p.collapsed} aria-label={`${p.collapsed ? "Expand" : "Collapse"} ${p.name}`}>
          <Icon name={p.collapsed ? "chevron-right" : "chevron-down"} size={12} />
          <span className="project-name">{p.name}</span>
        </button>
        <button className="ghost" onClick={(e) => setMenu({ at: e.currentTarget.getBoundingClientRect(), kind: "new" })} aria-label={`New lane in ${p.name}`} title="New lane — Claude Code, Codex or Antigravity">
          <Icon name="plus" size={14} />
        </button>
        <button className="ghost hover-only" onClick={(e) => setMenu({ at: e.currentTarget.getBoundingClientRect(), kind: "more" })} aria-label={`${p.name} actions`}>
          <Icon name="more" size={14} />
        </button>
      </div>
      <div className="project-status">
        <SetupBadge project={path} />
        {p.collapsed && needs > 0 && <span className="small" style={{ color: "var(--warn)" }}>{needs} needs you</span>}
        {p.collapsed && busy > 0 && (
          <span className="busy-label" title={`${busy} lane${busy === 1 ? "" : "s"} running a turn`}>
            <span className="busy-dot" /> {busy} running
          </span>
        )}
        {p.starting && <span className="small faint">starting…</span>}
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
          <div className="sep" />
          <div className="eyebrow" style={{ padding: "4px 8px" }}>Antigravity</div>
          <button onClick={() => make(false, "antigravity")}>
            <Icon name="terminal" size={14} /> In the project's tree
          </button>
          <button onClick={() => make(true, "antigravity")}>
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
          <button onClick={() => (setCtx(null), make(false, "antigravity"))}>
            <Icon name="terminal" size={14} /> New Antigravity lane
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
      {!p.collapsed && <div className="project-lanes">
        {p.lanes.map((l) => <LaneRow key={l} id={l} />)}
        {!p.lanes.length && <button className="lane lane-create" onClick={(e) => setMenu({ at: e.currentTarget.getBoundingClientRect(), kind: "new" })}><Icon name="plus" size={14} />New lane</button>}
        <History path={path} sessions={p.sessions} />
      </div>}
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
  const subText = useStore((s) => s.lanes[id] ? lastWord(s.lanes[id]) : "");
  // What it is doing, under the title while it is doing it; an idle lane stays one line.
  // A string, not the object: a selector that builds a new object every call re-renders forever.
  const now = useStore((s) => {
    const a = s.lanes[id] ? activity(s.lanes[id]) : undefined;
    return a && a.tone !== "idle" ? `${a.tone}\n${a.label}` : "";
  });
  const [tone, doing] = now ? now.split("\n") : ["", ""];
  const [ctx, setCtx] = useState<DOMRect | null>(null);
  const root = useStore((s) => (l ? s.projects[l.project]?.path : undefined));
  if (!l) return null;
  const asking = waiting(l);
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
        {asking ? <Icon name="hand" size={11} style={{ color: "var(--warn)" }} /> : failed ? <Icon name="x-circle" size={11} style={{ color: "var(--del)" }} /> : tone === "live" ? <span className="busy-dot" /> : <Icon name={l.isolated ? "branch" : "chat"} size={14} />}
      </span>
      <span className="lane-text" title={`${l.title} — ${AGENT_NAME[l.agent ?? "claude"]}${subText ? ` · ${subText}` : ""}`}>
        <span className="lane-title">{l.title}</span>
        {doing && <span className={`lane-sub ${tone}`}>{doing}</span>}
      </span>
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
        {l.agent !== "antigravity" && <button onClick={() => (setCtx(null), void navigator.clipboard.writeText(`${l.agent === "codex" ? "codex resume" : "claude --resume"} ${l.session}`))}>
          <Icon name="copy" size={14} /> Copy resume command
        </button>}
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
  const openKey = useStore((s) => (s.projects[path]?.lanes ?? []).map((id) => `${s.lanes[id]?.agent ?? "claude"}:${s.lanes[id]?.session ?? ""}`).join(","));
  const openIds = useMemo(() => new Set(openKey.split(",")), [openKey]);
  const [open, setOpen] = useState(false);
  const [all, setAll] = useState(false);
  const [ctx, setCtx] = useState<{ at: DOMRect; session: Session } | null>(null);
  if (!sessions) return null;
  const list = [...sessions].filter((s) => !openIds.has(`${s.provider ?? "claude"}:${s.id}`)).sort((a, b) => (b.last_active ?? "").localeCompare(a.last_active ?? ""));
  if (!list.length) return null;
  const shown = all ? list : list.slice(0, 5);
  return (
    <div className="history">
      <button className="lane history-head" onClick={() => setOpen(!open)} aria-expanded={open}>
        <Icon name={open ? "chevron-down" : "chevron-right"} size={12} />
        <span>History</span>
        <span className="history-count">{list.length}</span>
      </button>
      {open &&
        shown.map((s) => (
          <button key={`${s.provider ?? "claude"}:${s.id}`} className="lane" onClick={() => resume(path, s)} onContextMenu={(e) => (e.preventDefault(), setCtx({ at: pointer(e), session: s }))} aria-label={`Resume ${AGENT_NAME[s.provider ?? "claude"]}: ${s.title || "Untitled session"}`} title={`${AGENT_NAME[s.provider ?? "claude"]} — ${s.title || "Untitled session"} — ${s.messages} messages${s.branch ? ` · ${s.branch}` : ""}`}>
            <span className="marker">{s.busy && <span className="busy-dot" />}</span>
            <span className="lane-title faint">{s.title || "Untitled session"}</span>
            <span className="mono faint" style={{ fontSize: 10, whiteSpace: "nowrap" }}>
              {AGENT_NAME[s.provider ?? "claude"]} · {ago(s.last_active)}
            </span>
          </button>
        ))}
      {ctx && (
        <Floating anchor={ctx.at} onClose={() => setCtx(null)} width={250}>
          <button onClick={() => (setCtx(null), resume(path, ctx.session))}>
            <Icon name="terminal" size={14} /> Resume in a lane
          </button>
          <button onClick={() => (setCtx(null), void navigator.clipboard.writeText(resumeCommand(ctx.session)))}>
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
