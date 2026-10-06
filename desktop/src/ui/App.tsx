import { useCallback, useEffect, useState } from "react";
import { Sidebar, pickProject } from "./Sidebar";
import { useStore } from "../store";
import { Settings } from "./Settings";
import { NewProject } from "./NewProject";
import { Extensions } from "./Extensions";
import { Agents, Shell, SidePanel, TABS, type Tab } from "./Lane";
import { LaneHeader, Confirm, type Asking } from "./LaneHeader";
import { ApprovalStrip } from "./Approvals";
import { StatusBar } from "./StatusBar";
import { Drop } from "./Drop";
import { Palette, type Command } from "./Palette";
import { label, matches, type Chord } from "../keys";

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
      aria-label={name === "--panel-w" ? "Resize the side panel" : "Resize the sidebar"}
      onDoubleClick={() => (set(fallback), remember(name, String(fallback)))}
      onKeyDown={(e) => {
        const step = e.key === "ArrowLeft" ? -16 : e.key === "ArrowRight" ? 16 : 0;
        if (step) set(now() + (from === "left" ? step : -step));
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

export function App() {
  const active = useStore((s) => s.active);
  const exists = useStore((s) => (s.active ? !!s.lanes[s.active] : false));
  const settings = useStore((s) => s.settings);
  const creating = useStore((s) => s.creating);
  const extensions = useStore((s) => s.extensions);
  const [panel, setPanel] = useState(() => remembered("keel.panel", "1") !== "0");
  const [sidebar, setSidebar] = useState(() => remembered("keel.sidebar", "1") !== "0");
  const [expanded, setExpanded] = useState(false);
  const [tab, setTab] = useState<Tab>("turns");
  const [shell, setShell] = useState(false);
  const [palette, setPalette] = useState(false);
  const asking = useStore((s) => s.asking);
  const setAsking = (a: Asking) => useStore.setState({ asking: a });

  useEffect(() => {
    for (const [name, px] of [["--sidebar-w", 240], ["--panel-w", 420]] as const) document.documentElement.style.setProperty(name, `${remembered(name, String(px))}px`);
    // The lane on screen at launch is opened like a clicked one: its terminal, its transcript.
    const a = useStore.getState().active;
    if (a && useStore.getState().lanes[a]) useStore.getState().load(a);
  }, []);

  const lane = active && exists ? active : undefined;
  const togglePanel = useCallback(() => setPanel((p) => (remember("keel.panel", p ? "0" : "1"), !p)), []);
  const toggleSidebar = useCallback(() => setSidebar((p) => (remember("keel.sidebar", p ? "0" : "1"), !p)), []);
  const focusTerminal = () => (document.querySelector(".agents .terminal:not(.hidden) textarea") as HTMLElement | null)?.focus();
  const showTab = (t: Tab) => {
    setPanel(true);
    setTab(t);
  };
  // A tab asked for from outside — the sidebar's setup list has no window state to reach.
  const asked = useStore((s) => s.panelTab);
  useEffect(() => {
    if (!asked) return;
    setPanel(true);
    setTab(asked);
    useStore.setState({ panelTab: undefined });
  }, [asked]);


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
      [{ key: "ArrowDown", alt: true }, () => cycle(1)],
      [{ key: "ArrowUp", alt: true }, () => cycle(-1)],
      ...TABS.map((t, i): [Chord, () => void] => [{ key: String(i + 1), alt: true }, () => showTab(t)]),
      ...Array.from({ length: 9 }, (_, i): [Chord, () => void] => [{ key: String(i + 1) }, () => nth(i)]),
    ];
    const laneChords: [Chord, (l: string) => void][] = [
      [{ key: "n" }, (l) => newIn(l, false)],
      [{ key: "n", shift: true }, (l) => newIn(l, true)],
      [{ key: "a", shift: true }, (l) => answerOldest(l, "allow")],
      [{ key: "d", shift: true }, (l) => answerOldest(l, "deny")],
      [{ key: "m", shift: true }, (l) => useStore.getState().lanes[l]?.wt && setAsking("merge")],
      [{ key: "Backspace", shift: true }, (l) => useStore.getState().lanes[l]?.wt && setAsking("discard")],
      [{ key: "w" }, () => setAsking("close")],
    ];
    const handler = (e: KeyboardEvent) => {
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
  }, [togglePanel, toggleSidebar]);

  const overlay = creating ? <NewProject /> : extensions ? <Extensions /> : settings ? <Settings /> : !lane ? <Welcome /> : null;
  const showPanel = panel && !!lane && !overlay;
  return (
    <div className={`app ${sidebar ? "" : "no-sidebar"}`}>
      <Sidebar />
      <Handle name="--sidebar-w" min={200} max={360} fallback={240} from="left" />
      <main className={`main ${showPanel ? "with-panel" : ""} ${showPanel && expanded ? "expanded" : ""}`}>
        <section className="agent-pane" style={overlay ? { display: "none" } : undefined}>
          {lane && <LaneHeader key={lane} lane={lane} shell={shell} toggleShell={() => setShell((s) => !s)} />}
          {lane && <LaneError lane={lane} />}
          <div className="agents">
            <Agents />
          </div>
          {lane && shell && <Shell key={lane} lane={lane} close={() => setShell(false)} />}
          {lane && <ApprovalStrip lane={lane} />}
        </section>
        {showPanel && <Handle name="--panel-w" min={320} max={720} fallback={420} from="right" />}
        {showPanel && lane && <SidePanel key={lane} lane={lane} tab={tab} setTab={setTab} expanded={expanded} toggleExpanded={() => setExpanded((x) => !x)} close={togglePanel} ask={(what) => setAsking(what)} />}
        {overlay}
      </main>
      <StatusBar openTab={showTab} />
      <Drop />
      {palette && <PaletteHost lane={lane} close={() => setPalette(false)} actions={{ showTab, togglePanel, toggleSidebar, focusTerminal, setShell, setAsking }} />}
      {asking && lane && <Confirm lane={lane} what={asking} done={() => setAsking(null)} />}
    </div>
  );
}

function LaneError({ lane }: { lane: string }) {
  const error = useStore((s) => s.lanes[lane]?.error);
  if (!error) return null;
  return (
    <div className="banner error" role="alert">
      {error}{" "}
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
function answerOldest(lane: string, decision: "allow" | "deny") {
  const s = useStore.getState();
  const p = s.lanes[lane]?.pending[0];
  if (p && p.tool !== "AskUserQuestion") void s.answer(lane, p, decision);
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
    out.push({ id: "new", title: "New lane", detail: s.projects[l.project]?.name, keys: label({ key: "n" }), run: () => newIn(lane!, false) });
    out.push({ id: "new-branch", title: "New lane on its own branch", detail: s.projects[l.project]?.name, keys: label({ key: "n", shift: true }), run: () => newIn(lane!, true) });
    if (l.wt) {
      out.push({ id: "merge", title: "Merge lane…", keys: label({ key: "m", shift: true }), run: () => setAsking("merge") });
      out.push({ id: "discard", title: "Discard lane…", keys: label({ key: "Backspace", shift: true }), run: () => setAsking("discard") });
    }
    out.push({ id: "rename", title: "Rename lane…", keys: "F2", run: () => useStore.setState({ renaming: lane! }) });
    out.push({ id: "close", title: "Close lane", keys: label({ key: "w" }), run: () => setAsking("close") });
    out.push({ id: "focus", title: "Focus terminal", keys: label({ key: "l" }), run: focusTerminal });
    out.push({ id: "shell", title: "Toggle the shell", keys: label({ key: "j" }), run: () => setShell((x) => !x) });
    TABS.forEach((t, i) => out.push({ id: `tab-${t}`, title: `Show ${t[0].toUpperCase()}${t.slice(1)}`, keys: label({ key: String(i + 1), alt: true }), run: () => showTab(t) }));
    out.push({ id: "trust", title: s.trusted[l.project] ? "Stop trusting this project" : "Trust this project…", run: () => useStore.setState({ settings: true }) });
  }
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
      <p className="small faint">{label({ key: "k" })} opens the command palette.</p>
    </div>
  );
}
