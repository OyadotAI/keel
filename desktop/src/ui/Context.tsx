// A session inspector: reported occupancy, known sources, and evidence of skill use.
import { useCallback, useEffect, useRef, useState } from "react";
import { useShallow } from "zustand/react/shallow";
import { revealItemInDir } from "@tauri-apps/plugin-opener";
import { get, type Endpoint } from "../api";
import { contextMatches, evidenceLabel, type ContextSnapshot, type ContextSource, type ContextSkill } from "../context";
import { AGENT_NAME, useStore } from "../store";
import { tilde } from "../hooks";
import { count } from "./Turn";
import { Icon } from "./icons";
import "./Context.css";
import { ContextUsage } from "./ContextUsage";
import { usageSummary } from "../session-usage";

const amount = (n?: number | null) => n == null ? "—" : count(n);
const time = (at?: string | null) => at && Number.isFinite(Date.parse(at)) ? new Date(at).toLocaleTimeString([], { hour: "2-digit", minute: "2-digit" }) : "Not reported";

export function ContextSourceView({ source, ep, snapshot, wt }: { source: ContextSource; ep: Endpoint; snapshot: ContextSnapshot; wt?: string }) {
  const [preview, setPreview] = useState<{ text: string; path: string; estimate: number }>();
  const [error, setError] = useState("");
  const [busy, setBusy] = useState(false);
  const [show, setShow] = useState(false);
  async function open() {
    setShow(!show);
    if (preview || show || busy) return;
    setBusy(true); setError("");
    try { setPreview(await get(ep, "/api/context/source", { provider: snapshot.provider, session: snapshot.session, path: source.path, wt })); }
    catch (e) { setError(String(e)); }
    finally { setBusy(false); }
  }
  return <div className="context-source">
    <button className="context-source-button" onClick={() => void open()} aria-expanded={show} title={source.path}>
      <Icon name="file" size={14} /><span className="context-path">{tilde(source.path)}</span>
      <span className="context-number" title={source.tokens == null ? "Approximation: UTF-8 file bytes divided by four; not a measured contribution to context" : "Provider-reported token estimate"}>{source.tokens != null ? amount(source.tokens) : source.estimate != null ? `~${amount(source.estimate)}` : "—"}</span>
    </button>
    <div className="context-source-meta">{source.source && <span>{source.source}</span>}<span>{source.state === "configured" ? "On disk · loading unconfirmed" : source.state === "historical" ? "Observed before compaction" : source.state === "recalled" ? "Recalled in this session" : source.state === "read" ? "Read observed" : "Reported in context"}</span></div>
    {show && <div className="context-preview">
      {busy && <p className="muted" role="status">Reading source…</p>}
      {error && <p className="context-error" role="alert">{error}</p>}
      {preview && <><div className="context-preview-actions"><span>Read-only preview</span><button className="link" onClick={() => void revealItemInDir(preview.path).catch((e) => setError(String(e)))}>Reveal file ↗</button></div><pre>{preview.text}</pre></>}
    </div>}
  </div>;
}

function Skill({ skill, snapshot, ep, wt }: { skill: ContextSkill; snapshot: ContextSnapshot; ep: Endpoint; wt?: string }) {
  const events = snapshot.evidence.filter((e) => e.name === skill.name && (!e.path || !skill.path || e.path === skill.path));
  const last = events.at(-1);
  const active = events.some((e) => ["invocation", "read"].includes(e.kind));
  const status = last ? evidenceLabel(last.kind) : skill.advertised ? "Description in context" : skill.available === false ? "Disabled" : "Available";
  return <details className="context-skill">
    <summary><span className={`context-skill-dot${active ? " used" : ""}`} /><span className="context-skill-name">{skill.name}</span><span className="context-tag">{status}</span></summary>
    <div className="context-skill-body">
      <p>{skill.description || "No trigger description was reported."}</p>
      <div className="context-source-meta"><span>{skill.source || "Source not reported"}</span>{skill.user_only && <span>Explicit command only</span>}{skill.tokens != null && <span>{amount(skill.tokens)} description tokens</span>}</div>
      {!events.length && <p className="muted">No invocation or file-read evidence captured. Availability does not mean the full skill was loaded.</p>}
      {events.length > 0 && <ol className="context-evidence">{events.slice(-20).map((e) => <li key={e.key}>
        <div><strong>{evidenceLabel(e.kind)}</strong><span>{time(e.at)}</span></div>
        <p>{e.detail || "Selection reason not reported."}</p>
        {(e.turn || e.member) && <div className="context-source-meta">{e.member && <span>Member: {e.member}</span>}{e.turn && <span title={e.turn}>Turn {e.turn.slice(0, 12)}</span>}</div>}
        {snapshot.compacted_at && e.at && e.at < snapshot.compacted_at && <p className="muted">Before last compaction · current presence unconfirmed</p>}
      </li>)}</ol>}
      {skill.path && <ContextSourceView source={{ path: skill.path, source: skill.source, state: "configured", estimate: skill.estimate }} ep={ep} snapshot={snapshot} wt={wt} />}
    </div>
  </details>;
}

function useSessionContext(lane: string) {
  const l = useStore(useShallow((s) => {
    const current = s.lanes[lane];
    if (!current) return undefined;
    const { context, agent, session, known, project, wt, interface: surface, elsewhere, disconnected } = current;
    return { context, agent, session, known, project, wt, interface: surface, elsewhere, disconnected };
  }));
  const ep = useStore((s) => l ? s.projects[l.project]?.endpoint : undefined);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState("");
  const sequence = useRef(0);
  const snapshot = contextMatches(l?.context, l?.agent ?? "", l?.known ? l.session : undefined) ? l?.context : undefined;
  const session = l?.agent === "codex" && !l.known ? undefined : l?.session;
  const provider = l?.agent;
  const wt = l?.wt;
  const terminal = l?.interface === "terminal" || l?.elsewhere;
  const revision = useStore((s) => {
    const current = s.lanes[lane];
    if (current?.interface !== "terminal" && !current?.elsewhere) return "";
    const last = current.conv.turns.at(-1);
    return `${last?.key}:${last?.usage?.context}:${last?.ended}:${Object.keys(last?.calls ?? {}).length}`;
  });
  const refresh = useCallback(async (live = false) => {
    if (!ep || !provider || provider === "antigravity") return;
    const request = ++sequence.current;
    setBusy(true); setError("");
    try {
      const next = await get<ContextSnapshot>(ep, "/api/context", { lane, provider, session, wt, refresh: live && !terminal });
      if (sequence.current !== request) return;
      useStore.setState((s) => {
        const current = s.lanes[lane];
        if (!current || current.agent !== provider || (current.known ? current.session !== next.session : !!next.session)) return s;
        // A live event received during the request is newer than its cached response.
        if (current.context?.session === next.session && current.context.updated_at > next.updated_at) return s;
        return { lanes: { ...s.lanes, [lane]: { ...current, context: next } } };
      });
    } catch (e) { if (sequence.current === request) setError(String(e)); }
    finally { if (sequence.current === request) setBusy(false); }
  }, [ep, lane, provider, session, wt, terminal]);
  useEffect(() => { const requests = sequence; void refresh(true); return () => { requests.current++; }; }, [refresh]);
  useEffect(() => {
    if (!terminal) return;
    const timer = setTimeout(() => void refresh(), 350);
    return () => clearTimeout(timer);
  }, [revision, terminal, refresh]);
  return { l, ep, snapshot, wt, busy, error, refresh };
}

function Skills({ snapshot, ep, wt }: { snapshot: ContextSnapshot; ep: Endpoint; wt?: string }) {
  const [filter, setFilter] = useState("");
  const [allSkills, setAllSkills] = useState(false);
  const skills = [...(snapshot?.skills ?? [])];
  for (const e of snapshot?.evidence ?? []) if (e.name && !skills.some((s) => s.name === e.name)) skills.push({ name: e.name, path: e.path });
  const usedSkills = new Set(snapshot?.evidence.filter((e) => ["invocation", "read"].includes(e.kind)).map((e) => e.name));
  const matching = skills.filter((s) => !filter || `${s.name} ${s.description ?? ""}`.toLowerCase().includes(filter.toLowerCase())).sort((a, b) => Number(usedSkills.has(b.name)) - Number(usedSkills.has(a.name)) || a.name.localeCompare(b.name));
  return <>
    <details className="context-section" open><summary><span>Skills</span><span className="context-section-count">{usedSkills.size} used · {skills.length} known</span></summary><div className="context-section-body">
      <p className="context-note">Descriptions help the agent choose a skill. Full instructions load separately; observed use appears first.</p>
      {skills.length > 10 && <input className="context-search" type="search" aria-label="Filter skills" placeholder="Find a skill…" value={filter} onChange={(e) => { setFilter(e.target.value); setAllSkills(false); }} />}
      {snapshot && ep && matching.slice(0, allSkills ? matching.length : 10).map((s) => <Skill key={`${s.name}:${s.path ?? ""}`} skill={s} snapshot={snapshot} ep={ep} wt={wt} />)}
      {matching.length > 10 && <button className="link context-show-more" onClick={() => setAllSkills(!allSkills)}>{allSkills ? "Show less" : `Show ${matching.length - 10} more`}</button>}
      {!matching.length && <p className="context-note">{filter ? "No matching skills." : "No skill inventory or activity was reported."}</p>}
    </div></details>
  </>;
}

function MemoryActivity({ snapshot }: { snapshot: ContextSnapshot }) {
  const events = snapshot.evidence.filter((e) => e.kind === "memory_recall" || e.kind === "memory_read");
  if (!events.length) return null;
  return <details className="context-memory-activity"><summary>Recall & read activity <span className="muted">{events.length}</span></summary>
    <ol className="context-evidence">{events.slice(-10).map((e) => <li key={e.key}><div><strong>{evidenceLabel(e.kind)}</strong><span>{time(e.at)}</span></div><p>{e.detail}</p>{e.path && <p className="context-path">{tilde(e.path)}</p>}{e.member && <p className="muted">Member: {e.member}</p>}</li>)}</ol>
  </details>;
}

/** The footer's exact same definitions and totals, readable without hovering. */
function ReportedUsage({ lane }: { lane: string }) {
  const detail = useStore((s) => s.lanes[lane] ? usageSummary(s.lanes[lane]).detail : "");
  return <details className="context-section"><summary>Reported usage</summary><div className="context-section-body">{detail.split("\n").slice(1).map((line) => <p className="context-note" key={line}>{line}</p>)}</div></details>;
}

export function Context({ lane }: { lane: string }) {
  const { l, ep, snapshot, wt, busy, error, refresh } = useSessionContext(lane);
  if (!l) return null;
  return <div className="context-panel">
    <header className="context-heading"><div><h2>Context</h2><p>{AGENT_NAME[l.agent ?? "claude"]}{snapshot?.model && <> · {snapshot.model}</>}</p></div><button className="ghost" disabled={busy || !ep} onClick={() => void refresh(true)} title="Refresh context without sending a message">{busy ? <span className="busy-dot" /> : <Icon name="rewind" size={15} />}<span>Refresh</span></button></header>
    {error && <p className="context-error" role="alert">{error}</p>}
    <ReportedUsage lane={lane} />
    <ContextUsage snapshot={snapshot} busy={busy} disconnected={!!l.disconnected} />
    <details className="context-section" open><summary><span>Memory & instructions</span><span className="context-section-count">{snapshot?.memory.length ?? 0}</span></summary><div className="context-section-body">
      {snapshot?.memory.length && ep ? snapshot.memory.map((s) => <ContextSourceView key={s.path} source={s} ep={ep} snapshot={snapshot} wt={wt} />) : <p className="context-note">No memory sources were reported. This does not establish that the session has no memory.</p>}
      {snapshot && <MemoryActivity snapshot={snapshot} />}
    </div></details>
    {snapshot && ep && <Skills snapshot={snapshot} ep={ep} wt={wt} />}
    {snapshot?.compacted_at && <p className="context-note">Last compaction: {time(snapshot.compacted_at)}. Earlier activity is historical evidence.</p>}
    {!!snapshot?.limitations.length && <details className="context-section context-limits"><summary>Data availability</summary><ul>{snapshot.limitations.map((s) => <li key={s}>{s}</li>)}</ul></details>}
  </div>;
}
