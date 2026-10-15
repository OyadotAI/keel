import { useCallback, useEffect, useState } from "react";
import { get, post, url, type Endpoint } from "../api";
import { prepare, type DiffResponse, type Prepared } from "../git";
import { start, useRun } from "../runs";
import { useStore } from "../store";
import { SourceControl } from "./SourceControl";
import { InlineDiff } from "./InlineDiff";
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
export function Git({ lane }: { lane: string }) {
  const { ep, wt } = useLane(lane);
  const git = useStore((s) => s.lanes[lane]?.git);
  const [branches, setBranches] = useState<Branches>();
  const [log, setLog] = useState<Commit[]>();
  const [error, setError] = useState<string>();
  const [readError, setReadError] = useState<string>();
  const [note, setNote] = useState<string>();
  const [section, setSection] = useState<"changes" | "history" | "branches" | "pr">("changes");

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
        {current && current.ahead > 0 && (
          <button onClick={() => act("Pushed.", () => post<string>(ep, "/api/git/push", {}, { wt }))} title={current.upstream ? `Push to ${current.upstream}` : "Push and set the upstream"}>
            Push {current.ahead}
          </button>
        )}
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
      {(error || readError) && <div className="panel-note small error" role="alert">{error || readError}</div>}
      {note && !error && !readError && <div className="panel-note small faint">{note}</div>}
      <div className={section === "changes" ? "git-body" : "git-scroll"}>
        {section === "changes" && <SourceControl lane={lane} ep={ep} wt={wt} act={act} />}
        {section === "history" && <History lane={lane} ep={ep} wt={wt} log={log} act={act} />}
        {section === "branches" && <BranchList ep={ep} wt={wt} branches={branches} act={act} />}
        {section === "pr" && <PullRequest ep={ep} wt={wt} />}
      </div>
    </div>
  );
}

type Act = (what: string, run: () => Promise<unknown>) => Promise<void>;

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
  useEffect(() => {
    get<DiffResponse[]>(ep, "/api/git/commit/diff", { sha, wt }).then((d) => setFiles(d.slice(0, 30).map(prepare)), () => setFiles([]));
  }, [ep, wt, sha]);
  if (!files) return <div className="small faint" style={{ padding: "4px 32px" }}>Reading…</div>;
  return (
    <div style={{ padding: "0 12px 10px 32px" }}>
      {files.map((f) => (
        <details key={f.path}>
          <summary className="mono small" style={{ cursor: "pointer", padding: "2px 0" }}>
            {f.path} <span className="add">+{f.adds}</span> <span className="del">−{f.dels}</span>
          </summary>
          <InlineDiff rows={f.rows} />
          {f.note && <div className="small faint">{f.note}</div>}
        </details>
      ))}
    </div>
  );
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
