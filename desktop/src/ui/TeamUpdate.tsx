// "Team update available": Oya Factory's newer team files, offered to a project that has an older team.
import { useCallback, useEffect, useState } from "react";
import { post, type Endpoint } from "../api";
import { task, useRun } from "../runs";
import { absent, offered, pending, readTeam, updatedSentence, type TeamPlan, type TeamUpdated } from "../team";
import { useStore } from "../store";
import { recordShown, type SetupRecord } from "../setup";
import { Banner } from "./kit";

interface Shown { tone: "note" | "error"; text: string }

const KEPT = "Edited since Oya Factory wrote it, so Oya Factory leaves it alone.";
const DELETED = "You deleted it, so Oya Factory doesn't put it back.";
const DIRTY = "Updated, not committed: it had changes that weren't committed yet.";
const YOURS = "Oya Factory's section is in it, but it had your uncommitted changes, so Oya Factory didn't commit it.";

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
  const trusted = useStore((s) => !!s.trusted[project]);
  const record = useStore((s) => s.setupRun[project]);
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
  const added = !!plan?.add.some((p) => p.startsWith(".agents/") || p.startsWith(".codex/"));
  // A team the project already has is kept current on its own, trusted or not; only adding one
  // waits on trust. Either way this card then says what is happening, not what to click.
  if ((trusted || plan?.present) && recordShown(record)) {
    const auto = <AutoTeam ep={ep} record={record} />;
    if (!offered(plan) || !plan) return <>{auto}{banner}</>;
    return <div className="team-install">
      <h3>Agent team · kept current by Oya Factory</h3>
      <p className="small muted">Oya Factory's team changed since this project added it. Files you haven't edited are replaced and new ones added, on their own, as a commit named "Update the agent team". Files you changed stay as they are. {absent(plan)}</p>
      {auto}{banner}
      <Group title="Replaced" paths={plan.replace} />
      <Group title="Added" paths={plan.add} sub={added ? "For Antigravity (.agents/) and Codex (.codex/agents/)." : undefined} />
      <Group title="Kept, you changed it" paths={plan.kept} reason={KEPT} />
      <Group title="Not restored" paths={plan.not_restored} reason={DELETED} />
    </div>;
  }
  if (!offered(plan) || !plan) return <>{banner}{last && <Group title="Waiting on your commit" paths={last.uncommitted} reason={YOURS} />}</>;
  // One row and its button; the why and the file lists are a click away, since they can run long.
  return <div className="team-install team-offer">
    <div className="team-offer-row"><strong>Team update available</strong><span className="small muted">{pending(plan)} files</span><span className="spacer" /><button className="primary" disabled={run?.running} onClick={update}>{run?.running ? "Updating…" : "Update team"}</button></div>
    {banner}
    <details className="delivery-section"><summary>What changes</summary>
      <p className="small muted">Oya Factory's agent team changed since this project added it. Files you haven't edited are replaced and new ones added, as one commit, "Update the agent team"; files you changed stay as they are. {absent(plan)}</p>
      <Group title="Replaced" paths={plan.replace} />
      <Group title="Added" paths={plan.add} sub={added ? "For Antigravity (.agents/) and Codex (.codex/agents/)." : undefined} />
      <Group title="Kept, you changed it" paths={plan.kept} reason={KEPT} />
      <Group title="Not restored" paths={plan.not_restored} reason={DELETED} />
    </details>
  </div>;
}

/** What the automatic team update is doing or last did, as one banner; a failure gets the only button. */
function AutoTeam({ ep, record }: { ep: Endpoint; record: SetupRecord }) {
  const t = record.team, on = record.waiting?.on;
  if (record.phase === "running") return <Banner tone="note">{record.step ?? "Updating the agent team…"}</Banner>;
  if (record.phase === "waiting" && on === "you") return <Banner tone="note">Waiting on your commit: {record.waiting?.paths.join(", ")} has your uncommitted changes, so Oya Factory left it alone.</Banner>;
  if (record.phase === "waiting") return <Banner tone="note">{on === "turn" || on === "terminal" || on === "keel" ? "Waiting: a turn is writing this project. The update lands when it ends." : `Waiting: a ${on ?? "merge"} is in progress.`}</Banner>;
  if (record.phase === "failed" && record.error) return <Banner tone="error">Couldn’t update the team: {record.error} <button onClick={() => void post(ep, "/api/setup/run").catch(() => undefined)}>Try again</button></Banner>;
  if (!t) return null;
  return <Banner tone="note">{t.kind === "updated" ? "Team updated" : "Team added"}{t.commit ? ` in commit ${t.commit}` : ""}: {t.files.length} file{t.files.length === 1 ? "" : "s"}.{t.kept.length ? ` Kept ${t.kept.length} you changed.` : ""}{t.note ? ` ${t.note}.` : ""}</Banner>;
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
