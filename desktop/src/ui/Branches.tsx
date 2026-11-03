// Search and switch branches without hiding checkout ownership or destroying the current draft.
import { useRef, useState } from "react";
import { post, type Endpoint } from "../api";
import { branchResults, branchOwner, branchFailure, branchCommitAge, type Branches as BranchData, type BranchRow, type BranchFilter, type BranchSort, type GitAction } from "../branches";
import { useStore } from "../store";
import { Icon } from "./icons";
import { Floating } from "./Menu";

interface Props { lane: string; ep: Endpoint; wt?: string; branches?: BranchData; pending: boolean; running: boolean; act: GitAction; error?: string; retry?: () => void }

/** Keep discovery controls active while a serialized mutation or unavailable checkout blocks switching. */
export function BranchList({ lane, ep, wt, branches, pending, running, act, error, retry }: Props) {
  const [query, setQuery] = useState("");
  const [filter, setFilter] = useState<BranchFilter>("all");
  const [sort, setSort] = useState<BranchSort>("name");
  const [name, setName] = useState("");
  const [switching, setSwitching] = useState<string>();
  const [failure, setFailure] = useState<string>();
  const lock = useRef(false);
  const summary = useRef<HTMLDivElement>(null);
  const checkoutBusy = useStore((s) => Object.values(s.lanes).some((l) => l.project === s.lanes[lane]?.project && (l.wt ?? "") === (wt ?? "") && !!l.running));
  if (!branches) return <div className="panel-empty">{error ? <>{error}<button onClick={retry}>Retry</button></> : "Reading branches…"}</div>;
  const results = branchResults(branches, query, filter, sort);
  const checkoutPaused = !wt && (running || checkoutBusy);
  const reason = wt ? "This lane owns its branch. Switch branches from a project-workspace lane." : checkoutPaused ? undefined : branches.restriction;
  const disabled = pending || checkoutPaused || !!reason || !!switching;
  async function mutate(action: string, target: string, kind?: string) {
    if (lock.current || disabled) return false;
    lock.current = true; setFailure(undefined); setSwitching(action === "checkout" ? `${kind}:${target}` : undefined);
    const trigger = document.activeElement;
    try {
      const ok = await act(action === "delete" ? `Deleted ${target}.` : `Switched to ${target}.`, async () => {
        try { return await post(ep, "/api/git/branch", { action, name: target, kind }, { wt }); }
        catch (error) { throw new Error(branchFailure(error)); }
      });
      if (ok && action !== "delete") requestAnimationFrame(() => {
        if (document.activeElement === trigger || document.activeElement === document.body) summary.current?.focus();
      });
      return ok;
    } catch (error) { setFailure(branchFailure(error)); return false; }
    finally { lock.current = false; setSwitching(undefined); }
  }
  return <section className="branch-browser" aria-label="Branches">
    <div ref={summary} tabIndex={-1} className="branch-current"><Icon name="branch" size={15} /><span title={branches.current ?? branches.head ?? ""}>{branches.current ?? (branches.head ? `Detached HEAD · ${branches.head}` : "No branches yet")}</span><strong>{branches.current ? "Current" : ""}</strong></div>
    {error && <div className="branch-notice error" role="alert">Couldn’t refresh branches. Showing the last loaded list. <button onClick={retry}>Retry</button></div>}
    <div className="branch-search"><input className="field" aria-label="Search branches" placeholder="Find a branch…" value={query} onChange={(e) => setQuery(e.target.value)} />{query && <button aria-label="Clear branch search" onClick={() => setQuery("")}>Clear</button>}</div>
    <div className="branch-controls"><div className="segmented" role="group" aria-label="Branch filter">{(["all", "local", "remote"] as const).map((value) => <button key={value} aria-pressed={filter === value} onClick={() => setFilter(value)}>{value[0].toUpperCase() + value.slice(1)}</button>)}</div><label>Sort <select aria-label="Sort branches" value={sort} onChange={(e) => setSort(e.target.value as BranchSort)}><option value="name">Name</option><option value="commit">Latest commit</option></select></label></div>
    <div role="status" aria-live="polite" aria-atomic="true">{checkoutPaused && <div className="branch-busy-notice"><Icon name="octagon" size={16} /><div><strong>Branch switching is paused</strong><p>An agent is using this checkout. Wait for its turn to finish, or stop the turn before switching.</p></div></div>}</div>
    <p className="branch-count small muted" role="status">{results.count} {results.count === 1 ? "branch" : "branches"}</p>
    {reason && <p className="branch-notice small muted">{reason}</p>}
    {failure && <p className="branch-notice error" role="alert">{failure}</p>}
    {results.rows.map((branch) => <BranchItem key={`${branch.kind}:${branch.name}`} branch={branch} lane={lane} disabled={disabled} switching={switching === `${branch.kind}:${branch.name}`} mutate={mutate} />)}
    {!results.count && <p className="branch-notice muted">{query ? <>No branches match ‘{query}’. <button onClick={() => setQuery("")}>Clear search</button></> : filter === "remote" ? "No remote-tracking branches recorded locally." : "No branches yet. Create the first commit to establish a branch."}</p>}
    <div className="branch-create"><input className="field" aria-label="New branch name" placeholder="new-branch-name" value={name} onChange={(e) => setName(e.target.value)} /><button disabled={disabled || !name.trim()} onClick={() => void mutate("create", name.trim()).then((ok) => { if (ok) setName(""); })}>Create</button></div>
  </section>;
}

function BranchItem({ branch, lane, disabled, switching, mutate }: { branch: BranchRow; lane: string; disabled: boolean; switching: boolean; mutate: (action: string, name: string, kind?: string) => Promise<boolean> }) {
  const owner = useStore((s) => branchOwner(branch, s.lanes[lane]?.project, Object.values(s.lanes)));
  const [menu, setMenu] = useState<DOMRect | null>(null);
  const timestamp = branch.when ? new Date(branch.when * 1000) : undefined;
  const date = timestamp && Number.isFinite(timestamp.getTime()) ? timestamp : undefined;
  return <article className="branch-item">
    <div className="branch-name" title={branch.name}>{branch.name}</div>
    <div className="branch-meta"><span>{branch.kind === "local" ? "Local" : `Remote · ${branch.remote}`}</span>{date && <time dateTime={date.toISOString()} title={date.toLocaleString()}>Commit {branchCommitAge(branch.when!)}</time>}{!!branch.ahead && <span>↑{branch.ahead}</span>}{!!branch.behind && <span>↓{branch.behind}</span>}</div>
    {branch.subject && <div className="branch-subject small muted" title={branch.subject}>{branch.subject}</div>}
    {branch.worktree ? <div className="branch-occupancy small muted">{owner ? <>Open in {owner.title} <button onClick={() => useStore.getState().select(owner.id)}>Open lane</button></> : <>In another worktree <span className="mono">{branch.worktree.path}</span></>}</div> : <div className="branch-actions"><button aria-label={`Switch to ${branch.name}`} disabled={disabled} title={branch.kind === "remote" ? "Create or use a local tracking branch." : undefined} onClick={() => void mutate("checkout", branch.name, branch.kind)}>{switching ? "Switching…" : "Switch"}</button>{branch.kind === "local" && <button className="ghost" aria-label={`Actions for ${branch.name}`} disabled={disabled} onClick={(e) => setMenu(e.currentTarget.getBoundingClientRect())}><Icon name="more" size={14} /></button>}</div>}
    {menu && <Floating anchor={menu} onClose={() => setMenu(null)}><button disabled={disabled} onClick={() => { setMenu(null); void mutate("delete", branch.name, "local"); }}>Delete branch (merged only)</button></Floating>}
  </article>;
}
