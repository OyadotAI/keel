import { useEffect, useMemo, useRef, useState, type ReactNode } from "react";
import { get, type Endpoint } from "../api";
import { commitGraph, type GraphCommit, type GraphRow } from "../commit-graph";
import { Icon } from "./icons";

const COLORS = ["var(--graph-blue)", "var(--graph-purple)", "var(--graph-orange)", "var(--graph-green)", "var(--graph-pink)", "var(--graph-cyan)"];
const x = (column: number) => 12 + column * 13;

export function CommitGraph({ ep, wt, revision, onOpen, tools, selected }: { ep: Endpoint; wt?: string; revision: unknown; onOpen: (c: GraphCommit) => void; tools: ReactNode; selected?: string }) {
  const [commits, setCommits] = useState<GraphCommit[]>();
  const [error, setError] = useState<string>();
  const [all, setAll] = useState(true);
  const [open, setOpen] = useState(true);
  const [limit, setLimit] = useState(100);
  const [retry, setRetry] = useState(0);
  useEffect(() => {
    let live = true;
    setError(undefined);
    get<GraphCommit[]>(ep, "/api/git/graph", { wt, n: limit, all: String(all) }).then((data) => {
      if (live) setCommits(data);
    }, (e) => { if (live) setError(String(e)); });
    return () => { live = false; };
  }, [ep, wt, revision, all, limit, retry]);
  const rows = useMemo(() => commitGraph(commits ?? []), [commits]);
  return <section className={`commit-graph${open ? "" : " collapsed"}`} aria-label="Commit graph">
    <div className="graph-heading">
      <button className="graph-toggle" aria-expanded={open} onClick={() => setOpen(!open)}><Icon name={open ? "chevron-down" : "chevron-right"} size={12} />Graph</button>
      <select aria-label="Graph branches" value={all ? "all" : "current"} onChange={(e) => setAll(e.target.value === "all")}><option value="all">All branches</option><option value="current">Current branch</option></select>
      {tools}
      <button className="ghost" title="Refresh graph" aria-label="Refresh commit graph" onClick={() => setRetry((n) => n + 1)}><Icon name="rewind" size={13} /></button>
    </div>
    {open && <>
      {error ? <div className="graph-message" role="alert">{error}<button onClick={() => setRetry((n) => n + 1)}>Retry</button></div> : !commits ? <div className="graph-message">Reading commit history…</div> : !rows.length ? <div className="graph-message">No commits yet. Stage changes and create your first commit.</div> : <GraphHistory rows={rows} onOpen={onOpen} selected={selected} />}
      {commits && commits.length >= limit && <div className="graph-history-limit">{limit < 200 ? <button className="link" onClick={() => setLimit(200)}>Load older commits</button> : "Showing the latest 200 commits. Lines continue to older parents."}</div>}
    </>}
  </section>;
}

/** The graph can grow horizontally without taking space away from commit messages. */
function GraphHistory({ rows, onOpen, selected }: { rows: GraphRow[]; onOpen: (c: GraphCommit) => void; selected?: string }) {
  const lanes = useRef<HTMLDivElement>(null);
  const commits = useRef<HTMLDivElement>(null);
  const width = x(Math.max(1, ...rows.map((r) => r.width)));
  useEffect(() => {
    const graph = lanes.current;
    if (!graph) return;
    const observer = new ResizeObserver(() => {
      // Keep both viewports equally tall when the platform reserves a horizontal scrollbar.
      if (commits.current) commits.current.style.marginBottom = `${graph.offsetHeight - graph.clientHeight}px`;
    });
    observer.observe(graph);
    return () => observer.disconnect();
  }, []);
  function sync(top: number, target: HTMLDivElement | null) {
    if (target && target.scrollTop !== top) target.scrollTop = top;
  }
  return <div className="graph-history" style={{ gridTemplateColumns: `min(${width}px, 30%) minmax(0, 1fr)` }}>
    <div className="graph-lanes" ref={lanes} role="region" aria-label="Branch graph; scroll horizontally to see more branches" tabIndex={0} onWheel={(e) => {
      if (!e.shiftKey && Math.abs(e.deltaY) > Math.abs(e.deltaX) && commits.current) commits.current.scrollTop += e.deltaY;
    }} onKeyDown={(e) => moveGraph(e, commits.current)}>
      <svg width={width} height={rows.length * 28} aria-hidden="true">
        {rows.map((row, i) => <g key={row.commit.sha} transform={`translate(0, ${i * 28})`} onClick={() => onOpen(row.commit)}>
          <rect width={width} height={28} fill={row.commit.sha === selected ? "var(--tint)" : "transparent"} />
          <GraphNode row={row} />
        </g>)}
      </svg>
    </div>
    <div className="graph-commits" ref={commits} role="group" aria-label="Commits" onScroll={(e) => sync(e.currentTarget.scrollTop, lanes.current)} onKeyDown={walkCommits}>
      {rows.map(({ commit }) => <button className="graph-row" key={commit.sha} aria-pressed={commit.sha === selected} title={`${commit.subject}\n${commit.sha}\n${commit.author} · ${new Date(commit.when * 1000).toLocaleString()}\n${commit.refs.join(", ")}`} onClick={() => onOpen(commit)}>
        <span className="graph-subject">{commit.subject}</span>
        <span className="graph-labels">{commit.refs.slice(0, 2).map((ref) => <span key={ref} className={`graph-ref${ref.startsWith("HEAD") ? " current" : ref.includes("refs/remotes/") ? " remote" : ""}`}><Icon name="branch" size={11} /><span>{ref.replace(/^HEAD -> /, "").replace(/refs\/(heads|remotes|tags)\//, "")}</span></span>)}</span>
        {commit.refs.length > 2 && <small className="faint">+{commit.refs.length - 2}</small>}
      </button>)}
    </div>
  </div>;
}

function GraphNode({ row: { commit, column, color, edges } }: { row: GraphRow }) {
  return <>
    {edges.map((edge, i) => {
      const y1 = edge.half === "bottom" ? 14 : 0;
      const y2 = edge.half === "top" ? 14 : 28;
      return <path key={i} d={`M${x(edge.from)},${y1} C${x(edge.from)},${(y1 + y2) / 2} ${x(edge.to)},${(y1 + y2) / 2} ${x(edge.to)},${y2}`} fill="none" stroke={COLORS[edge.color % COLORS.length]} strokeWidth={1.5} />;
    })}
    <circle cx={x(column)} cy={14} r={commit.parents.length > 1 ? 4 : 3.5} fill="var(--bg)" stroke={COLORS[color % COLORS.length]} strokeWidth={2} />
    {commit.refs.some((r) => r === "HEAD" || r.startsWith("HEAD ->")) && <circle cx={x(column)} cy={14} r={6.5} fill="none" stroke={COLORS[color % COLORS.length]} />}
  </>;
}

function walkCommits(e: React.KeyboardEvent<HTMLDivElement>) {
  if (e.metaKey || e.ctrlKey || e.altKey || !["ArrowDown", "ArrowUp", "Home", "End"].includes(e.key)) return;
  const nodes = [...e.currentTarget.querySelectorAll<HTMLButtonElement>(".graph-row")];
  const at = nodes.indexOf(document.activeElement as HTMLButtonElement);
  if (at < 0) return;
  e.preventDefault();
  const next = e.key === "Home" ? 0 : e.key === "End" ? nodes.length - 1 : at + (e.key === "ArrowDown" ? 1 : -1);
  nodes[Math.max(0, Math.min(nodes.length - 1, next))]?.focus();
}

function moveGraph(e: React.KeyboardEvent<HTMLDivElement>, commits: HTMLDivElement | null) {
  if (e.metaKey || e.ctrlKey || e.altKey) return;
  if (e.key === "ArrowLeft" || e.key === "ArrowRight") {
    e.preventDefault(); e.currentTarget.scrollLeft += e.key === "ArrowRight" ? 52 : -52;
  } else if (commits) {
    const top: Record<string, number> = { ArrowDown: commits.scrollTop + 28, ArrowUp: commits.scrollTop - 28, PageDown: commits.scrollTop + commits.clientHeight, PageUp: commits.scrollTop - commits.clientHeight, Home: 0, End: commits.scrollHeight };
    if (e.key in top) { e.preventDefault(); commits.scrollTop = top[e.key]; }
  }
}
