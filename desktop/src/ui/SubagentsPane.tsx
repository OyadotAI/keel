// Active children occupy temporary panes while the parent PTY stays mounted and focused.
import { useEffect, useLayoutEffect, useRef, useState, useSyncExternalStore, type CSSProperties, type ReactNode } from "react";
import { AGENT_NAME, useStore } from "../store";
import { terminalLease } from "../typers";
import { childrenOf, childScope, useChildPane, useChildReading, releaseGridText, SHORT_ID, type Invocation, type ChildOutput } from "../subagents";
import { gridColumns, useLiveInvocations, useVisibleChildren } from "../subagents";
import { useGridOutput } from "../subagent-output";
import { onTerminalTheme, terminalDark } from "../terminal-theme";
import { onTerminalFont, terminalFont } from "../zoom";
import { MemberAvatar } from "./MemberAvatar";
import "./SubagentsPane.css";
const shortId = (child: Invocation) => child.id.split(":").at(-1)?.slice(0, SHORT_ID) ?? "";
const nameOf = (child: Invocation) => child.member || "Subagent";
const STATUS = { working: "Working", waiting: "Waiting", unknown: "Reconnecting · status unconfirmed", done: "Done", failed: "Failed", cancelled: "Cancelled", interrupted: "Interrupted" };
/** Grid changes never submit, restart or close an agent process. */
export function SubagentsPane({ lane, hidden, enabled = true, children }: { lane: string; hidden: boolean; enabled?: boolean; children?: ReactNode }) {
  const current = useStore((s) => s.lanes[lane]), ep = useStore((s) => current ? s.projects[current.project]?.endpoint : undefined);
  const scope = childScope(current), snapshot = enabled && current ? childrenOf(current) : undefined;
  const live = useLiveInvocations(scope, snapshot), active = enabled ? live : [];
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
  const total = active.length + 1, columns = gridColumns(width, total), rows = Math.ceil(total / columns);
  const style = { "--terminal-columns": columns, "--terminal-rows": rows, "--child-terminal-font": font + "px", "--child-bg": dark ? "#141410" : "#fafaf8", "--child-fg": dark ? "#f4f3ef" : "#1c1c1a" } as CSSProperties;
  const returnToMain = () => { main.current?.scrollIntoView({ block: "nearest" }); terminalLease(lane)?.focus(); };
  return <div className={"terminal-grid-shell" + (active.length ? " split" : "")} style={style}>
    <div ref={grid} className="terminal-grid" onScroll={(event) => setScrolled(event.currentTarget.scrollTop > 0)}>
      <section key="main" ref={main} className="terminal-grid-main" aria-label="Main terminal">
        {active.length > 0 && <header className="terminal-pane-heading"><strong>{AGENT_NAME[current?.agent ?? "claude"]} · Main</strong></header>}
        {children}
      </section>
      {active.map((child, index) => <ChildPane key={scope + ":" + child.id} lane={lane} scope={scope} child={child} connected={!!snapshot?.connected} output={view.outputs[child.id]} last={index === active.length - 1} span={total % columns ? columns - total % columns + 1 : 1} />)}
    </div>
    {active.length > 0 && scrolled && <button className="terminal-grid-return" onClick={returnToMain}>Main terminal</button>}
  </div>;
}
function ChildPane({ lane, scope, child, connected, output, last, span }: { lane: string; scope: string; child: Invocation; connected: boolean; output?: ChildOutput; last: boolean; span: number }) {
  const pane = useRef<HTMLElement>(null);
  useLayoutEffect(() => {
    const node = pane.current;
    return () => {
      if (node?.contains(document.activeElement) && useStore.getState().active === lane) {
        node.closest(".terminal-grid")?.querySelector(".terminal-grid-main")?.scrollIntoView({ block: "nearest" });
        terminalLease(lane)?.focus();
      }
    };
  }, [lane]);
  const status = connected ? child.status : "unknown";
  return <section ref={pane} className="child-terminal-pane" data-child-invocation={child.id} data-reference-target="" style={last ? { gridColumn: "span " + span } : undefined} aria-label={nameOf(child) + " " + shortId(child) + " terminal"} onKeyDown={(event) => {
    if (event.key === "Escape") { event.preventDefault(); event.stopPropagation(); terminalLease(lane)?.focus(); }
  }}>
    <header className="terminal-pane-heading"><MemberAvatar name={child.member ?? "subagent"} size={16} working={status === "working" || status === "waiting"} /><strong>{nameOf(child)}</strong><small>{shortId(child)}</small><span className="terminal-pane-status" role="status">{STATUS[status]}</span><small>Read only</small></header>
    <ChildOutputView scope={scope} child={child} output={output} />
  </section>;
}
function ChildOutputView({ scope, child, output }: { scope: string; child: Invocation; output?: ChildOutput }) {
  const { ref, scroll, latest } = useChildReading(scope, child.id, output), detail = output?.detail;
  const unavailable = (detail?.availability ?? child.output) === "unavailable";
  return <div className="child-terminal-content">
    {output?.disconnected && <p className="child-terminal-note" role="status">Output disconnected · reconnecting…</p>}
    {detail?.truncated && <p className="child-terminal-note">Showing recent output.</p>}
    <div ref={ref} className="child-terminal-output" tabIndex={0} aria-label={nameOf(child) + " live output"} onScroll={scroll}>
      {!detail?.items.length ? <p className="child-terminal-note">{unavailable ? "Live output is unavailable for this subagent." : child.activity || "Waiting for output…"}</p> : detail.items.map((item, index) => <pre key={index} className={item.kind === "tool" ? "child-terminal-tool" : undefined}>{item.text}</pre>)}
    </div>
    {output?.following === false && <button className="child-terminal-latest" onClick={latest}>Jump to latest</button>}
  </div>;
}
