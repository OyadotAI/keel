import { UpdateButton } from "./UpdateButton";
import { restoreZoom, zoom } from "../zoom";
import { useCallback, useEffect, useState, type ReactNode, type Dispatch, type SetStateAction } from "react";
import { Sidebar, pickProject } from "./Sidebar";
import { Shortcuts } from "./Shortcuts";
import { focusTerminal } from "./kit";
import { useStore } from "../store";
import { Settings } from "./Settings";
import { NewProject } from "./NewProject";
import { Extensions } from "./Extensions";
import { Agents, ShellDock, SidePanel, TABS, type Tab } from "./Lane";
import { LaneHeader, Confirm, type Asking } from "./LaneHeader";
import { ApprovalStrip } from "./Approvals";
import { InterfaceSetting } from "./InterfaceSetting";
import { Composer } from "./Composer";
import { StatusBar } from "./StatusBar";
import { Drop } from "./Drop";
import { Palette, type Command } from "./Palette";
import { label, matches, type Chord } from "../keys";
import { useIde } from "../ide";
import { Explorer, IdeWorkspace, LayoutSwitch } from "./Ide";
import { FileNavigation } from "./FileNavigation";
import { ideCommand } from "../ide-keys";
import { MAC } from "../keys";

const remembered = (key: string, fallback: string) => {
  try {
    return localStorage.getItem(key) ?? fallback;
  } catch {
    return fallback;
  }
};
const remember = (key: string, value: string) => {
  try {
    localStorage.setItem(key, value);
  } catch {
    // A remembered layout is a convenience.
  }
};

/// A divider that resizes by writing a CSS variable while dragging — no React state per pointer
/// move — and remembers the width when it is let go.
function Handle({ name, min, max, fallback, from }: { name: string; min: number; max: number; fallback: number; from: "left" | "right" }) {
  const set = (px: number) => document.documentElement.style.setProperty(name, `${Math.round(Math.min(max, Math.max(min, px)))}px`);
  const now = () => parseInt(getComputedStyle(document.documentElement).getPropertyValue(name)) || fallback;
  return (
    <div
      className="handle"
      role="separator"
      tabIndex={0}
      aria-orientation="vertical"
      aria-label={name === "--ide-chat-w" ? "Resize the conversation" : name === "--panel-w" ? "Resize the side panel" : "Resize the sidebar"}
      aria-valuemin={min}
      aria-valuemax={max}
      aria-valuenow={fallback}
      onDoubleClick={() => (set(fallback), remember(name, String(fallback)))}
      onKeyDown={(e) => {
        const step = e.key === "ArrowLeft" ? -16 : e.key === "ArrowRight" ? 16 : 0;
        if (step) { e.preventDefault(); set(now() + (from === "left" ? step : -step)); }
        if (e.key === "Home") set(fallback);
        remember(name, String(now()));
      }}
      onPointerDown={(e) => {
        const el = e.currentTarget;
        el.setPointerCapture(e.pointerId);
        el.classList.add("dragging");
        const start = e.clientX;
        const width = now();
        const move = (m: PointerEvent) => set(width + (from === "left" ? m.clientX - start : start - m.clientX));
        const up = () => {
          el.classList.remove("dragging");
          el.removeEventListener("pointermove", move);
          el.removeEventListener("pointerup", up);
          remember(name, String(now()));
          window.dispatchEvent(new Event("resize"));
        };
        el.addEventListener("pointermove", move);
        el.addEventListener("pointerup", up);
      }}
    />
  );
}

/** Closing retains a visible entry point and returns keyboard focus to it. */
function useWorkspacePanel() {
  const ide = useIde((s) => s.layout === "ide");
  const [chatPanel, setChatPanel] = useState(() => window.innerWidth >= 1200 && remembered("keel.panel", "1") !== "0");
  const [idePanel, setIdePanel] = useState(false);
  const panel = ide ? idePanel : chatPanel;
  const setPanel = ide ? setIdePanel : setChatPanel;
  const [expanded, setExpanded] = useState(false);
  const closePanel = useCallback(() => {
    setPanel(false);
    setExpanded(false);
    if (!ide) remember("keel.panel", "0");
    requestAnimationFrame(() => document.querySelector<HTMLButtonElement>(".workspace-toggle")?.focus());
  }, [setPanel, ide]);
  const togglePanel = useCallback(() => {
    const s = useStore.getState();
    if (s.settings || s.extensions || s.creating) {
      useStore.setState({ settings: false, extensions: false, creating: false });
      setPanel(true);
      if (!ide) remember("keel.panel", "1");
    } else if (panel) closePanel();
    else { setPanel(true); if (!ide) remember("keel.panel", "1"); }
  }, [panel, closePanel, setPanel, ide]);
  return { panel, setPanel, expanded, setExpanded, togglePanel, closePanel };
}

export function App() {
  const ide = useIde((s) => s.layout === "ide");
  const active = useStore((s) => s.active);
  const exists = useStore((s) => (s.active ? !!s.lanes[s.active] : false));
  const settings = useStore((s) => s.settings);
  const creating = useStore((s) => s.creating);
  const extensions = useStore((s) => s.extensions);
  const { panel, setPanel, expanded, setExpanded, togglePanel, closePanel } = useWorkspacePanel();
  const [sidebar, setSidebar] = useState(() => window.innerWidth >= 900 && remembered("keel.sidebar", "1") !== "0");
  const [tab, setTab] = useState<Tab>("turns");
  const [shell, setShell] = useState(false);
  const [palette, setPalette] = useState(false);
  const [conversation, setConversation] = useState(false);
  const asking = useStore((s) => s.asking);
  const setAsking = (a: Asking) => useStore.setState({ asking: a });

  useEffect(restoreZoom, []);
  useEffect(() => { if (ide) setShell(true); }, [ide]);
  useEffect(() => {
    for (const [name, px] of [["--sidebar-w", 240], ["--panel-w", 380], ["--ide-chat-w", 400], ["--coding-sidebar-w", 320]] as const) document.documentElement.style.setProperty(name, `${remembered(name, String(px))}px`);
    // The lane on screen at launch is opened like a clicked one: its terminal, its transcript.
    const a = useStore.getState().active;
    if (a && useStore.getState().lanes[a]) useStore.getState().load(a);
  }, []);

  const lane = active && exists ? active : undefined;
  const toggleSidebar = useCallback(() => setSidebar((p) => (remember("keel.sidebar", p ? "0" : "1"), !p)), []);
  const showTab = useCallback((t: Tab) => {
    setPanel(true);
    setTab(t);
    // Into the tab, so ↑↓ and Enter work at once; two frames so the list has rendered.
    requestAnimationFrame(() => requestAnimationFrame(() => document.querySelector<HTMLElement>(".side-panel .tab-body [data-file]:not(:disabled), .side-panel .tab-body button:not(:disabled)")?.focus()));
  }, [setPanel]);
  // A tab asked for from outside — the sidebar's setup list has no window state to reach.
  const asked = useStore((s) => s.panelTab);
  useEffect(() => {
    if (!asked) return;
    setPanel(true);
    setTab(asked);
    useStore.setState({ panelTab: undefined });
  }, [asked, setPanel]);


  useEffect(() => {
    const chords: [Chord, () => void][] = [
      [{ key: "k" }, () => setPalette((p) => !p)],
      [{ key: "o" }, () => void pickProject()],
      [{ key: "l" }, focusTerminal],
      [{ key: "j" }, () => setShell((x) => !x)],
      [{ key: "e", shift: true }, toggleSidebar],
      [{ key: "i", alt: true }, togglePanel],
      [{ key: "," }, () => useStore.setState((s) => ({ settings: !s.settings }))],
      [{ key: "]", shift: true }, () => cycle(1)],
      [{ key: "[", shift: true }, () => cycle(-1)],
      [{ key: "/" }, () => useStore.setState((s) => ({ shortcuts: !s.shortcuts }))],
      [{ key: "ArrowDown", alt: true }, () => cycle(1)],
      [{ key: "=" }, () => zoom(1)],
      [{ key: "=", shift: true }, () => zoom(1)],
      [{ key: "-" }, () => zoom(-1)],
      [{ key: "0" }, () => zoom(0)],
      [{ key: "ArrowUp", alt: true }, () => cycle(-1)],
      ...TABS.map((t, i): [Chord, () => void] => [{ key: String(i + 1), alt: true }, () => showTab(t)]),
      ...Array.from({ length: 9 }, (_, i): [Chord, () => void] => [{ key: String(i + 1) }, () => nth(i)]),
    ];
    const laneChords: [Chord, (l: string) => void][] = [
      [{ key: "n" }, (l) => newIn(l, false)],
      [{ key: "n", shift: true }, (l) => newIn(l, true)],
      [{ key: "a", shift: true }, (l) => answerOldest(l, "allow")],
      [{ key: "d", shift: true }, (l) => answerOldest(l, "deny")],
      [{ key: "s", shift: true }, (l) => answerOldest(l, "allow", "session")],
      [{ key: "m", shift: true }, (l) => useStore.getState().lanes[l]?.wt && setAsking("merge")],
      [{ key: "Backspace", shift: true }, (l) => useStore.getState().lanes[l]?.wt && setAsking("discard")],
      [{ key: "w" }, () => setAsking("close")],
      [{ key: "." }, (l) => void useStore.getState().stop(l)],
    ];
    const handler = (e: KeyboardEvent) => {
      if (handleIdeShortcut(e, { setPalette, setShell, toggleSidebar, setSidebar })) return;
      for (const [c, run] of chords)
        if (matches(e, c)) {
          e.preventDefault();
          return run();
        }
      const l = useStore.getState().active;
      if (!l) return;
      for (const [c, run] of laneChords)
        if (matches(e, c)) {
          e.preventDefault();
          return run(l);
        }
    };
    window.addEventListener("keydown", handler);
    return () => window.removeEventListener("keydown", handler);
  }, [togglePanel, toggleSidebar, showTab]);

  const navigation = useIde((s) => s.navigation);
  const showCode = () => { if (panel) closePanel(); };
  const showGit = () => { setSidebar(true); useIde.getState().setSidebar("git"); };
  const overlay = creating ? <NewProject /> : extensions ? <Extensions /> : settings ? <Settings /> : !lane ? <Welcome /> : null;
  const showPanel = panel && !!lane && !overlay;
  const ideActive = ide && !!lane && !overlay;
  const workspace = showPanel && lane ? <SidePanel key={lane} lane={lane} tab={tab} setTab={setTab} expanded={expanded} toggleExpanded={() => setExpanded((x) => !x)} close={closePanel} ask={(what) => setAsking(what)} /> : null;
  return (
    <div className={`app ${sidebar ? "" : "no-sidebar"} ${ideActive ? "ide-layout" : ""} ${conversation ? "ide-conversation-open" : ""}`}>
      <Sidebar showCode={showCode} explorer={ideActive && lane ? <Explorer key={lane} lane={lane} enabled={ideActive} showCode={showCode} /> : undefined} />
      <button className="sidebar-backdrop" style={{ background: "transparent" }} aria-label="Close projects" onClick={toggleSidebar} />
      <Handle name={ideActive ? "--coding-sidebar-w" : "--sidebar-w"} min={240} max={ideActive ? 480 : 360} fallback={ideActive ? 320 : 240} from="left" />
      <WorkspaceMain lane={lane} overlay={overlay} showPanel={showPanel} expanded={expanded} ideActive={ideActive} shell={shell} setShell={setShell} closePanel={closePanel} workspace={workspace} conversation={conversation} setConversation={setConversation} showGit={showGit} />
      <StatusBar layout={lane ? <LayoutSwitch /> : undefined} openTab={showTab} panel={showPanel} togglePanel={togglePanel} sidebar={sidebar} toggleSidebar={toggleSidebar} />
      <Drop />
      {palette && <PaletteHost lane={lane} close={() => setPalette(false)} actions={{ showTab, togglePanel, toggleSidebar, focusTerminal, setShell, setAsking }} />}
      {ideActive && navigation && lane && <FileNavigation key={`${lane}:${navigation}`} lane={lane} showCode={showCode} />}
      <Shortcuts />
      {asking && lane && <Confirm lane={lane} what={asking} done={() => setAsking(null)} />}
    </div>
  );
}

function handleIdeShortcut(e: KeyboardEvent, { setPalette, setShell, toggleSidebar, setSidebar }: {
  setPalette: Dispatch<SetStateAction<boolean>>; setShell: Dispatch<SetStateAction<boolean>>;
  toggleSidebar: () => void; setSidebar: Dispatch<SetStateAction<boolean>>;
}): boolean {
  if (useIde.getState().layout !== "ide" || !useStore.getState().active || getOverlay()) return false;
  const command = ideCommand(e, MAC);
  if (command) {
    e.preventDefault();
    if (command === "commands") setPalette((p) => !p);
    else if (command === "terminal") setShell((p) => !p);
    else if (command === "sidebar") toggleSidebar();
    else if (command === "explorer" || command === "git") {
      setSidebar(true); useIde.getState().setSidebar(command);
    } else {
      const active = useStore.getState().active;
      if (active && command === "closeFile") {
        const selected = useIde.getState().lanes[active]?.selected;
        if (useIde.getState().lanes[active]?.commit) useIde.getState().patch(active, { commit: undefined });
        else if (selected) useIde.getState().close(active, selected);
      } else useIde.setState({ navigation: command as "files" | "line" | "find" });
    }
    return true;
  }
  return false;
}

function getOverlay() {
  const s = useStore.getState();
  return s.settings || s.extensions || s.creating || !!document.querySelector('[role="dialog"], [role="menu"]');
}

function WorkspaceMain({ lane, overlay, showPanel, expanded, ideActive, shell, setShell, closePanel, workspace, conversation, setConversation, showGit }: {
  lane?: string; overlay: ReactNode; showPanel: boolean; expanded: boolean; ideActive: boolean; shell: boolean;
  setShell: Dispatch<SetStateAction<boolean>>; closePanel: () => void; workspace: ReactNode;
  conversation: boolean; setConversation: Dispatch<SetStateAction<boolean>>; showGit: () => void;
}) {
  const questions = useStore((s) => lane ? s.lanes[lane]?.pending.length ?? 0 : 0);
  useEffect(() => {
    if (ideActive && questions) setConversation(true);
  }, [ideActive, questions, setConversation]);
  return (<main className={`main ${showPanel && !ideActive ? "with-panel" : ""} ${showPanel && expanded && !ideActive ? "expanded" : ""} ${shell ? "has-shell" : ""}`}>
        <AgentPane lane={lane} hidden={!!overlay} shell={shell} setShell={setShell} closeConversation={() => setConversation(false)} />
        {lane && <IdeWorkspace key={`ide:${lane}`} lane={lane} enabled={ideActive} terminalOpen={shell} toggleTerminal={() => setShell((s) => !s)} showGit={showGit} showCode={showPanel ? closePanel : undefined}>{ideActive ? workspace : null}</IdeWorkspace>}
        {ideActive && <>
          <Handle name="--ide-chat-w" min={340} max={720} fallback={400} from="right" />
          <button className="ide-conversation-toggle" onClick={() => setConversation((x) => !x)} aria-expanded={conversation}>Conversation{questions > 0 && ` · ${questions} waiting`}</button>
          {conversation && <button className="ide-chat-backdrop" aria-label="Close conversation" onClick={() => setConversation(false)} />}
        </>}
        {showPanel && !ideActive && <>
          <button className="panel-backdrop" aria-label="Close workspace panel" onClick={closePanel} />
          <Handle name="--panel-w" min={320} max={720} fallback={380} from="right" />
        </>}
        <ShellDock lane={lane} open={shell && !overlay} close={() => setShell(false)} />
        {!ideActive && workspace}
        {overlay}
      </main>);
}

function AgentPane({ lane, hidden, shell, setShell, closeConversation }: { lane?: string; hidden: boolean; shell: boolean; setShell: (value: boolean | ((current: boolean) => boolean)) => void; closeConversation: () => void }) {
  return <section className="agent-pane" style={hidden ? { display: "none" } : undefined}>
    {lane && <LaneHeader key={`header:${lane}`} lane={lane} shell={shell} toggleShell={() => setShell((s) => !s)} />}
    {lane && <LaneError lane={lane} />}
    <div className="agents"><Agents /></div>
    <div className="ide-chat-tools"><button className="ghost" onClick={closeConversation}>Close conversation</button></div>
    {lane && <LaneComposer key={`composer:${lane}`} lane={lane} />}
  </section>;
}

function LaneError({ lane }: { lane: string }) {
  const error = useStore((s) => s.lanes[lane]?.error);
  if (!error) return null;
  return (
    <div className="banner error" role="alert">
      {error}{" "}
      {/auth|oauth|expired|unauthorized|\/login/i.test(error) && <button className="link small" onClick={() => useStore.setState({ settings: true })}>Log in →</button>}{" "}
      <button className="link small" onClick={() => useStore.setState((s) => ({ lanes: { ...s.lanes, [lane]: { ...s.lanes[lane], error: undefined } } }))}>
        Dismiss
      </button>
    </div>
  );
}

function lanesInOrder() {
  const s = useStore.getState();
  return s.order.flatMap((p) => s.projects[p]?.lanes ?? []);
}
export function cycle(by: number) {
  const all = lanesInOrder();
  const at = all.indexOf(useStore.getState().active ?? "");
  if (all.length) useStore.getState().select(all[(at + by + all.length) % all.length]);
}
function nth(i: number) {
  const s = useStore.getState();
  const project = s.active ? s.lanes[s.active]?.project : s.order[0];
  const lane = project ? s.projects[project]?.lanes[i] : undefined;
  if (lane) s.select(lane);
}
function newIn(lane: string, isolated: boolean) {
  const s = useStore.getState();
  const project = s.lanes[lane]?.project;
  if (project) s.select(s.newLane(project, isolated, s.lanes[lane]?.agent));
}
/// Never answers something the person has not seen: with nothing waiting here, the first press
/// goes to the lane that is waiting and the second answers there. A question is focused, not
/// answered — its options are its own keys.
function answerOldest(lane: string, decision: "allow" | "deny", scope: "once" | "session" = "once") {
  const s = useStore.getState();
  const p = s.lanes[lane]?.pending[0];
  if (!p) {
    const waiting = lanesInOrder().find((l) => s.lanes[l]?.pending.length);
    if (waiting) s.select(waiting);
    return;
  }
  if (p.tool === "AskUserQuestion") return void document.querySelector<HTMLElement>(".strip-row .actions button")?.focus();
  if (scope === "session" && !p.rules.length) return;
  void s.answer(lane, p, decision, undefined, scope);
}

/// Mounted only while the palette is open: building its list reads the whole store, and doing that
/// in `App` would re-render the window on every event the agent sends.
function PaletteHost({ lane, close, actions }: { lane?: string; close: () => void; actions: Omit<Parameters<typeof useCommands>[0], "lane"> }) {
  const commands = useCommands({ lane, ...actions });
  return <Palette commands={commands} close={close} />;
}

function useCommands({ lane, showTab, togglePanel, toggleSidebar, focusTerminal, setShell, setAsking }: { lane?: string; showTab: (t: Tab) => void; togglePanel: () => void; toggleSidebar: () => void; focusTerminal: () => void; setShell: (f: (s: boolean) => boolean) => void; setAsking: (a: Asking) => void }): Command[] {
  const s = useStore();
  const out: Command[] = [];
  const l = lane ? s.lanes[lane] : undefined;
  for (const p of l?.pending ?? []) {
    if (p.tool === "AskUserQuestion") continue;
    out.push({ id: `allow-${p.id}`, title: `Allow once: ${p.tool} ${p.command.slice(0, 60)}`, keys: label({ key: "a", shift: true }), run: () => void s.answer(lane!, p, "allow") });
    if (p.rules.length) {
      out.push({ id: `session-${p.id}`, title: `Allow ${p.rules.join(", ")} for this session`, keys: label({ key: "s", shift: true }), run: () => void s.answer(lane!, p, "allow", undefined, "session") });
      out.push({ id: `project-${p.id}`, title: `Always allow ${p.rules.join(", ")} in this project`, run: () => void s.answer(lane!, p, "allow", undefined, "project") });
    }
    out.push({ id: `deny-${p.id}`, title: `Deny: ${p.tool} ${p.command.slice(0, 60)}`, keys: label({ key: "d", shift: true }), run: () => void s.answer(lane!, p, "deny") });
  }
  for (const pid of s.order) {
    const p = s.projects[pid];
    p?.lanes.forEach((id, i) => {
      const x = s.lanes[id];
      if (x) out.push({ id: `go-${id}`, title: `Go to lane: ${x.title}`, detail: p.name, keys: i < 9 && l?.project === pid ? label({ key: String(i + 1) }) : undefined, run: () => s.select(id) });
    });
    for (const ses of (p?.sessions ?? []).slice(0, 30)) out.push({ id: `resume-${ses.id}`, title: `Resume: ${ses.title || "Untitled session"}`, detail: p.name, run: () => s.resume(pid, ses) });
  }
  if (l) {
    if (useIde.getState().layout === "ide") {
      for (const [id, title] of [["files", "Go to File…"], ["line", "Go to Line…"], ["find", "Find in File…"]] as const) out.push({ id: `ide-${id}`, title, run: () => useIde.setState({ navigation: id }) });
      out.push({ id: "ide-git", title: "Source Control", run: () => { useIde.getState().setSidebar("git"); showTab("git"); } });
    }
    out.push({ id: "new", title: "New lane", detail: s.projects[l.project]?.name, keys: label({ key: "n" }), run: () => newIn(lane!, false) });
    out.push({ id: "new-branch", title: "New lane on its own branch", detail: s.projects[l.project]?.name, keys: label({ key: "n", shift: true }), run: () => newIn(lane!, true) });
    if (l.wt) {
      out.push({ id: "merge", title: "Merge lane…", keys: label({ key: "m", shift: true }), run: () => setAsking("merge") });
      out.push({ id: "discard", title: "Discard lane…", keys: label({ key: "Backspace", shift: true }), run: () => setAsking("discard") });
    }
    out.push({ id: "rename", title: "Rename lane…", keys: "F2", run: () => useStore.setState({ renaming: lane! }) });
    out.push({ id: "close", title: "Close lane", keys: label({ key: "w" }), run: () => setAsking("close") });
    out.push({ id: "focus", title: "Focus composer", keys: label({ key: "l" }), run: focusTerminal });
    out.push({ id: "shell", title: "Toggle the shell", keys: label({ key: "j" }), run: () => setShell((x) => !x) });
    TABS.forEach((t, i) => out.push({ id: `tab-${t}`, title: `Show ${t[0].toUpperCase()}${t.slice(1)}`, keys: label({ key: String(i + 1), alt: true }), run: () => showTab(t) }));
    out.push({ id: "trust", title: s.trusted[l.project] ? "Stop trusting this project" : "Trust this project…", run: () => useStore.setState({ settings: true }) });
  }
  out.push({ id: "shortcuts", title: "Keyboard shortcuts", keys: label({ key: "/" }), run: () => useStore.setState({ shortcuts: true }) });
  out.push({ id: "next-lane", title: "Next lane", keys: label({ key: "ArrowDown", alt: true }), run: () => cycle(1) });
  out.push({ id: "prev-lane", title: "Previous lane", keys: label({ key: "ArrowUp", alt: true }), run: () => cycle(-1) });
  out.push({ id: "open", title: "Open project…", keys: label({ key: "o" }), run: () => void pickProject() });
  out.push({ id: "new-project", title: "New project…", run: () => useStore.setState({ creating: true, settings: false }) });
  out.push({ id: "sidebar", title: "Toggle sidebar", keys: label({ key: "e", shift: true }), run: toggleSidebar });
  out.push({ id: "panel", title: "Toggle panel", keys: label({ key: "i", alt: true }), run: togglePanel });
  out.push({ id: "settings", title: "Settings", keys: label({ key: "," }), run: () => useStore.setState({ settings: true }) });
  out.push({ id: "extensions", title: "Extensions — skills, subagents, MCP servers, plugins", run: () => useStore.setState({ extensions: true, settings: false, creating: false }) });
  return out;
}

function Welcome() {
  const none = useStore((s) => s.order.length === 0);
  return (
    <div className="settings">
      <header className="lane-header"><span className="lane-heading">Keel</span><UpdateButton /></header>
      <div className="empty">
      <h1>{none ? "Open a project to start" : "Pick a lane"}</h1>
      {none && (
        <div className="actions" style={{ justifyContent: "center" }}>
          <button className="primary" onClick={() => void pickProject()}>
            Open project…
          </button>
          <button onClick={() => useStore.setState({ creating: true })}>New project…</button>
        </div>
      )}
      <p>
        {none
          ? "Each lane runs your own Claude Code or Codex in the project, and Keel shows every turn beside it: the files it changed, the commands it ran, whether the checks passed."
          : "Each lane is one conversation. Open one on the left, or start a new one under a project."}
      </p>
      {none && <InterfaceSetting />}
      <p className="small faint">{label({ key: "k" })} opens the command palette.</p>
      </div>
    </div>
  );
}

function LaneComposer({ lane }: { lane: string }) {
  const surface = useStore((s) => s.lanes[lane]?.interface);
  return surface === "terminal" ? <ApprovalStrip lane={lane} /> : <Composer lane={lane} />;
}
