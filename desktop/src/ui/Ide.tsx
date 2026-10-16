import { useEffect, useMemo, useRef, useState, type ReactNode, type RefObject } from "react";
import { Virtuoso, type VirtuosoHandle } from "react-virtuoso";
import { get } from "../api";
import { prepare, type DiffResponse, type Prepared } from "../git";
import { completedEdits, EMPTY_IDE, liveReader, relativeFile, useIde, visibleTree, type FileNode, type IdeLane } from "../ide";
import { useStore } from "../store";
import { SyntaxText } from "./SyntaxText";
import { CommitReview } from "./Git";
import { Icon } from "./icons";

export function LayoutSwitch() {
  const layout = useIde((s) => s.layout);
  return <div className="segmented layout-switch" role="group" aria-label="Workspace layout">
    {(["chat", "ide"] as const).map((value) => <button key={value} aria-pressed={layout === value} onClick={() => {
      useIde.getState().setLayout(value);
      requestAnimationFrame(() => window.dispatchEvent(new Event("resize")));
    }}>{value === "ide" ? "Coding Mode" : "Agentic Mode"}</button>)}
  </div>;
}

function useCheckout(lane: string) {
  const project = useStore((s) => s.lanes[lane]?.project);
  const ep = useStore((s) => project ? s.projects[project]?.endpoint : undefined);
  const wt = useStore((s) => s.lanes[lane]?.wt);
  const isolated = useStore((s) => s.lanes[lane]?.isolated);
  const revision = useStore((s) => s.lanes[lane]?.reviewRevision ?? 0);
  const error = useStore((s) => project ? s.projects[project]?.error : undefined);
  return { ep, wt, revision, root: wt ? `${project}/.keel/worktrees/${wt}` : project ?? "", ready: !!ep && (!isolated || !!wt),
    waiting: error ? `Could not connect: ${error}` : isolated && !wt ? "This lane’s checkout will be available when its first turn starts." : "Connecting to the project…" };
}

export function Explorer({ lane, enabled, showCode }: { lane: string; enabled: boolean; showCode?: () => void }) {
  const { ep, wt, revision, ready, waiting } = useCheckout(lane);
  const l = useIde((s) => s.lanes[lane] ?? EMPTY_IDE);
  const projectName = useStore((s) => s.projects[s.lanes[lane]?.project]?.name);
  const changes = useStore((s) => s.lanes[lane]?.git?.changes);
  const [nodes, setNodes] = useState<FileNode[]>([]);
  const [error, setError] = useState<string>();
  const [loaded, setLoaded] = useState(false);
  const [retry, setRetry] = useState(0);
  const reader = useRef<{ refresh(): void } | null>(null);
  const list = useRef<VirtuosoHandle>(null);
  const box = useRef<HTMLDivElement>(null);
  useEffect(() => {
    if (!enabled || !ready || !ep) return;
    setLoaded(false);
    const reading = liveReader(() => get<FileNode[]>(ep, "/api/tree", { wt }), (tree) => {
      setNodes(tree); setLoaded(true); setError(undefined);
    }, (e) => { setError(String(e)); setLoaded(true); }, 300);
    reader.current = reading;
    reading.refresh();
    return () => { reading.close(); reader.current = null; };
  }, [ep, wt, ready, retry, enabled]);
  useEffect(() => { reader.current?.refresh(); }, [revision]);

  const changed = useMemo(() => new Map((changes ?? []).filter((c) => !c.dir).map((c) => [c.path, c.status.trim() || "M"])), [changes]);
  const rows = useMemo(() => visibleTree(nodes, l.expanded, [...changed.keys()]), [nodes, l.expanded, changed]);
  const treeSize = useMemo(() => {
    const count = (list: FileNode[]): number => list.reduce((n, node) => n + 1 + count(node.children ?? []), 0);
    return count(nodes);
  }, [nodes]);
  function focus(index: number) {
    const row = rows[index];
    if (!row) return;
    list.current?.scrollIntoView({ index, done: () => {
      box.current?.querySelector<HTMLButtonElement>(`[data-path="${CSS.escape(row.node.path)}"]`)?.focus();
    } });
  }
  function toggle(path: string) { useIde.getState().patch(lane, { expanded: l.expanded.includes(path) ? l.expanded.filter((p) => p !== path) : [...l.expanded, path] }); }
  return <section className="ide-explorer" aria-label="File explorer">
    <header><span title={wt ? `keel/${wt}` : projectName}>{projectName}{wt && <small> / {wt}</small>}</span>
      <button className="ghost" aria-label="Refresh files" onClick={() => setRetry((n) => n + 1)}><Icon name="rewind" size={13} /></button>
    </header>
    {!ready ? <p className="ide-note">{waiting}</p> : error ? <p className="ide-note error" role="alert">{error}<button onClick={() => setRetry((n) => n + 1)}>Retry</button></p> : !loaded ? <p className="ide-note muted">Reading files…</p> : rows.length === 0 ? <p className="ide-note muted">No project files yet.</p> : <div ref={box} className="ide-tree" role="tree" aria-label="Project files" onKeyDown={(e) => {
      const button = (e.target as HTMLElement).closest<HTMLElement>("[data-path]");
      const index = rows.findIndex((r) => r.node.path === button?.dataset.path);
      const row = rows[index];
      if (!row || e.metaKey || e.ctrlKey || e.altKey) return;
      if (e.key === "ArrowDown") focus(Math.min(rows.length - 1, index + 1));
      else if (e.key === "ArrowUp") focus(Math.max(0, index - 1));
      else if (e.key === "Home") focus(0);
      else if (e.key === "End") focus(rows.length - 1);
      else if (e.key === "ArrowRight" && row.node.dir) { if (!l.expanded.includes(row.node.path)) toggle(row.node.path); else focus(index + 1); }
      else if (e.key === "ArrowLeft") { if (row.node.dir && l.expanded.includes(row.node.path)) toggle(row.node.path); else focus(rows.findIndex((r) => r.node.path === row.parent)); }
      else return;
      e.preventDefault();
      e.stopPropagation();
    }}><Virtuoso ref={list} totalCount={rows.length} computeItemKey={(i) => rows[i].node.path} itemContent={(i) => {
      const { node: n, depth } = rows[i];
      const status = changed.get(n.path);
      const descendant = n.dir && [...changed.keys()].some((p) => p.startsWith(n.path + "/"));
      return <button role="treeitem" aria-level={depth + 1} aria-expanded={n.dir ? l.expanded.includes(n.path) : undefined} aria-selected={l.selected === n.path}
        data-path={n.path} title={n.path + (status ? ` · ${status}` : "")} className={`ide-file${l.selected === n.path ? " selected" : ""}`} style={{ paddingLeft: 12 + depth * 14 }}
        onClick={() => {
          if (n.dir) toggle(n.path);
          else { useIde.getState().open(lane, n.path, status ? "diff" : "code"); showCode?.(); }
        }}>
        <Icon name={n.dir ? l.expanded.includes(n.path) ? "chevron-down" : "chevron-right" : "file"} size={13} />
        <span>{n.name}</span>{(status || descendant) && <small className={status?.includes("D") ? "del" : "add"}>{status ?? "•"}</small>}
      </button>;
    }} /></div>}
    <footer>Read-only · Agent edits appear live{treeSize >= 20_000 && <span>File list limited to 20,000 entries.</span>}</footer>
  </section>;
}

interface Content { text: string | null; note?: string | null }
interface PreviewData { path: string; mode: string; checkout: string; staged?: boolean; content: Content; diff?: Prepared }

export function IdeWorkspace({ lane, enabled, children, showCode, terminalOpen, toggleTerminal, showGit }: { lane: string; enabled: boolean; children?: ReactNode; showCode?: () => void; terminalOpen: boolean; toggleTerminal: () => void; showGit: () => void }) {
  const { ep, wt, root, ready, waiting, revision } = useCheckout(lane);
  const l = useIde((s) => s.lanes[lane] ?? EMPTY_IDE);
  const editsKey = useStore((s) => JSON.stringify(completedEdits(s.lanes[lane]?.conv ?? { turns: [] })));
  const [data, setData] = useState<PreviewData>();
  const [error, setError] = useState<string>();
  const [retry, setRetry] = useState(0);
  const reader = useRef<{ refresh(): void } | null>(null);
  const seen = useRef(new Set<string>());
  const list = useRef<VirtuosoHandle>(null);
  const reveal = useRef(false);
  const latest = useRef<string | undefined>(undefined);
  const wasEnabled = useRef(false);

  useEffect(() => {
    const entering = enabled && !wasEnabled.current;
    wasEnabled.current = enabled;
    const edits = JSON.parse(editsKey) as { id: string; path: string }[];
    const fresh = edits.filter((e) => !seen.current.has(e.id));
    seen.current = new Set(edits.map((e) => e.id));
    const edit = fresh.at(-1);
    if (edit) latest.current = relativeFile(edit.path, root);
    if (enabled && l.follow && latest.current && (edit || !l.selected || entering)) {
      reveal.current = true;
      useIde.getState().open(lane, latest.current, "diff", false);
    }
  }, [editsKey, root, lane, enabled, l.follow, l.selected]);

  useEffect(() => {
    if (!enabled || !ready || !ep || !l.selected) return;
    const path = l.selected;
    setError(undefined);
    const reading = liveReader(async (): Promise<PreviewData> => {
      try {
        // Gate expensive diff work on the same bounded text preview as Code.
        const content = await get<Content>(ep, "/api/file", { path, wt });
        const diff = l.mode === "diff" && content.text !== null ? prepare(await get<DiffResponse>(ep, "/api/git/diff", { path, wt, staged: l.staged })) : undefined;
        return { path, mode: l.mode, checkout: wt ?? "", staged: l.staged, content, diff };
      } catch (e) {
        // Deleted paths may still have a useful diff even though their current content is gone.
        if (l.mode !== "diff") throw e;
        const diff = prepare(await get<DiffResponse>(ep, "/api/git/diff", { path, wt, staged: l.staged }));
        if (!diff.rows.length) throw e;
        return { path, mode: l.mode, checkout: wt ?? "", staged: l.staged, content: { text: null }, diff };
      }
    }, (value) => { setData(value); setError(undefined); }, (e) => setError(String(e)));
    reader.current = reading;
    reading.refresh();
    return () => { reading.close(); reader.current = null; };
  }, [ep, wt, ready, l.selected, l.mode, l.staged, enabled, retry]);
  useEffect(() => { reader.current?.refresh(); }, [revision, editsKey]);

  const current = data?.path === l.selected && data?.mode === l.mode && data?.staged === l.staged && data?.checkout === (wt ?? "") ? data : undefined;
  const diff = l.mode === "diff" ? current?.diff : undefined;
  useEffect(() => {
    if (!current || !reveal.current) return;
    reveal.current = false;
    const index = diff?.rows.findIndex((r) => r.kind === "add" || r.kind === "del") ?? 0;
    list.current?.scrollToIndex({ index: Math.max(0, index), align: "center" });
  }, [current, diff]);
  const pause = () => { if (l.follow || l.targetLine) useIde.getState().patch(lane, { follow: false, targetLine: undefined }); };
  const area = l.mode === "diff" ? l.staged : undefined;
  const changeMode = (mode: "code" | "diff") => { setData(undefined); setError(undefined); useIde.getState().patch(lane, { mode, follow: false }); showCode?.(); };
  return <section className="ide-center" aria-label="Code workspace" hidden={!enabled}>
    <div className="ide-workspace-actions">
      <button onClick={() => useIde.setState({ navigation: "files" })}><Icon name="search" size={13} />Go to File…</button>
      <span className="spacer" />
      <button onClick={showGit}><Icon name="branch" size={13} />Source Control</button>
      <button aria-pressed={terminalOpen} onClick={toggleTerminal}><Icon name="terminal" size={13} />Terminal</button>
    </div>
    <FileTabs lane={lane} l={l} showCode={showCode} />
    <header className="ide-toolbar" hidden={!!l.commit}><span className="ide-breadcrumb" title={l.selected}>{l.selected ? `${area === true ? "Staged Changes / " : area === false ? "Changes / " : ""}${l.selected}` : "Browse files or follow the agent"}</span>
      {l.selected && <div className="segmented" role="group" aria-label="File view"><button aria-pressed={l.mode === "code"} onClick={() => changeMode("code")}>Code</button><button aria-pressed={l.mode === "diff"} onClick={() => changeMode("diff")}>Diff</button></div>}
      <button className={`ide-follow${l.follow ? " active" : ""}`} aria-pressed={l.follow} onClick={() => {
        useIde.getState().patch(lane, { follow: !l.follow });
        if (!l.follow && latest.current) { reveal.current = true; useIde.getState().open(lane, latest.current, "diff", false); }
      }}><span className="ide-live-dot" />{l.follow ? "Following changes" : "Resume following"}</button>
    </header>
    <div className="ide-preview" onWheel={pause} onPointerDown={pause} onKeyDown={(e) => {
      if (["ArrowUp", "ArrowDown", "PageUp", "PageDown", "Home", "End"].includes(e.key)) pause();
    }}>
      {children ?? (l.commit && ep ? <CommitReview key={l.commit.sha} ep={ep} wt={wt} commit={l.commit} /> : <CodeContent current={current} ready={ready} waiting={waiting} error={error} lane={lane} l={l} list={list} setRetry={setRetry} changeMode={changeMode} />)}
    </div>
  </section>;
}


function FileTabs({ lane, l, showCode }: { lane: string; l: IdeLane; showCode?: () => void }) {
  return <div className="ide-file-tabs" aria-label="Open files">
    {l.tabs.map((path) => <div className={`ide-file-tab${!l.commit && path === l.selected ? " active" : ""}`} key={path}>
      <button title={path} aria-pressed={!l.commit && path === l.selected} onClick={() => { useIde.getState().open(lane, path, "code"); showCode?.(); }}><Icon name="file" size={13} />{path.split("/").at(-1)}</button>
      <button className="ghost" aria-label={`Close ${path}`} onClick={() => useIde.getState().close(lane, path)}><Icon name="x" size={12} /></button>
    </div>)}
    {l.commit && <div className="ide-file-tab active"><button title={l.commit.subject} aria-pressed="true"><Icon name="branch" size={13} />{l.commit.sha.slice(0, 7)} · {l.commit.subject}</button><button className="ghost" aria-label="Close commit details" onClick={() => useIde.getState().patch(lane, { commit: undefined })}><Icon name="x" size={12} /></button></div>}
    {!l.tabs.length && !l.commit && <span className="ide-tabs-placeholder">Code workspace</span>}
  </div>;
}

function CodeContent({ current, ready, waiting, error, lane, l, list, setRetry, changeMode }: {
  current?: PreviewData; ready: boolean; waiting: string; error?: string; lane: string; l: IdeLane;
  list: RefObject<VirtuosoHandle | null>; setRetry: (update: (n: number) => number) => void; changeMode: (mode: "code" | "diff") => void;
}) {
  const lines = useMemo(() => current?.content.text?.split("\n").slice(0, 20_000) ?? [], [current]);
  const diff = l.mode === "diff" ? current?.diff : undefined;
  const scrollKey = `${l.selected}:${l.mode}`;
  useEffect(() => {
    if (!current || l.mode !== "code" || !l.targetLine) return;
    const index = Math.max(0, Math.min(lines.length - 1, l.targetLine.line - 1));
    const frame = requestAnimationFrame(() => {
      list.current?.scrollToIndex({ index, align: "center" });
      document.querySelector<HTMLElement>(".ide-code-list")?.focus({ preventScroll: true });
    });
    return () => cancelAnimationFrame(frame);
  }, [current, l.mode, l.targetLine, lines.length, list]);
  if (!ready) return <div className="panel-empty muted">{waiting}</div>;
  if (!l.selected) return <div className="ide-welcome">
    <Icon name="file" size={28} /><h2>Your code, in view.</h2>
    <p>Choose a file on the left, or send a message on the right.<br />The files your agent changes will appear here.</p><small>Read-only workspace</small>
  </div>;
  if (error) return <div className="panel-empty error" role="alert">{error}<button onClick={() => setRetry((n) => n + 1)}>Retry</button></div>;
  if (!current) return <div className="panel-empty muted">Reading {l.selected}…</div>;
  return <>
    <PreviewNotes current={current} diff={diff} mode={l.mode} lineCount={lines.length} changeMode={changeMode} />
    <Virtuoso key={scrollKey} ref={list} tabIndex={0} className={l.mode === "diff" ? "diff ide-code-list" : "ide-code-list"}
      initialScrollTop={l.targetLine ? undefined : l.scroll[scrollKey] ?? 0} initialTopMostItemIndex={l.targetLine ? { index: Math.max(0, Math.min(lines.length - 1, l.targetLine.line - 1)), align: "center" } : undefined} totalCount={diff ? diff.rows.length : l.mode === "code" ? lines.length : 0}
      onScroll={(e) => {
        const s = useIde.getState();
        const state = s.lanes[lane] ?? EMPTY_IDE;
        s.patch(lane, { scroll: { ...state.scroll, [scrollKey]: e.currentTarget.scrollTop } });
      }} itemContent={(i) => <CodeRow diff={diff} lines={lines} path={l.selected ?? ""} index={i} target={l.targetLine?.line} />} />
  </>;
}

function CodeRow({ diff, lines, path, index: i, target }: { diff?: Prepared; lines: string[]; path: string; index: number; target?: number }) {
  if (diff) {
    const r = diff.rows[i];
    if (r.kind === "hunk") return <div className="row hunk">{r.text}</div>;
    const line = r as { kind: string; old: number | null; new: number | null; text: string };
    return <div className={`row ${line.kind}`}><span className="ln">{line.old ?? ""}</span><span className="ln">{line.new ?? ""}</span><span className="code-text"><SyntaxText text={line.text} path={path} /></span></div>;
  }
  return <div className={`ide-code-line${target === i + 1 ? " ide-target-line" : ""}`}><span className="ln">{i + 1}</span><code><SyntaxText text={lines[i]} path={path} /></code></div>;
}

function PreviewNotes({ current, diff, mode, lineCount, changeMode }: { current: PreviewData; diff?: Prepared; mode: string; lineCount: number; changeMode: (mode: "code" | "diff") => void }) {
  return <>
    {current.content.note && !diff?.rows.length && <div className="panel-note muted">{current.content.note}</div>}
    {diff && !diff.rows.length && <div className="panel-note muted">{diff.note ?? "No working-tree differences. The agent may have checkpointed this file."}<button className="link" onClick={() => changeMode("code")}>View code</button></div>}
    {diff?.note && !!diff.rows.length && <div className="panel-note muted">{diff.note}</div>}
    {mode === "code" && lineCount >= 20_000 && <div className="panel-note muted">Showing the first 20,000 lines.</div>}
    {mode === "code" && current.content.text === "" && <div className="panel-note muted">This file is empty.</div>}
  </>;
}
