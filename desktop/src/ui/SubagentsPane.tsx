// Active children occupy temporary panes while the parent PTY stays mounted and focused.
// A child pane is read-only, so it carries no drop target of its own: a file dropped on it
// goes to the lane (the `.terminal` ancestor), the one composer this surface has.
import { useEffect, useLayoutEffect, useRef, useState, useSyncExternalStore, type CSSProperties, type ReactNode, type RefObject } from "react";
import { AGENT_NAME, useStore } from "../store";
import { terminalLease } from "../typers";
import { requestChatFocus } from "../chat-focus";
import { childrenOf, childScope, useChildPane, useChildReading, releaseGridText, SHORT_ID, type Invocation, type ChildOutput, type Shown } from "../subagents";
import { gridColumns, useDeparting, useFlip, useLiveInvocations, useVisibleChildren, usePaneMotion, useReducedMotion, PANE_OPEN_MS, PANE_CLOSE_MS, PANE_MOVE_MS } from "../subagents";
import { useGridOutput } from "../subagent-output";
import { onTerminalTheme, terminalDark } from "../terminal-theme";
import { terminalPaneColors } from "../terminal-palette";
import { onTerminalFont, terminalFont } from "../zoom";
import { MemberAvatar } from "./MemberAvatar";
import { Icon } from "./icons";
import { Markdown } from "./Markdown";
import "./SubagentsPane.css";
/** Back to where the person types: the lane's terminal, or a chat lane's composer. */
const focusMain = (lane: string) => { if (useStore.getState().lanes[lane]?.interface === "terminal") terminalLease(lane)?.focus(); else requestChatFocus(lane); };
const shortId = (child: Invocation) => child.id.split(":").at(-1)?.slice(0, SHORT_ID) ?? "";
const nameOf = (child: Invocation) => child.member || "Subagent";
/** A pane's own size: folded to its header, or given the whole subagent area. Unset is a grid tile. */
type Size = "min" | "max";
const STATUS = { working: "Working", waiting: "Waiting", unknown: "Reconnecting · status unconfirmed", done: "Done", failed: "Failed", cancelled: "Cancelled", interrupted: "Interrupted" };
/** The clock a pane opened at, or nothing: a provider that reports no start time gets no guess. */
const startedAt = (child: Invocation) => { const at = child.started ? new Date(child.started) : undefined; return at && !Number.isNaN(at.getTime()) ? at.toLocaleTimeString([], { hour: "2-digit", minute: "2-digit" }) : undefined; };
/** Grid changes never submit, restart or close an agent process. */
/** `surface` is what the main pane holds: the agent's terminal, or a chat lane's conversation —
 *  the same live helper panes beside either, so a chat shows its subagents as a terminal does. */
export function SubagentsPane({ lane, hidden, enabled = true, surface = "terminal", children }: { lane: string; hidden: boolean; enabled?: boolean; surface?: "terminal" | "chat"; children?: ReactNode }) {
  const current = useStore((s) => s.lanes[lane]), ep = useStore((s) => current ? s.projects[current.project]?.endpoint : undefined);
  const scope = childScope(current), snapshot = enabled && current ? childrenOf(current) : undefined;
  const reduced = useReducedMotion();
  const live = useLiveInvocations(scope, snapshot), shown = useDeparting(enabled ? live : [], scope, reduced ? 0 : PANE_CLOSE_MS), active = shown.filter((child) => !child.leaving);
  const view = useChildPane(scope), grid = useRef<HTMLDivElement>(null), main = useRef<HTMLElement>(null);
  const visible = useVisibleChildren(grid, active.map((child) => child.id), hidden);
  useGridOutput(lane, scope, visible, ep);
  useEffect(() => { if (hidden) releaseGridText(scope); }, [scope, hidden]);
  const [width, setWidth] = useState(0), [scrolled, setScrolled] = useState(false);
  const dark = useSyncExternalStore(onTerminalTheme, terminalDark, () => false);
  const font = useSyncExternalStore(onTerminalFont, terminalFont, () => 13);
  useEffect(() => {
    const node = grid.current; if (!node) return;
    const observer = new ResizeObserver(() => setWidth(node.clientWidth));
    setWidth(node.clientWidth); observer.observe(node); return () => observer.disconnect();
  }, []);
  // The main terminal always has the first row to itself. Open children share the rows below it and
  // minimized ones fold to their headers after them. A maximized child takes every subagent row, the
  // rest fold to headers, and main keeps its row: maximizing never covers the terminal you type in.
  const [sizes, setSizes] = useState<Record<string, Size>>({});
  const resize = (id: string, size: Size) => setSizes((before) => {
    const max = Object.keys(before).find((key) => before[key] === "max"), next = { ...before };
    // Restoring a pane that is folded only because another is maximized ends the maximize.
    if (max && max !== id && size === "min") { delete next[max]; delete next[id]; return next; }
    if (max && size === "max") delete next[max];
    if (before[id] === size) delete next[id]; else next[id] = size;
    return next;
  });
  const maxed = shown.find((child) => sizes[child.id] === "max")?.id;
  const sizeOf = (id: string): Size | undefined => maxed ? (id === maxed ? "max" : "min") : sizes[id];
  const open = shown.filter((child) => !sizeOf(child.id)), columns = gridColumns(width, maxed ? shown.length : open.length);
  const rows = maxed ? 1 : Math.ceil(open.length / columns);
  const lastOpen = open.at(-1)?.id, span = open.length % columns ? columns - open.length % columns + 1 : 1;
  useFlip(grid, shown.map((child) => child.id + (sizes[child.id] ?? "")).join() + "/" + columns, reduced);
  const template = ["minmax(var(--pane-min-height), 1fr)", ...Array(rows).fill("minmax(var(--pane-min-height), 1fr)")].join(" ");
  const style = { ...terminalPaneColors(dark), "--terminal-columns": columns, "--terminal-rows": template, "--child-terminal-font": font + "px", "--pane-open-ms": PANE_OPEN_MS + "ms", "--pane-close-ms": PANE_CLOSE_MS + "ms", "--pane-move-ms": PANE_MOVE_MS + "ms" } as CSSProperties;
  const returnToMain = () => { main.current?.scrollIntoView({ block: "nearest" }); focusMain(lane); };
  const agentName = current?.agent === "claude" ? "Claude Code" : AGENT_NAME[current?.agent ?? "claude"];
  const mainName = surface === "chat" ? "Conversation" : "Main terminal";
  const panes = shown.map((child, index) => <ChildPane key={scope + ":" + child.id} lane={lane} scope={scope} child={child} reduced={reduced} order={index + 2} connected={!!snapshot?.connected} output={view.outputs[child.id]} span={child.id === lastOpen && surface !== "chat" ? span : 1} size={sizeOf(child.id)} onSize={(size) => resize(child.id, size)} />);
  if (surface === "chat") return <ChatHelpers grid={grid} main={main} label={mainName} style={style} panes={panes} working={active.filter((c) => c.status === "working").length}>{children}</ChatHelpers>;
  return <div className={"terminal-grid-shell" + (shown.length ? " split" : "")} style={style}>
    <div ref={grid} className="terminal-grid" onScroll={(event) => setScrolled(event.currentTarget.scrollTop > 0)}>
      <section key="main" ref={main} className="terminal-grid-main" data-pane="main" aria-label={mainName}>
        <div className="terminal-main-viewport" data-main-viewport>
        <header aria-hidden={!shown.length} className={"terminal-pane-heading main-pane-chrome" + (shown.length ? " is-visible" : "")}><span className="terminal-pane-order">1</span><strong>{agentName} · Main</strong><small>you type here</small></header>
        <div className="terminal-main-content" data-main-content>{children}</div>
        <footer aria-hidden={!shown.length} className={"terminal-pane-footer main-pane-chrome" + (shown.length ? " is-visible" : "")}><span>{active.length} subagent{active.length === 1 ? "" : "s"} working</span><small>Esc in a pane returns here</small></footer>
        </div>
      </section>
      {panes}
    </div>
    {shown.length > 0 && scrolled && <button className="terminal-grid-return" onClick={returnToMain}>{mainName}</button>}
  </div>;
}
/** A chat lane keeps the conversation where it is and stacks its helpers in a column on the right;
 *  the terminal's tiling grid rearranged the whole lane each time a helper started. */
function ChatHelpers({ grid, main, label, style, panes, working, children }: { grid: RefObject<HTMLDivElement | null>; main: RefObject<HTMLElement | null>; label: string; style: CSSProperties; panes: ReactNode[]; working: number; children?: ReactNode }) {
  return <div className={"terminal-grid-shell chat-grid" + (panes.length ? " has-helpers" : "")} style={style}>
    <div ref={grid} className="chat-grid-row">
      <section key="main" ref={main} className="chat-grid-main" data-pane="main" aria-label={label}>{children}</section>
      {panes.length > 0 && <aside className="chat-helpers" aria-label="Subagents">
        <header className="chat-helpers-head"><span className={"chat-helpers-dot" + (working ? " is-live" : "")} /><strong>Helpers</strong><small>{working || "none"} working</small></header>
        {panes}</aside>}
    </div>
  </div>;
}
function ChildPane({ lane, scope, child, reduced, order, connected, output, span, size, onSize }: { lane: string; scope: string; child: Shown; reduced: boolean; order: number; connected: boolean; output?: ChildOutput; span: number; size?: Size; onSize: (size: Size) => void }) {
  const pane = useRef<HTMLElement>(null);
  usePaneMotion(pane, !!child.leaving, reduced);
  useLayoutEffect(() => {
    const node = pane.current;
    return () => {
      if (node?.contains(document.activeElement) && useStore.getState().active === lane) {
        node.closest(".terminal-grid")?.querySelector(".terminal-grid-main")?.scrollIntoView({ block: "nearest" });
        focusMain(lane);
      }
    };
  }, [lane]);
  const status = connected ? child.status : "unknown", started = startedAt(child);
  return <section ref={pane} className={"child-terminal-pane" + (child.leaving ? " is-leaving" : "") + (size ? " is-" + size : "")} data-pane={child.id} data-child-invocation={child.id} data-status={child.leaving ? "leaving" : status} aria-hidden={child.leaving || undefined} style={span > 1 ? { gridColumn: "span " + span } : undefined} aria-label={nameOf(child) + " " + shortId(child) + " terminal"} onKeyDown={(event) => {
    if (event.key === "Escape") { event.preventDefault(); event.stopPropagation(); if (size === "max") onSize("max"); focusMain(lane); }
  }}>
    <div className="child-terminal-surface">
    <header className="terminal-pane-heading"><span className="terminal-pane-order">{order}</span><MemberAvatar name={child.member ?? "subagent"} size={16} working={!child.leaving && (status === "working" || status === "waiting")} /><strong>{nameOf(child)}</strong><small>{shortId(child)}</small><span className="terminal-pane-status" role="status">{child.leaving ? "Closing" : STATUS[status]}</span>
      <button className="terminal-pane-control" aria-pressed={size === "min"} aria-label={size === "min" ? "Restore pane" : "Minimize pane"} title={size === "min" ? "Restore" : "Minimize"} onClick={() => onSize("min")}><Icon name={size === "min" ? "chevron-down" : "minus"} size={12} /></button>
      <button className="terminal-pane-control" aria-pressed={size === "max"} aria-label={size === "max" ? "Restore pane" : "Maximize pane"} title={size === "max" ? "Restore (Esc)" : "Maximize"} onClick={() => onSize("max")}><Icon name={size === "max" ? "minimize-2" : "maximize"} size={12} /></button></header>
    <ChildOutputView scope={scope} child={child} output={output} />
    <footer className="terminal-pane-footer"><span>{child.activity || (child.leaving ? "Closing" : "Waiting for the first step…")}</span>{started && <small>started {started}</small>}<small>Read only</small></footer>
    </div>
    <div className="child-terminal-smoke" aria-hidden="true"><i /><i /><i /><span /></div>
  </section>;
}
function ChildOutputView({ scope, child, output }: { scope: string; child: Invocation; output?: ChildOutput }) {
  const { ref, scroll, latest } = useChildReading(scope, child.id, output), detail = output?.detail;
  const unavailable = (detail?.availability ?? child.output) === "unavailable";
  return <div className="child-terminal-content">
    {output?.disconnected && <p className="child-terminal-note" role="status">Output disconnected · reconnecting…</p>}
    {detail?.truncated && <p className="child-terminal-note">Showing recent output.</p>}
    <div ref={ref} className="child-terminal-output" tabIndex={0} aria-label={nameOf(child) + " live output"} onScroll={scroll}>
      {!detail?.items.length ? <p className="child-terminal-note">{unavailable ? "Live output is unavailable for this subagent." : child.activity || "Waiting for output…"}</p> : detail.items.map((item, index) => <OutputItem key={index} item={item} />)}
    </div>
    {output?.following === false && <button className="child-terminal-latest" onClick={latest}>Jump to latest</button>}
  </div>;
}
/** One line the way the CLI draws it: a bullet per message, a tool as its name and subject, and a
 * result folded to its first line with the rest a click away (a native disclosure, no state). */
function OutputItem({ item }: { item: { kind: string; text: string } }) {
  if (item.kind === "tool") {
    const space = item.text.indexOf(" "), tool = space < 0 ? item.text : item.text.slice(0, space), subject = space < 0 ? "" : item.text.slice(space + 1);
    return <div className="child-line child-tool"><span className="child-mark">●</span><span><b>{tool}</b>{subject && <>({subject})</>}</span></div>;
  }
  if (item.kind === "result") {
    const lines = item.text.replace(/\s+$/, "").split("\n"), more = lines.length - 1;
    if (more < 1) return <div className="child-line child-result"><span className="child-mark">⎿</span><span className="child-code">{lines[0]}</span></div>;
    return <details className="child-line child-result"><summary><span className="child-mark">⎿</span><span className="child-code">{lines[0]}</span><small>… +{more} line{more === 1 ? "" : "s"}</small></summary><pre className="child-code">{lines.slice(1).join("\n")}</pre></details>;
  }
  return <div className="child-line child-text"><span className="child-mark">●</span><div className="child-prose"><Markdown text={item.text} /></div></div>;
}
