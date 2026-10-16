import { useCallback, useEffect, useRef, useState, type ReactNode } from "react";
import { get, post, url, type Endpoint } from "../api";
import { prepare, type DiffResponse, type Prepared } from "../git";
import { start, useRun } from "../runs";
import { useStore } from "../store";
import { SourceControl } from "./SourceControl";
import { InlineDiff } from "./InlineDiff";
import { type GraphCommit } from "../commit-graph";
import { CommitGraph } from "./CommitGraph";
import { useIde } from "../ide";
import { Count, Tabs } from "./kit";
import { Icon } from "./icons";

interface Commit {
  sha: string;
  subject: string;
  when: number;
  files: number;
  pushed: boolean;
}
interface Branch {
  name: string;
  current: boolean;
  upstream?: string | null;
  ahead: number;
  behind: number;
}
interface Branches {
  current?: string | null;
  local: Branch[];
  remotes: string[];
  staged: number;
  unstaged: number;
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
  const [branches, setBranches] = useState<Branches>();
  const [log, setLog] = useState<Commit[]>();
  const [error, setError] = useState<string>();
  const [readError, setReadError] = useState<string>();
  const [note, setNote] = useState<string>();
  const [section, setSection] = useState<GitSection>("changes");

  const read = useCallback(async (e: Endpoint) => {
    // Settled, so a failed branch read keeps the history it has, and a good read clears the banner.
    const [b, c] = await Promise.allSettled([get<Branches>(e, "/api/git/branches", { wt }), get<Commit[]>(e, "/api/git/log", { wt, n: 20 })]);
    if (b.status === "fulfilled") setBranches(b.value);
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
    if (locked.current) return;
    locked.current = true;
    setPending(true);
    setError(undefined);
    setNote(undefined);
    try {
      const r = await run();
      if (typeof r === "string" && r) setNote(r);
      else setNote(what);
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
    }
    await useStore.getState().refreshGit(lane);
    if (ep) await read(ep);
    setPending(false);
    locked.current = false;
  }

  if (!ep) return <div className="panel-empty">Starting Keel for this project…</div>;
  if (git && !git.is_repo)
    return (
      <div className="panel-empty">
        <h3>Not a git repository</h3>
        <p>Keel commits each turn and gives lanes their own branches, and both need git.</p>
        <button className="primary" onClick={() => act("Initialised a git repository.", () => post(ep, "/api/git/init", {}, { wt }))}>
          Initialise git here
        </button>
      </div>
    );
  const current = branches?.local.find((b) => b.current);
  const content = <GitContent lane={lane} ep={ep} wt={wt} act={act} section={section} log={log} branches={branches} pending={pending} running={running} onOpenDiff={onOpenDiff} selection={selection} />;
  const feedback = <GitFeedback error={error || readError} note={note} retry={() => void read(ep)} />;
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

      </div>
      <RemoteActions ep={ep} wt={wt} current={current} running={running} pending={pending} act={act} remotes={branches?.remotes} />
      <LaneGitActions wt={wt} busy={running || pending} />
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
  lane: string; ep: Endpoint; wt?: string; act: Act; section: GitSection; log?: Commit[]; branches?: Branches; pending: boolean; running: boolean;
  onOpenDiff?: (path: string, staged: boolean) => void; selection?: { path?: string; staged?: boolean };
}
function GitContent({ lane, ep, wt, act, section, log, branches, pending, running, onOpenDiff, selection }: GitContentProps) {
  switch (section) {
    case "changes": return <SourceControl lane={lane} ep={ep} wt={wt} act={act} onOpenDiff={onOpenDiff} selection={selection} />;
    case "history": return <History lane={lane} ep={ep} wt={wt} log={log} act={act} />;
    case "branches": return <BranchList ep={ep} wt={wt} branches={branches} act={act} />;
    case "pr": return <PullRequest ep={ep} wt={wt} />;
    case "remotes": return <><RemoteActions ep={ep} wt={wt} current={branches?.local.find((b) => b.current)} running={running} pending={pending} act={act} remotes={branches?.remotes} /><LaneGitActions wt={wt} busy={running || pending} /></>;
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
  const [changesOpen, setChangesOpen] = useState(true);
  return <div className="git-panel coding-git">
    <header className="coding-git-heading" onKeyDown={(e) => { if (e.key === "Escape") setMenu(false); }}><strong>Source Control</strong>
      <button className="ghost" aria-label="More source control actions" aria-expanded={menu} onClick={() => setMenu(!menu)}>···</button>
      {menu && <><button className="coding-menu-backdrop" aria-label="Close source control menu" onClick={() => setMenu(false)} /><div className="coding-git-menu" onKeyDown={(e) => { if (e.key === "Escape") setMenu(false); }}>
        {([ ["changes", "Changes"], ["branches", "Branches…"], ["history", "History & undo commit…"], ["remotes", "Remotes & sync…"], ["pr", "Pull request…"] ] as const).map(([id, label]) => <button key={id} onClick={() => { setSection(id); setChangesOpen(true); setMenu(false); }}>{label}</button>)}
      </div></>}
    </header>
    {feedback}
    <section className={`coding-changes${changesOpen ? "" : " collapsed"}`} aria-label="Working changes">
      <button className="coding-section-toggle" aria-expanded={changesOpen} onClick={() => setChangesOpen(!changesOpen)}><Icon name={changesOpen ? "chevron-down" : "chevron-right"} size={12} />{section === "changes" ? "Changes" : section === "pr" ? "Pull request" : section[0].toUpperCase() + section.slice(1)}<Count n={git?.changes.length} /></button>
      {changesOpen && <div className={section === "changes" ? "git-body" : "git-scroll"}>
        {section !== "changes" && <button className="link coding-back" onClick={() => setSection("changes")}>← Back to changes</button>}
        {children}
      </div>}
    </section>
    <CommitGraph ep={ep} wt={wt} revision={git} selected={selected} onOpen={(commit) => { useIde.getState().patch(lane, { commit, follow: false }); onOpenCommit?.(); }} tools={<GraphRemoteActions ep={ep} wt={wt} current={current} pending={pending} running={running} remotes={branches?.remotes} act={act} />} />
    <button className="coding-branch-status" title="Manage branches" onClick={() => { setSection("branches"); setChangesOpen(true); }}><Icon name="branch" size={13} /><span>{branches?.current ?? git?.branch ?? "No commits yet"}</span><small>{current?.behind ?? 0}↓ {current?.ahead ?? 0}↑</small></button>
  </div>;
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

type Act = (what: string, run: () => Promise<unknown>) => Promise<void>;

function RemoteActions({ ep, wt, current, running, pending, act, remotes }: { ep: Endpoint; wt?: string; current?: Branch; running: boolean; pending: boolean; act: Act; remotes?: string[] }) {
  const [remote, setRemote] = useState("");
  return (<><div className="git-remote-actions" role="group" aria-label="Remote actions">
        <button disabled={pending} onClick={() => act("Fetched all remotes.", () => post(ep, "/api/git/remote", { action: "fetch" }, { wt }))}>Fetch</button>
        <button disabled={pending || running || !current?.upstream} title={running ? "Wait for the agent to finish" : current?.upstream ? "Fast-forward only" : "Set an upstream before pulling"} onClick={() => act("Pulled upstream changes.", () => post(ep, "/api/git/remote", { action: "pull" }, { wt }))}>Pull{current?.behind ? ` ↓${current.behind}` : ""}</button>
        <button disabled={pending || running} title={current?.upstream ? `Push to ${current.upstream}` : "Push and set upstream"} onClick={() => act("Pushed.", () => post<string>(ep, "/api/git/push", {}, { wt }))}>Push{current?.ahead ? ` ↑${current.ahead}` : ""}</button>
      </div>
      {remotes?.length === 0 && <div className="git-add-remote"><input className="field" aria-label="Origin remote URL" placeholder="https://github.com/owner/repo.git" value={remote} onChange={(e) => setRemote(e.target.value)} /><button disabled={pending || !remote.trim()} onClick={() => act("Added origin.", () => post(ep, "/api/git/remote", { action: "add", url: remote.trim() }, { wt }))}>Add origin</button></div>}
    </>);
}

function LaneGitActions({ wt, busy }: { wt?: string; busy: boolean }) {
  if (!wt) return null;
  return <div className="git-remote-actions" role="group" aria-label="Lane branch actions"><button disabled={busy} onClick={() => useStore.setState({ asking: "merge" })}>Merge lane…</button><button disabled={busy} onClick={() => useStore.setState({ asking: "discard" })}>Discard lane…</button></div>;
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

function BranchList({ ep, wt, branches, act }: { ep: Endpoint; wt?: string; branches?: Branches; act: Act }) {
  const [name, setName] = useState("");
  if (!branches) return <div className="panel-empty">Reading branches…</div>;
  const branch = (action: string, n: string, what: string) => act(what, () => post(ep, "/api/git/branch", { action, name: n }, { wt }));
  return (
    <div>
      <div style={{ padding: "10px 12px", borderBottom: "1px solid var(--line)", display: "flex", gap: 8 }}>
        <input className="field" style={{ margin: 0, flex: 1, maxWidth: "none" }} value={name} onChange={(e) => setName(e.target.value)} placeholder="new-branch-name" aria-label="New branch name" />
        <button disabled={!name.trim()} onClick={() => (branch("create", name.trim(), `Created and switched to ${name.trim()}.`), setName(""))}>
          Create
        </button>
      </div>
      {branches.local.map((b) => (
        <div key={b.name} className="change" style={{ alignItems: "center" }}>
          <span className="marker">{b.current && <Icon name="check-circle" size={12} style={{ color: "var(--accent)" }} />}</span>
          <span className="change-path" style={{ fontWeight: b.current ? 600 : 400 }}>
            {b.name}
          </span>
          <span className="mono faint" style={{ fontSize: 10 }}>
            {b.ahead ? `↑${b.ahead} ` : ""}
            {b.behind ? `↓${b.behind}` : ""}
          </span>
          {!b.current && (
            <>
              {/* A lane's checkout is its branch: switching it would put the lane's commits on another branch. */}
              {!wt && (
                <button className="link small" onClick={() => branch("checkout", b.name, `Switched to ${b.name}.`)}>
                  Switch
                </button>
              )}
              <button className="ghost" aria-label={`Delete ${b.name}`} title="Delete (only if merged)" onClick={() => branch("delete", b.name, `Deleted ${b.name}.`)}>
                <Icon name="x" size={12} />
              </button>
            </>
          )}
        </div>
      ))}
    </div>
  );
}

function PullRequest({ ep, wt }: { ep: Endpoint; wt?: string }) {
  const [title, setTitle] = useState("");
  const [draft, setDraft] = useState(false);
  // Kept outside the panel: switching tabs mid-push must not offer a second pull request.
  const key = `pr:${ep.port}:${wt ?? ""}`;
  const run = useRun<{ lines: string[]; result?: "done" | "failed" }>(key);
  const lines = run?.state.lines ?? [];
  const state = run?.running ? "running" : (run?.state.result ?? "idle");
  function open() {
    start(
      key,
      url(ep, "/api/github/pr", { title, draft: draft ? "true" : undefined, wt }),
      ep.token,
      { lines: [] as string[] } as { lines: string[]; result?: "done" | "failed" },
      (s, e) => ({
        lines: e.event === "line" || e.event === "fatal" ? [...s.lines, String(e.data)].slice(-100) : s.lines,
        result: e.event === "done" ? (e.data === "0" || e.data === 0 ? "done" : "failed") : e.event === "fatal" ? "failed" : s.result,
      }),
      (s) => ({ ...s, result: s.result ?? "failed" }),
    );
  }
  const link = lines.map((l) => l.match(/https:\/\/github\.com\/\S+\/pull\/\d+/)?.[0]).find(Boolean);
  return (
    <div style={{ padding: "10px 12px" }}>
      <p className="small muted" style={{ marginTop: 0 }}>
        Pushes this branch and opens a pull request with <code>gh</code>. Leave the title empty to let it come from the commits.
      </p>
      <input className="field" style={{ maxWidth: "none", width: "100%" }} value={title} onChange={(e) => setTitle(e.target.value)} placeholder="Title (optional)" aria-label="Pull request title" />
      <label className="small" style={{ display: "flex", gap: 6, alignItems: "center", margin: "6px 0" }}>
        <input type="checkbox" checked={draft} onChange={(e) => setDraft(e.target.checked)} /> Open as a draft
      </label>
      <button className="primary" disabled={state === "running"} onClick={open}>
        {state === "running" ? "Opening…" : "Open pull request"}
      </button>
      {link && (
        <p>
          <a href={link}>{link}</a>
        </p>
      )}
      {lines.length > 0 && <pre className="code">{lines.join("\n")}</pre>}
    </div>
  );
}
