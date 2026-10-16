import { useEffect, useMemo, useState, type ReactNode } from "react";
import { get, type Endpoint } from "../api";
import { commitGraph, type GraphCommit } from "../commit-graph";
import { Icon } from "./icons";

const COLORS = ["var(--graph-blue)", "var(--graph-purple)", "var(--graph-orange)", "var(--graph-green)", "var(--graph-pink)", "var(--graph-cyan)"];
const x = (column: number) => 12 + column * 13;

export function CommitGraph({ ep, wt, revision, onOpen, tools }: { ep: Endpoint; wt?: string; revision: unknown; onOpen: (c: GraphCommit) => void; tools: ReactNode }) {
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
  const columns = Math.max(1, ...rows.map((r) => r.width));
  return <section className={`commit-graph${open ? "" : " collapsed"}`} aria-label="Commit graph">
    <div className="graph-heading">
      <button className="graph-toggle" aria-expanded={open} onClick={() => setOpen(!open)}><Icon name={open ? "chevron-down" : "chevron-right"} size={12} />Graph</button>
      <select aria-label="Graph branches" value={all ? "all" : "current"} onChange={(e) => setAll(e.target.value === "all")}><option value="all">All branches</option><option value="current">Current branch</option></select>
      {tools}
      <button className="ghost" title="Refresh graph" aria-label="Refresh commit graph" onClick={() => setRetry((n) => n + 1)}><Icon name="rewind" size={13} /></button>
    </div>
    {open && <div className="graph-scroll">
      {error ? <div className="graph-message" role="alert">{error}<button onClick={() => setRetry((n) => n + 1)}>Retry</button></div> : !commits ? <div className="graph-message">Reading commit history…</div> : !rows.length ? <div className="graph-message">No commits yet. Stage changes and create your first commit.</div> : <div role="group" aria-label="Commits">
        {rows.map(({ commit, column, color, edges }) => <button className="graph-row" key={commit.sha} title={`${commit.subject}\n${commit.sha}\n${commit.author} · ${new Date(commit.when * 1000).toLocaleString()}\n${commit.refs.join(", ")}`} onClick={() => onOpen(commit)}>
          <svg width={x(columns)} height={28} aria-hidden="true" style={{ flex: "none" }}>
            {edges.map((edge, i) => {
              const y1 = edge.half === "bottom" ? 14 : 0;
              const y2 = edge.half === "top" ? 14 : 28;
              return <path key={i} d={`M${x(edge.from)},${y1} C${x(edge.from)},${(y1 + y2) / 2} ${x(edge.to)},${(y1 + y2) / 2} ${x(edge.to)},${y2}`} fill="none" stroke={COLORS[edge.color % COLORS.length]} strokeWidth={1.5} />;
            })}
            <circle cx={x(column)} cy={14} r={commit.parents.length > 1 ? 4 : 3.5} fill="var(--bg)" stroke={COLORS[color % COLORS.length]} strokeWidth={2} />
            {commit.refs.some((r) => r === "HEAD" || r.startsWith("HEAD ->")) && <circle cx={x(column)} cy={14} r={6.5} fill="none" stroke={COLORS[color % COLORS.length]} />}
          </svg>
          <span className="graph-subject">{commit.subject}</span>
          {commit.refs.slice(0, 2).map((ref) => <span key={ref} className={`graph-ref${ref.startsWith("HEAD") ? " current" : ref.includes("refs/remotes/") ? " remote" : ""}`}><Icon name="branch" size={11} />{ref.replace(/^HEAD -> /, "").replace(/refs\/(heads|remotes|tags)\//, "")}</span>)}
          {commit.refs.length > 2 && <small className="faint">+{commit.refs.length - 2}</small>}
        </button>)}
        {commits.length >= limit && <div className="graph-message">{limit < 200 ? <button onClick={() => setLimit(200)}>Load older commits</button> : "Showing the latest 200 commits. Lines continue to older parents."}</div>}
      </div>}
    </div>}
  </section>;
}
