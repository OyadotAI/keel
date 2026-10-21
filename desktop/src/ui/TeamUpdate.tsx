// "Team update available": Keel's newer team files, offered to a project that has an older team.
import { useCallback, useEffect, useState } from "react";
import { post, type Endpoint } from "../api";
import { task, useRun } from "../runs";
import { absent, offered, pending, readTeam, updatedSentence, type TeamPlan, type TeamUpdated } from "../team";
import { useStore } from "../store";
import { Banner } from "./kit";

interface Shown { tone: "note" | "error"; text: string }

const KEPT = "Edited since Keel wrote it, so Keel leaves it alone.";
const DELETED = "You deleted it, so Keel doesn't put it back.";
const DIRTY = "Updated, not committed: it had changes that weren't committed yet.";

/** One group of paths, open when it is short enough to read at a glance. */
function Group({ title, paths, reason, sub, dirty = [] }: { title: string; paths: string[]; reason?: string; sub?: string; dirty?: string[] }) {
  if (!paths.length) return null;
  return <details className="team-group" open={paths.length <= 5}>
    <summary>{title} ({paths.length})</summary>
    {sub && <p className="small muted">{sub}</p>}
    <ul>{paths.map((p) => <li key={p}><code title={p}>{p}</code>{(reason || dirty.includes(p)) && <span className="small muted"> — {dirty.includes(p) ? DIRTY : reason}</span>}</li>)}</ul>
  </details>;
}

/** The inline panel in Extensions › Subagents. The project root, or a lane of its own when the root is busy. */
export function TeamUpdate({ ep, project, plan, reread }: { ep: Endpoint; project: string; plan?: TeamPlan; reread: () => void }) {
  const key = `team-update:${project}`;
  const run = useRun<Shown>(key);
  const [last, setLast] = useState<TeamUpdated>();
  const finished = !!run && !run.running;
  useEffect(() => { if (finished) reread(); }, [finished, reread]);
  function update() {
    void task<Shown>(key, { tone: "note", text: `Updating ${pending(plan)} files…` }, async (put) => {
      const r = await post<TeamUpdated>(ep, "/api/team/update");
      setLast(r);
      // Written beside the busy tree rather than refused: open the lane, where it can be read and finished.
      if (r.lane) {
        const store = useStore.getState();
        const lid = store.newLane(project, true);
        useStore.setState((s) => ({ lanes: { ...s.lanes, [lid]: { ...s.lanes[lid], wt: r.lane!, title: "Update the agent team" } } }));
        store.select(lid);
      }
      put({ tone: "note", text: updatedSentence(r) });
      window.dispatchEvent(new Event("keel:subagents-changed"));
    }, (e) => ({ tone: "error", text: e instanceof Error ? e.message : String(e) }));
  }
  const banner = run && <Banner tone={run.state.tone}>{run.state.text}</Banner>;
  if (!offered(plan) || !plan) return <>{banner}{last && <Group title="Updated, not committed" paths={last.uncommitted} reason={DIRTY} />}</>;
  const added = plan.add.some((p) => p.startsWith(".agents/") || p.startsWith(".codex/"));
  return <div className="team-install">
    <h3>Team update available · {pending(plan)} files</h3>
    <p className="small muted">Keel's agent team has changed since this project added it. Files you haven't edited are replaced and new ones are added; files you changed stay as they are. {absent(plan)}</p>
    {/* Before the lists, which can run long: the button is what this panel is for. */}
    <button className="primary" disabled={run?.running} onClick={update}>{run?.running ? "Updating…" : "Update team"}</button>
    <p className="small muted">Commits only the files below, as "Update the agent team".</p>
    {banner}
    <Group title="Replaced" paths={plan.replace} />
    <Group title="Added" paths={plan.add} sub={added ? "For Antigravity (.agents/) and Codex (.codex/agents/)." : undefined} />
    <Group title="Kept, you changed it" paths={plan.kept} reason={KEPT} />
    <Group title="Not restored" paths={plan.not_restored} reason={DELETED} />
  </div>;
}

/** The plan, read when the panel mounts and again whenever something changes the team. */
export function useTeamPlan(ep: Endpoint): [TeamPlan | undefined, () => void] {
  const [plan, setPlan] = useState<TeamPlan>();
  const [tick, setTick] = useState(0);
  // A reply for the project left behind is dropped, not drawn over the one on screen.
  useEffect(() => {
    let live = true;
    readTeam(ep).then((p) => { if (live) setPlan(p); }, () => { if (live) setPlan(undefined); });
    return () => { live = false; };
  }, [ep, tick]);
  useEffect(() => {
    const again = () => setTick((n) => n + 1);
    window.addEventListener("keel:subagents-changed", again);
    return () => window.removeEventListener("keel:subagents-changed", again);
  }, []);
  const reread = useCallback(() => setTick((n) => n + 1), []);
  return [plan, reread];
}
