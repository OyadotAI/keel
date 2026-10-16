import { useEffect, useMemo, useRef, useState } from "react";
import { post, type Endpoint } from "../api";
import { inArea, type Change } from "../git";
import { useStore } from "../store";
import { Diff } from "./Changes";
import { Icon } from "./icons";
import { MAC } from "../keys";

type Act = (message: string, run: () => Promise<unknown>) => Promise<void>;
interface Actions { lane: string; ep: Endpoint; wt?: string; act: Act; disabled: boolean; onOpenDiff?: (path: string, staged: boolean) => void; selection?: { path?: string; staged?: boolean } }
const NONE: Change[] = [];
const commitDrafts = new Map<string, string>();
const LIMIT = 2000;
const STATUS_LABELS: Record<string, string> = { U: "Untracked", M: "Modified", A: "Added", D: "Deleted", R: "Renamed", C: "Copied" };

export function SourceControl({ lane, ep, wt, act, onOpenDiff, selection }: Omit<Actions, "disabled">) {
  const git = useStore((s) => s.lanes[lane]?.git);
  const running = useStore((s) => !!s.lanes[lane]?.running);
  const viewing = useStore((s) => s.lanes[lane]?.viewing);
  const [message, setMessage] = useState(() => commitDrafts.get(lane) ?? "");
  const [filter, setFilter] = useState("");
  const [tree, setTree] = useState(false);
  const [pending, setPending] = useState(false);
  const locked = useRef(false);
  async function action(note: string, run: () => Promise<unknown>) {
    if (locked.current) return;
    locked.current = true;
    setPending(true);
    try { await act(note, run); }
    finally { locked.current = false; setPending(false); }
  }
  const changes = git?.changes ?? NONE;
  const staged = useMemo(() => changes.filter((c) => inArea(c, true)), [changes]);
  const unstaged = useMemo(() => changes.filter((c) => inArea(c, false)), [changes]);
  const actions = { lane, ep, wt, act: action, disabled: pending, onOpenDiff, selection };
  const canCommit = !!message.trim() && !!staged.length && !pending && !running;
  const commit = () => {
    if (!canCommit) return;
    void action("Committed staged changes.", async () => {
      await post(ep, "/api/git/commit-staged", { message }, { wt });
      setMessage("");
      commitDrafts.delete(lane);
    });
  };
  if (!git) return <div className="panel-empty">Reading git…</div>;
  return <div className="scm" aria-label="Source control">
    <div className={`scm-layout${viewing && !onOpenDiff ? " has-diff" : ""}`}>
      <div className="scm-sidebar">
        <div className="scm-commit">
          <textarea value={message} onChange={(e) => { setMessage(e.target.value); commitDrafts.set(lane, e.target.value); }} rows={onOpenDiff ? 1 : 2} aria-label="Commit message" placeholder={`Message (${MAC ? "⌘Enter" : "Ctrl+Enter"} to commit)`} onKeyDown={(e) => {
            if (e.key === "Enter" && (e.metaKey || e.ctrlKey)) { e.preventDefault(); commit(); }
          }} />
          <button className="primary scm-commit-button" disabled={!canCommit} onClick={commit} title={running ? "Wait for the agent to finish its turn" : !staged.length ? "Stage changes to commit them" : "Commit staged changes (⌘Enter / Ctrl+Enter)"}>
            <Icon name="check-circle" size={14} />{pending ? "Working…" : "Commit"}{staged.length > 0 && <span>{staged.length}</span>}
          </button>
        </div>
        <div className="scm-toolbar">
          <div className="scm-filter"><Icon name="search" size={13} /><input aria-label="Filter changed files" value={filter} onChange={(e) => setFilter(e.target.value)} placeholder="Filter files…" />{filter && <button className="ghost" aria-label="Clear file filter" onClick={() => setFilter("")}><Icon name="x" size={12} /></button>}</div>
          <button className="ghost" aria-label={tree ? "Show changes as a list" : "Show changes as a tree"} title={tree ? "List view" : "Tree view"} onClick={() => setTree(!tree)}><Icon name={tree ? "list" : "folder"} size={14} /></button>
          <button className="ghost" aria-label="Refresh source control" title="Refresh" disabled={pending} onClick={() => void action("Changes refreshed.", () => useStore.getState().refreshGit(lane))}><Icon name="rewind" size={14} /></button>
        </div>
        <div className="scm-files" onKeyDown={walk}>
          {changes.length === 0 ? <div className="scm-clean"><Icon name="check-circle" size={22} /><strong>No changes</strong><span>Your working tree is clean.</span></div> : <>
            {(!onOpenDiff || staged.length > 0) && <ChangeGroup key={`${lane}:staged`} title="Staged Changes" staged changes={staged} filter={filter} tree={tree} actions={actions} />}
            <ChangeGroup key={`${lane}:changes`} title="Changes" staged={false} changes={unstaged} filter={filter} tree={tree} actions={actions} />
          </>}
          {git.collapsed && <p className="scm-hint">Git has collapsed large folders. Use the terminal to inspect every file.</p>}
        </div>
      </div>
      {viewing && !onOpenDiff && <div className="scm-editor"><Diff lane={lane} embedded /></div>}
    </div>
  </div>;
}

function ChangeGroup({ title, staged, changes, filter, tree, actions }: { title: string; staged: boolean; changes: Change[]; filter: string; tree: boolean; actions: Actions }) {
  const [open, setOpen] = useState(true);
  const files = useMemo(() => changes.filter((c) => c.path.toLocaleLowerCase().includes(filter.toLocaleLowerCase())).slice(0, LIMIT), [changes, filter]);
  return <section className="scm-group" aria-label={title}>
    <div className="scm-group-heading">
      <button className="scm-group-toggle" aria-expanded={open} onClick={() => setOpen(!open)}><Icon name={open ? "chevron-down" : "chevron-right"} size={12} /><strong>{title}</strong><span className="scm-count">{changes.length}</span></button>
      <button className="ghost scm-group-action" aria-label={staged ? "Unstage all changes" : "Stage all changes"} title={staged ? "Unstage all changes" : "Stage all changes"} disabled={actions.disabled || !changes.length} onClick={() => void actions.act(staged ? "Unstaged all changes." : "Staged all changes.", () => post(actions.ep, "/api/git/stage-all", { stage: !staged }, { wt: actions.wt }))}><Icon name={staged ? "minus" : "plus"} size={15} /></button>
    </div>
    {open && <>
      {files.length === 0 ? <p className="scm-hint">{changes.length ? "No matching files." : staged ? "Stage changes with + to include them in a commit." : "No unstaged changes."}</p> : <FileTree files={files} filter={filter} tree={tree} staged={staged} actions={actions} />}
      {changes.length > LIMIT && <p className="scm-hint">Showing up to {LIMIT} matching files. Filter to narrow the list.</p>}
    </>}
  </section>;
}

interface Directory { path: string; name: string; dirs: Map<string, Directory>; files: Change[] }
function directory(path = "", name = ""): Directory { return { path, name, dirs: new Map(), files: [] }; }
function compact(dir: Directory): Directory {
  const names = [dir.name];
  while (!dir.files.length && dir.dirs.size === 1) {
    dir = dir.dirs.values().next().value!;
    names.push(dir.name);
  }
  return { ...dir, name: names.join("/") };
}
function fileTree(files: Change[]) {
  const root = directory();
  for (const file of files) {
    const parts = file.path.replace(/\/$/, "").split("/");
    let parent = root;
    for (const name of parts.slice(0, -1)) {
      if (!parent.dirs.has(name)) parent.dirs.set(name, directory(`${parent.path}${name}/`, name));
      parent = parent.dirs.get(name)!;
    }
    parent.files.push(file);
  }
  return root;
}

function FileTree({ files, filter, tree, staged, actions }: { files: Change[]; filter: string; tree: boolean; staged: boolean; actions: Actions }) {
  const root = useMemo(() => fileTree(files), [files]);
  const [closed, setClosed] = useState<Set<string>>(new Set());
  function toggle(path: string, collapse?: boolean) {
    setClosed((prev) => { const next = new Set(prev); if (collapse ?? !next.has(path)) next.add(path); else next.delete(path); return next; });
  }
  function render(dir: Directory, depth: number): React.ReactNode {
    return <>
      {[...dir.dirs.values()].map(compact).sort((a, b) => a.name.localeCompare(b.name)).map((child) => {
        const expanded = !!filter || !closed.has(child.path);
        return <div key={child.path}>
          <button data-scm-node className="scm-folder" style={{ paddingLeft: 10 + depth * 14 }} title={child.path} aria-expanded={expanded} onClick={() => toggle(child.path)} onKeyDown={(e) => {
            if (e.key === "ArrowLeft" || e.key === "ArrowRight") { e.preventDefault(); e.stopPropagation(); toggle(child.path, e.key === "ArrowLeft"); }
          }}><Icon name={expanded ? "chevron-down" : "chevron-right"} size={12} /><Icon name="folder" size={14} /><span>{child.name}</span></button>
          {expanded && render(child, depth + 1)}
        </div>;
      })}
      {dir.files.sort((a, b) => a.path.localeCompare(b.path)).map((c) => <FileRow key={c.path} change={c} staged={staged} depth={depth} list={false} actions={actions} />)}
    </>;
  }
  return <div className="scm-tree">{tree ? render(root, 0) : [...files].sort((a, b) => a.path.localeCompare(b.path)).map((c) => <FileRow key={c.path} change={c} staged={staged} depth={0} list actions={actions} />)}</div>;
}

function FileRow({ change: c, staged, depth, list, actions: { lane, ep, wt, act, disabled, onOpenDiff, selection } }: { change: Change; staged: boolean; depth: number; list: boolean; actions: Actions }) {
  const viewing = useStore((s) => s.lanes[lane]?.viewing === c.path && s.lanes[lane]?.viewingStaged === staged);
  const selected = onOpenDiff ? selection?.path === c.path && selection.staged === staged : viewing;
  const row = useRef<HTMLDivElement>(null);
  useEffect(() => { if (selected) row.current?.scrollIntoView({ block: "nearest", inline: "nearest" }); }, [selected]);
  const [discard, setDiscard] = useState(false);
  const path = c.path.replace(/\/$/, "");
  const name = path.split("/").at(-1);
  const dir = path.slice(0, Math.max(0, path.lastIndexOf("/")));
  const status = fileStatus(c, staged);
  const label = STATUS_LABELS[status] ?? c.label;
  const stage = () => act(`${staged ? "Unstaged" : "Staged"} ${c.path}.`, () => post(ep, "/api/git/act", { action: staged ? "unstage" : "stage", path: c.path }, { wt }));
  return <div ref={row} className={`scm-file${status === "D" ? " deleted" : ""}${selected ? " selected" : ""}`} style={{ paddingLeft: 10 + depth * 14 }}>
    <button data-scm-node data-file={c.path} data-area={staged ? "staged" : "changes"} className="scm-file-label" disabled={c.dir} aria-current={selected ? "true" : undefined} title={`${c.path} · ${label}${staged ? " · Staged" : ""}\nEnter: open diff · Space: ${staged ? "unstage" : "stage"}`} onClick={() => onOpenDiff ? onOpenDiff(c.path, staged) : void useStore.getState().openDiff(lane, c.path, staged)} onKeyDown={(e) => {
      if (e.key === " ") { e.preventDefault(); if (!disabled) void stage(); }
    }}><Icon name={c.dir ? "folder" : "file"} size={14} /><span className="scm-filename">{name}</span>{list && dir && <span className="scm-dirname">{dir}</span>}</button>
    {discard ? <div className="scm-confirm"><button disabled={disabled} onClick={() => { setDiscard(false); void act(`Discarded unstaged changes to ${c.path}.`, () => post(ep, "/api/git/act", { action: "discard-worktree", path: c.path }, { wt })); }}>{c.status === "??" ? "Trash file?" : "Discard edits?"}</button><button className="ghost" aria-label="Cancel discard" onClick={() => setDiscard(false)}><Icon name="x" size={12} /></button></div> : <div className="scm-file-actions">
      {!staged && <button className="ghost" disabled={disabled} aria-label={`Discard changes to ${c.path}`} title="Discard this file’s changes" onClick={() => setDiscard(true)}><Icon name="rewind" size={13} /></button>}
      <button className="ghost" disabled={disabled} aria-label={`${staged ? "Unstage" : "Stage"} ${c.path}`} title={staged ? "Unstage changes" : "Stage changes"} onClick={() => void stage()}><Icon name={staged ? "minus" : "plus"} size={15} /></button>
    </div>}
    <span className={`scm-status ${status === "U" || status === "A" ? "add" : status === "D" ? "del" : ""}`} title={label}>{status}</span>
  </div>;
}

function walk(e: React.KeyboardEvent<HTMLDivElement>) {
  if (e.metaKey || e.ctrlKey || e.altKey || !["ArrowDown", "ArrowUp", "Home", "End"].includes(e.key)) return;
  const nodes = [...e.currentTarget.querySelectorAll<HTMLButtonElement>("[data-scm-node]:not(:disabled)")];
  const at = nodes.indexOf(document.activeElement as HTMLButtonElement);
  if (at < 0) return;
  e.preventDefault(); e.stopPropagation();
  const next = e.key === "Home" ? 0 : e.key === "End" ? nodes.length - 1 : at + (e.key === "ArrowDown" ? 1 : -1);
  nodes[Math.max(0, Math.min(nodes.length - 1, next))]?.focus();
}

function fileStatus(change: Change, staged: boolean) {
  if (change.status === "??") return "U";
  return change.status[staged ? 0 : 1]?.trim() || change.status.trim()[0] || "M";
}
