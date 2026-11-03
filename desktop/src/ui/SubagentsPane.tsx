// Active children occupy temporary panes while the parent PTY stays mounted and focused.
// A child pane is read-only, so it carries no drop target of its own: a file dropped on it
// goes to the lane (the `.terminal` ancestor), the one composer this surface has.
import { useEffect, useLayoutEffect, useRef, useState, useSyncExternalStore, type CSSProperties, type ReactNode } from "react";
import { AGENT_NAME, useStore } from "../store";
import { terminalLease } from "../typers";
import { childrenOf, childScope, useChildPane, useChildReading, releaseGridText, SHORT_ID, type Invocation, type ChildOutput, type Shown } from "../subagents";
import { gridColumns, useDeparting, useFlip, useLiveInvocations, useVisibleChildren, usePaneMotion, useReducedMotion, PANE_OPEN_MS, PANE_CLOSE_MS, PANE_MOVE_MS } from "../subagents";
import { useGridOutput } from "../subagent-output";
import { onTerminalTheme, terminalDark } from "../terminal-theme";
import { terminalPaneColors } from "../terminal-palette";
import { onTerminalFont, terminalFont } from "../zoom";
import { MemberAvatar } from "./MemberAvatar";
import { Markdown } from "./Markdown";
import "./SubagentsPane.css";
const shortId = (child: Invocation) => child.id.split(":").at(-1)?.slice(0, SHORT_ID) ?? "";
const nameOf = (child: Invocation) => child.member || "Subagent";
const STATUS = { working: "Working", waiting: "Waiting", unknown: "Reconnecting · status unconfirmed", done: "Done", failed: "Failed", cancelled: "Cancelled", interrupted: "Interrupted" };
/** The clock a pane opened at, or nothing: a provider that reports no start time gets no guess. */
const startedAt = (child: Invocation) => { const at = child.started ? new Date(child.started) : undefined; return at && !Number.isNaN(at.getTime()) ? at.toLocaleTimeString([], { hour: "2-digit", minute: "2-digit" }) : undefined; };
/** Grid changes never submit, restart or close an agent process. */
export function SubagentsPane({ lane, hidden, enabled = true, children }: { lane: string; hidden: boolean; enabled?: boolean; children?: ReactNode }) {
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
  const total = shown.length + 1, columns = gridColumns(width, total), rows = Math.ceil(total / columns);
  useFlip(grid, shown.map((child) => child.id).join() + "/" + columns, reduced);
  const style = { ...terminalPaneColors(dark), "--terminal-columns": columns, "--terminal-rows": rows, "--child-terminal-font": font + "px", "--pane-open-ms": PANE_OPEN_MS + "ms", "--pane-close-ms": PANE_CLOSE_MS + "ms", "--pane-move-ms": PANE_MOVE_MS + "ms" } as CSSProperties;
  const returnToMain = () => { main.current?.scrollIntoView({ block: "nearest" }); terminalLease(lane)?.focus(); };
  const agentName = current?.agent === "claude" ? "Claude Code" : AGENT_NAME[current?.agent ?? "claude"];
  return <div className={"terminal-grid-shell" + (shown.length ? " split" : "")} style={style}>
    <div ref={grid} className="terminal-grid" onScroll={(event) => setScrolled(event.currentTarget.scrollTop > 0)}>
      <section key="main" ref={main} className="terminal-grid-main" data-pane="main" aria-label="Main terminal">
        <div className="terminal-main-viewport" data-main-viewport>
        <header aria-hidden={!shown.length} className={"terminal-pane-heading main-pane-chrome" + (shown.length ? " is-visible" : "")}><span className="terminal-pane-order">1</span><strong>{agentName} · Main</strong><small>you type here</small></header>
        <div className="terminal-main-content" data-main-content>{children}</div>
        <footer aria-hidden={!shown.length} className={"terminal-pane-footer main-pane-chrome" + (shown.length ? " is-visible" : "")}><span>{active.length} subagent{active.length === 1 ? "" : "s"} working</span><small>Esc in a pane returns here</small></footer>
        </div>
      </section>
      {shown.map((child, index) => <ChildPane key={scope + ":" + child.id} lane={lane} scope={scope} child={child} reduced={reduced} order={index + 2} connected={!!snapshot?.connected} output={view.outputs[child.id]} last={index === shown.length - 1} span={total % columns ? columns - total % columns + 1 : 1} />)}
    </div>
    {shown.length > 0 && scrolled && <button className="terminal-grid-return" onClick={returnToMain}>Main terminal</button>}
  </div>;
}
function ChildPane({ lane, scope, child, reduced, order, connected, output, last, span }: { lane: string; scope: string; child: Shown; reduced: boolean; order: number; connected: boolean; output?: ChildOutput; last: boolean; span: number }) {
  const pane = useRef<HTMLElement>(null);
  usePaneMotion(pane, !!child.leaving, reduced);
  useLayoutEffect(() => {
    const node = pane.current;
    return () => {
      if (node?.contains(document.activeElement) && useStore.getState().active === lane) {
        node.closest(".terminal-grid")?.querySelector(".terminal-grid-main")?.scrollIntoView({ block: "nearest" });
        terminalLease(lane)?.focus();
      }
    };
  }, [lane]);
  const status = connected ? child.status : "unknown", started = startedAt(child);
  return <section ref={pane} className={"child-terminal-pane" + (child.leaving ? " is-leaving" : "")} data-pane={child.id} data-child-invocation={child.id} aria-hidden={child.leaving || undefined} style={last ? { gridColumn: "span " + span } : undefined} aria-label={nameOf(child) + " " + shortId(child) + " terminal"} onKeyDown={(event) => {
    if (event.key === "Escape") { event.preventDefault(); event.stopPropagation(); terminalLease(lane)?.focus(); }
  }}>
    <div className="child-terminal-surface">
    <header className="terminal-pane-heading"><span className="terminal-pane-order">{order}</span><MemberAvatar name={child.member ?? "subagent"} size={16} working={!child.leaving && (status === "working" || status === "waiting")} /><strong>{nameOf(child)}</strong><small>{shortId(child)}</small><span className="terminal-pane-status" role="status">{child.leaving ? "Closing" : STATUS[status]}</span></header>
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
