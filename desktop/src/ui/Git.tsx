// Source Control composes checkout mutations, branch discovery, and commit history.
import { Floating } from "./Menu";
import { useCallback, useEffect, useRef, useState, useId, type ReactNode } from "react";
import { get, post, type Endpoint } from "../api";
import { isWorkspace, prepare, type DiffResponse, type Prepared } from "../git";
import { useStore } from "../store";
import { SourceControl } from "./SourceControl";
import { InlineDiff } from "./InlineDiff";
import { type GraphCommit } from "../commit-graph";
import { CommitGraph } from "./CommitGraph";
import { useIde } from "../ide";
import { useGitCards } from "../git-cards";
import { Count, Tabs } from "./kit";
import { PullRequest } from "./PullRequest";
import { Icon } from "./icons";
import { BranchList } from "./Branches";
import { GitCardSplitter } from "./GitCardSplitter";
import type { Branch, Branches, GitAction } from "../branches";

interface Commit {
  sha: string;
  subject: string;
  when: number;
  files: number;
  pushed: boolean;
}
/// The lane's daemon and checkout — two values that change about never. Selecting the lane itself
/// re-rendered the whole panel on every batch of a streaming turn.
function useLane(lane: string) {
  const wt = useStore((s) => s.lanes[lane]?.wt);
  const ep = useStore((s) => {
    const project = s.lanes[lane]?.project;
    return project ? s.projects[project]?.endpoint : undefined;
  });
  return { ep, wt };
}

/// The lane's checkout as git sees it: what changed, what to commit, what was committed, which
/// branch, and the way out to a pull request. Every action is the daemon's; this only asks.
export function Git({ lane, onOpenDiff, onOpenCommit, selection }: { onOpenCommit?: () => void; lane: string; onOpenDiff?: (path: string, staged: boolean) => void; selection?: { path?: string; staged?: boolean } }) {
  const { ep, wt } = useLane(lane);
  const git = useStore((s) => s.lanes[lane]?.git);
  const running = useStore((s) => !!s.lanes[lane]?.running);
  const [pending, setPending] = useState(false);
  const locked = useRef(false);
  const [branchResult, setBranchResult] = useState<{ key: string; data: Branches }>();
  const [log, setLog] = useState<Commit[]>();
  const [error, setError] = useState<string>();
  const [readError, setReadError] = useState<string>();
  const [branchError, setBranchError] = useState<string>();
  const generation = useRef(0);
  const identity = useRef("");
  identity.current = `${ep?.port}:${ep?.token}:${wt ?? ""}`;
  const branches = branchResult?.key === identity.current ? branchResult.data : undefined;
  const [note, setNote] = useState<string>();
  const [section, setSection] = useState<GitSection>("changes");

  const read = useCallback(async (e: Endpoint) => {
    // Settled, so a failed branch read keeps the history it has, and a good read clears the banner.
    const ticket = ++generation.current;
    const key = `${e.port}:${e.token}:${wt ?? ""}`;
    const [b, c] = await Promise.allSettled([get<Branches>(e, "/api/git/branches", { wt }), get<Commit[]>(e, "/api/git/log", { wt, n: 20 })]);
    if (ticket !== generation.current || identity.current !== key) return;
    setBranchError(b.status === "rejected" ? String(b.reason) : undefined);
    if (b.status === "fulfilled") setBranchResult({ key, data: b.value });
    if (c.status === "fulfilled") setLog(c.value);
    const failed = [b, c].find((r) => r.status === "rejected");
    setReadError(failed ? String((failed as PromiseRejectedResult).reason) : undefined);
  }, [wt]);
  // Read again whenever the daemon says git moved (the store re-reads `git` on those events).
  useEffect(() => {
    if (ep) void read(ep);
    // `git` is the trigger, not an input: a new status means branches and history moved too.
  }, [ep, read, git]);

  async function act(what: string, run: () => Promise<unknown>) {
    if (locked.current) return false;
    locked.current = true;
    setPending(true);
    setError(undefined);
    setNote(undefined);
    let ok = false;
    try {
      const r = await run();
      ok = true;
      if (typeof r === "string" && r) setNote(r);
      else setNote(what);
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
    }
    try { await useStore.getState().refreshGit(lane); if (ep) await read(ep); }
    finally { setPending(false); locked.current = false; }
    return ok;
  }

  if (!ep) return <div className="panel-empty">Starting Oya Factory for this project…</div>;
  if (git && !git.is_repo)
    return (
      <div className="panel-empty">
        <h3>Not a git repository</h3>
        <p>Oya Factory commits each turn and gives lanes their own branches, and both need git.</p>
        <button className="primary" onClick={() => act("Initialised a git repository.", () => post(ep, "/api/git/init", {}, { wt }))}>
          Initialise git here
        </button>
      </div>
    );
  const current = branches?.local.find((b) => b.current);
  const content = <GitContent lane={lane} ep={ep} wt={wt} act={act} section={section} log={log} branches={branches} pending={pending} running={running} onOpenDiff={onOpenDiff} selection={selection} branchError={branchError} retryBranches={() => void read(ep)} />;
  // A folder of repositories has no branch of its own: git's "not a repository" about the folder is
  // not news, and showing it twice read as the panel being broken.
  const workspace = isWorkspace(git);
  const feedback = <GitFeedback error={error || (workspace ? undefined : readError)} note={note} retry={() => void read(ep)} />;
  if (onOpenDiff) return <CodingGit lane={lane} ep={ep} wt={wt} branches={branches} pending={pending} running={running} act={act} section={section} setSection={setSection} feedback={feedback} onOpenCommit={onOpenCommit}>{content}</CodingGit>;
  return (
    <div className="git-panel">
      <div className="panel-head">
        <Icon name="branch" size={14} />
        <span className="mono" style={{ fontWeight: 500 }}>
          {branches?.current ?? git?.branch ?? "…"}
        </span>
        {current && (current.ahead > 0 || current.behind > 0) && (
          <span className="mono small faint">
            {current.ahead ? `↑${current.ahead}` : ""} {current.behind ? `↓${current.behind}` : ""}
          </span>
        )}
        <span style={{ flex: 1 }} />
        <GitHeaderActions ep={ep} wt={wt} current={current} running={running} pending={pending} act={act} remotes={branches?.remotes} />
      </div>
      <Tabs
        className="tabs-sub"
        tabs={[
          { id: "changes", label: <>Changes <Count n={git?.changes.length} /></> },
          { id: "history", label: "History" },
          { id: "branches", label: "Branches" },
          { id: "pr", label: "Pull request" },
        ]}
        value={section}
        onChange={setSection}
      />
      {feedback}
      <div className={section === "changes" ? "git-body" : "git-scroll"}>
        {content}
      </div>
    </div>
  );
}

type GitSection = "changes" | "history" | "branches" | "pr" | "remotes";
interface GitContentProps {
  branchError?: string; retryBranches?: () => void;
  lane: string; ep: Endpoint; wt?: string; act: Act; section: GitSection; log?: Commit[]; branches?: Branches; pending: boolean; running: boolean;
  onOpenDiff?: (path: string, staged: boolean) => void; selection?: { path?: string; staged?: boolean };
}
function GitContent({ lane, ep, wt, act, section, log, branches, pending, running, onOpenDiff, selection, branchError, retryBranches }: GitContentProps) {
  switch (section) {
    case "changes": return <SourceControl lane={lane} ep={ep} wt={wt} act={act} onOpenDiff={onOpenDiff} selection={selection} />;
    case "history": return <History lane={lane} ep={ep} wt={wt} log={log} act={act} />;
    case "branches": return <BranchList lane={lane} ep={ep} wt={wt} branches={branches} act={act} pending={pending} running={running} error={branchError} retry={retryBranches} />;
    case "pr": return <PullRequest ep={ep} wt={wt} lane={lane} />;
    case "remotes": return <div className="panel-head git-remotes"><span className="small muted">{branches?.remotes.length ? branches.remotes.join(", ") : "No remote yet"}</span><span style={{ flex: 1 }} /><GitHeaderActions ep={ep} wt={wt} current={branches?.local.find((b) => b.current)} running={running} pending={pending} act={act} remotes={branches?.remotes} /></div>;
  }
}
function GitFeedback({ error, note, retry }: { error?: string; note?: string; retry: () => void }) {
  if (error) return <div className="panel-note small error" role="alert">{error}<button className="link" onClick={retry}>Retry</button></div>;
  return note ? <div className="coding-git-notice" role="status">{note}</div> : null;
}
function CodingGit({ lane, ep, wt, branches, pending, running, act, section, setSection, feedback, children, onOpenCommit }: Omit<GitContentProps, "log" | "onOpenDiff" | "selection"> & { setSection: (section: GitSection) => void; feedback: ReactNode; children: ReactNode; onOpenCommit?: () => void }) {
  const git = useStore((s) => s.lanes[lane]?.git);
  const current = branches?.local.find((b) => b.current);
  const selected = useIde((s) => s.lanes[lane]?.commit?.sha);
  const [menu, setMenu] = useState(false);
  const changesOpen = useGitCards((s) => s.changes);
  const setChangesOpen = (open: boolean) => useGitCards.getState().setOpen("changes", open);
  const changesId = useId();
  const sectionTitle = section === "changes" ? "Changes" : section === "pr" ? "Pull request" : section[0].toUpperCase() + section.slice(1);
  return <div className="git-panel coding-git">
    <header className="coding-git-heading" onKeyDown={(e) => { if (e.key === "Escape") setMenu(false); }}><strong>Source Control</strong>
      <button className="ghost" aria-label="More source control actions" aria-expanded={menu} onClick={() => setMenu(!menu)}>···</button>
      {menu && <><button className="coding-menu-backdrop" aria-label="Close source control menu" onClick={() => setMenu(false)} /><div className="coding-git-menu" onKeyDown={(e) => { if (e.key === "Escape") setMenu(false); }}>
        {([ ["changes", "Changes"], ["branches", "Branches…"], ["history", "History & undo commit…"], ["remotes", "Remotes & sync…"], ["pr", "Pull request…"] ] as const).map(([id, label]) => <button key={id} onClick={() => { setSection(id); setChangesOpen(true); setMenu(false); }}>{label}</button>)}
      </div></>}
    </header>
    {feedback}
    <GitCardSplitter title={sectionTitle} upper={<section className={`coding-changes${changesOpen ? "" : " collapsed"}`} aria-label="Working changes">
      <button className="coding-section-toggle" title={`${changesOpen ? "Collapse" : "Expand"} ${sectionTitle}`} aria-controls={changesId} aria-expanded={changesOpen} onClick={() => setChangesOpen(!changesOpen)}><Icon name={changesOpen ? "chevron-down" : "chevron-right"} size={12} />{sectionTitle}<Count n={git?.changes.length} /></button>
      <div id={changesId} hidden={!changesOpen} className={section === "changes" ? "git-body" : "git-scroll"}>
        {section !== "changes" && <button className="link coding-back" onClick={() => setSection("changes")}>← Back to changes</button>}
        {children}
      </div>
    </section>} lower={isWorkspace(git) ? <Repositories repos={git?.repos ?? []} /> : <CommitGraph ep={ep} wt={wt} revision={git} selected={selected} onOpen={(commit) => { useIde.getState().patch(lane, { commit, follow: false }); onOpenCommit?.(); }} tools={<GraphRemoteActions ep={ep} wt={wt} current={current} pending={pending} running={running} remotes={branches?.remotes} act={act} />} />} />
    <BranchStatus lane={lane} branches={branches} open={() => { setSection("branches"); setChangesOpen(true); }} />
  </div>;
}

/// The branch the checkout is on and how far it is from its upstream — or, for a folder of
/// repositories, how many there are, since none of their branches is the folder's.
function BranchStatus({ lane, branches, open }: { lane: string; branches?: Branches; open: () => void }) {
  const git = useStore((s) => s.lanes[lane]?.git);
  if (isWorkspace(git)) return <div className="coding-branch-status"><Icon name="folder" size={13} /><span>{git?.repos?.length} repositories</span></div>;
  const current = branches?.local.find((b) => b.current);
  return <button className="coding-branch-status" title="Manage branches" onClick={open}><Icon name="branch" size={13} /><span>{branches?.current ?? git?.branch ?? "No commits yet"}</span><small>{current?.behind ?? 0}↓ {current?.ahead ?? 0}↑</small></button>;
}

/// In a folder of repositories the graph's place lists them: history is each one's own.
function Repositories({ repos }: { repos: { dir: string; branch: string | null }[] }) {
  return <section className="commit-graph coding-repos" aria-label="Repositories in this folder">
    <p className="coding-repos-note">History and branches are per repository.</p>
    <ul>{repos.map((r) => <li key={r.dir}><Icon name="folder" size={13} /><span className="coding-repo-name">{r.dir || "."}</span><span className="mono coding-repo-branch">{r.branch ?? "detached"}</span></li>)}</ul>
  </section>;
}

function GraphRemoteActions({ ep, wt, current, pending, running, remotes, act }: { ep: Endpoint; wt?: string; current?: Branch; pending: boolean; running: boolean; remotes?: string[]; act: Act }) {
  return <>
    <button className="ghost" aria-label="Fetch all remotes" title="Fetch all remotes" disabled={pending || !remotes?.length} onClick={() => void act("Fetched all remotes.", () => post(ep, "/api/git/remote", { action: "fetch" }, { wt }))}>⇣</button>
    <button className="ghost" aria-label="Pull upstream changes" title={current?.upstream ? `Pull ${current.upstream} (fast-forward only)` : "Set an upstream before pulling"} disabled={pending || running || !current?.upstream} onClick={() => void act("Pulled upstream changes.", () => post(ep, "/api/git/remote", { action: "pull" }, { wt }))}>↓</button>
    <button className="ghost" aria-label="Push branch" title={current?.upstream ? `Push to ${current.upstream}` : "Push and set upstream"} disabled={pending || running || !remotes?.length} onClick={() => void act("Pushed.", () => post(ep, "/api/git/push", {}, { wt }))}>↑</button>
  </>;
}
export function CommitReview({ ep, wt, commit }: { ep: Endpoint; wt?: string; commit: GraphCommit }) {
  return <article className="commit-review" aria-label="Commit details"><header><h2>{commit.subject}</h2><p>{commit.author} · {new Date(commit.when * 1000).toLocaleString()}</p><code>{commit.sha}</code>{commit.parents.length > 1 && <p>Merge commit · comparing with its first parent</p>}</header><CommitDiff key={commit.sha} ep={ep} wt={wt} sha={commit.sha} /></article>;
}

type Act = GitAction;

/// Sync as three small icons in the branch row, and the rare actions — merging or discarding the
/// lane, adding a remote — one click away under ⋯, so the panel opens on the changes themselves.
function GitHeaderActions(props: { ep: Endpoint; wt?: string; current?: Branch; running: boolean; pending: boolean; act: Act; remotes?: string[] }) {
  return <><SyncButtons {...props} /><GitMoreMenu {...props} /></>;
}

function SyncButtons({ ep, wt, current, running, pending, act, remotes }: { ep: Endpoint; wt?: string; current?: Branch; running: boolean; pending: boolean; act: Act; remotes?: string[] }) {
  const busy = pending || running, upstream = current?.upstream, hasRemote = !!remotes?.length;
  const pullTitle = running ? "Wait for the agent to finish" : upstream ? `Pull ${upstream} (fast-forward only)` : "Set an upstream before pulling";
  const pushTitle = !hasRemote ? "Add a remote to push" : upstream ? `Push to ${upstream}` : "Push and set upstream";
  return <>
    <button className="ghost" aria-label="Fetch all remotes" title="Fetch all remotes" disabled={pending || !hasRemote} onClick={() => void act("Fetched all remotes.", () => post(ep, "/api/git/remote", { action: "fetch" }, { wt }))}><Icon name="rotate-cw" size={13} /></button>
    <button className="ghost git-sync" aria-label="Pull upstream changes" title={pullTitle} disabled={busy || !upstream} onClick={() => void act("Pulled upstream changes.", () => post(ep, "/api/git/remote", { action: "pull" }, { wt }))}>↓{current?.behind || ""}</button>
    <button className="ghost git-sync" aria-label="Push branch" title={pushTitle} disabled={busy || !hasRemote} onClick={() => void act("Pushed.", () => post<string>(ep, "/api/git/push", {}, { wt }))}>↑{current?.ahead || ""}</button>
  </>;
}

/// Merging or discarding the lane and adding a remote: rare, so one click away under ⋯.
function GitMoreMenu({ ep, wt, running, pending, act, remotes }: { ep: Endpoint; wt?: string; running: boolean; pending: boolean; act: Act; remotes?: string[] }) {
  const [menu, setMenu] = useState<DOMRect | null>(null);
  const [adding, setAdding] = useState(false);
  const [remote, setRemote] = useState("");
  const busy = pending || running;
  const pick = (run: () => void) => () => { setMenu(null); run(); };
  const add = () => { setAdding(false); void act("Added origin.", () => post(ep, "/api/git/remote", { action: "add", url: remote.trim() }, { wt })); };
  return <>
    <button className="ghost" aria-label="More git actions" aria-haspopup="menu" onClick={(e) => setMenu(e.currentTarget.getBoundingClientRect())}><Icon name="more" size={15} /></button>
    {menu && <Floating anchor={menu} onClose={() => setMenu(null)}>
      {wt && <><button disabled={busy} onClick={pick(() => useStore.setState({ asking: "merge" }))}>Merge lane…</button><button className="danger-item" disabled={busy} onClick={pick(() => useStore.setState({ asking: "discard" }))}>Discard lane…</button><div className="sep" /></>}
      <button onClick={pick(() => setAdding(true))}>{remotes?.length ? "Add another remote…" : "Add origin…"}</button>
    </Floating>}
    {adding && <form className="git-add-remote" onSubmit={(e) => { e.preventDefault(); if (remote.trim()) add(); }}><input autoFocus className="field" aria-label="Origin remote URL" placeholder="https://github.com/owner/repo.git" value={remote} onChange={(e) => setRemote(e.target.value)} onKeyDown={(e) => e.key === "Escape" && setAdding(false)} /><button type="submit" disabled={pending || !remote.trim()}>Add</button></form>}
  </>;
}

function History({ lane, ep, wt, log, act }: { lane: string; ep: Endpoint; wt?: string; log?: Commit[]; act: Act }) {
  const [open, setOpen] = useState<string>();
  const busy = useStore((s) => !!s.lanes[lane]?.running);
  if (!log) return <div className="panel-empty">Reading the history…</div>;
  if (!log.length) return <div className="panel-empty">No commits yet.</div>;
  return (
    <div>
      {log.map((c, i) => (
        <div key={c.sha} style={{ borderBottom: "1px solid var(--line)" }}>
          <button className="file-row" style={{ height: 32, padding: "0 12px" }} aria-expanded={open === c.sha} onClick={() => setOpen(open === c.sha ? undefined : c.sha)}>
            <Icon name={open === c.sha ? "chevron-down" : "chevron-right"} size={12} />
            <span className="mono faint" style={{ fontSize: 11 }}>
              {c.sha.slice(0, 7)}
            </span>
            <span style={{ flex: 1, minWidth: 0, textAlign: "left", overflow: "hidden", textOverflow: "ellipsis", whiteSpace: "nowrap" }}>{c.subject}</span>
            <span className="mono faint" style={{ fontSize: 10 }}>
              {c.files}f · {new Date(c.when * 1000).toLocaleDateString([], { month: "short", day: "numeric" })}
              {!c.pushed ? " · local" : ""}
            </span>
          </button>
          {i === 0 && !c.pushed && (
            <div style={{ padding: "0 12px 6px 32px" }}>
              <button className="link small" disabled={busy} onClick={() => act("Undid the last commit; its changes are back in the tree.", () => post(ep, "/api/git/uncommit", {}, { wt }))}>
                Undo this commit
              </button>
            </div>
          )}
          {open === c.sha && <CommitDiff ep={ep} wt={wt} sha={c.sha} />}
        </div>
      ))}
    </div>
  );
}

function CommitDiff({ ep, wt, sha }: { ep: Endpoint; wt?: string; sha: string }) {
  const [files, setFiles] = useState<Prepared[]>();
  const [error, setError] = useState<string>();
  const [retry, setRetry] = useState(0);
  useEffect(() => {
    let live = true;
    setFiles(undefined); setError(undefined);
    get<DiffResponse[]>(ep, "/api/git/commit/diff", { sha, wt }).then((d) => {
      if (live) setFiles(d.slice(0, 30).map(prepare));
    }, (e) => { if (live) setError(String(e)); });
    return () => { live = false; };
  }, [ep, wt, sha, retry]);
  if (error) return <div className="panel-note error" role="alert">{error}<button onClick={() => setRetry((n) => n + 1)}>Retry</button></div>;
  if (!files) return <div className="panel-note faint">Reading changed files…</div>;
  if (!files.length) return <div className="panel-note faint">This commit has no file changes.</div>;
  return <div className="commit-files">{files.map((file) => <CommitFile key={file.path} file={file} />)}</div>;
}
function CommitFile({ file }: { file: Prepared }) {
  const [open, setOpen] = useState(false);
  return <details onToggle={(e) => setOpen(e.currentTarget.open)}>
    <summary className="mono small">{file.path} <span className="add">+{file.adds}</span> <span className="del">−{file.dels}</span></summary>
    {open && <InlineDiff rows={file.rows} path={file.path} />}
    {file.note && <div className="small faint">{file.note}</div>}
  </details>;
}
