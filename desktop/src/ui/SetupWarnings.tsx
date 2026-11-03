import { useState } from "react";
import { agentsIn } from "../team";
import { useStore } from "../store";
import { post, type Endpoint } from "../api";
import { automatic, blocks, chipState, readSetup, recordShown, resultLines, scope, type SetupRecord, type Warning } from "../setup";
import { installSetup } from "../setup-install";
import { repairLane } from "../repairs";
import { task, useRun } from "../runs";
import { Floating } from "./Menu";
import { Icon } from "./icons";

const FIX: Record<Warning["fix"], string> = { settings: "Open Settings", extensions: "Open Extensions", readiness: "Open Readiness" };

/// The lane in `project` to hand work to: the one on screen if it is this project's, its first
/// otherwise, a new one if it has none. Never one on its own branch: the setup check reads the
/// project's own tree, so a fix made on `keel/<name>` would leave every item listed as missing.
function laneIn(project: string): string {
  const s = useStore.getState();
  const shared = (lid?: string) => !!lid && s.lanes[lid]?.project === project && !s.lanes[lid]?.isolated;
  if (shared(s.active)) return s.active!;
  return s.projects[project]?.lanes.find(shared) ?? s.newLane(project);
}

/// The lane setup work was handed to, while it still has that work: running, sending, or with
/// it queued. A second "Install all" then would send the same prompt again behind the first.
function useHandedBusy(project: string): boolean {
  return useStore((s) => {
    const l = s.lanes[s.handed[project]?.lane ?? ""];
    return !!l && (l.running || !!l.submitting || l.queued.length > 0);
  });
}

const GROUPS = [
  ["session", "This session"],
  ["folder", "This folder"],
  ["global", "Global"],
] as const;

/// The repository's own agent hooks and config: Keel sets them aside before every run, and an
/// agent asked to "fix" them would delete somebody's hooks. Known about, never auto-fixed.
const SET_ASIDE = /^security\/untrusted-agent-config$/;

/// The findings only an agent can fix: everything the automatic setup and the set-aside rule leave.
const forAgent = (warnings: Warning[]) => warnings.filter((w) => w.fix === "readiness" && !SET_ASIDE.test(w.id) && !w.id.startsWith("agent/no-reviewers"));

/// Per window: the result the person has already opened and closed, so the chip can go quiet.
const seen = new Map<string, number>();

/// What a row for one of the automatic setup's own items says, from the daemon's record: the
/// warning is the fact, the record is what Keel is doing about it.
function automaticRow(w: Warning, record: SetupRecord): { sev: string; kind: string; detail: string } | undefined {
  const plugin = w.id.startsWith("plugin/") ? record.plugins.find((p) => p.name === w.id.slice(7)) : undefined;
  if (plugin?.error) return { sev: "Failed", kind: "bad", detail: `${plugin.error} Not retried until this project is next opened.` };
  if (record.phase === "failed" && record.error && !w.id.startsWith("plugin/")) return { sev: "Failed", kind: "bad", detail: record.error };
  if (record.phase === "running") return { sev: "Setting up", kind: "info", detail: w.id.startsWith("plugin/") ? "Installs after the team." : "Keel is writing it now." };
  if (record.phase === "waiting" && record.waiting?.on === "you") return { sev: "Yours", kind: "warn", detail: `${record.waiting.paths.join(", ")} has changes you haven't committed. Keel's section goes in once they're committed or stashed, so your edit isn't touched.` };
  if (record.phase === "waiting") {
    const on = record.waiting?.on;
    const what = on === "turn" || on === "terminal" || on === "keel" ? "A turn is writing this tree. Keel does it when the turn ends — nothing to do." : `A ${on} is in progress. Nothing was written; Keel does it once it's finished.`;
    return { sev: "Waiting", kind: "quiet", detail: what };
  }
  return undefined;
}

/// Everything a click still does: in an untrusted project the installs and then the hand-off; in
/// a trusted one only the hand-off, since Keel's own parts ran, or are running, on their own.
async function runSetup(project: string, trusted: boolean, warnings: Warning[]) {
  const runKey = `setup:${project}`;
  const refreshFor = async (ep: Endpoint) => {
    const fresh = await readSetup(ep, project);
    useStore.setState((s) => ({
      setup: { ...s.setup, [project]: fresh.warnings },
      readiness: fresh.readiness ? { ...s.readiness, [project]: fresh.readiness } : s.readiness,
      license: fresh.license ?? s.license,
    }));
    return fresh;
  };
  await task(runKey, "Connecting to this project…", async (setDoing) => {
    const ep = await useStore.getState().ensure(project);
    if (!ep) throw new Error("Keel is not running for this project yet — try again in a moment.");
    const errors: string[] = [];
    let team: Awaited<ReturnType<typeof installSetup>>["team"];
    let installed = 0, total = 0;
    if (!trusted) {
      const r = await installSetup(ep, warnings, setDoing, async () => {
        window.dispatchEvent(new Event("keel:subagents-changed"));
        await refreshFor(ep);
      });
      ({ team, installed, total } = r);
      errors.push(...r.errors);
    }
    // Re-read after adoption: do not ask an agent to fix findings already resolved.
    setDoing("Checking what is installed…");
    const fresh = await refreshFor(ep);
    if (team && fresh.warnings.some((w) => w.id === "agent/no-reviewers"))
      errors.push("The agent team is still incomplete. Review the skipped files above or open Subagents in Extensions.");
    const handedTo = await handoff(project, setDoing, fresh, errors);
    setDoing([
      team ? (agentsIn(team.written) ? `Agent team added for ${agentsIn(team.written)}.` : "Agent team checked.") : "",
      team?.note ?? "",
      total ? `${installed} of ${total} plugins installed.` : "",
      handedTo,
      fresh.warnings.some(blocks) ? "Claude Code needs your attention: open Settings." : "",
      ...errors,
      fresh.warnings.length ? `${fresh.warnings.length} ${fresh.warnings.length === 1 ? "item still needs" : "items still need"} attention.` : errors.length ? "Review the installer errors before retrying." : "Setup complete.",
    ].filter(Boolean).join("\n"));
  }, (error) => `Setup failed: ${error instanceof Error ? error.message : String(error)}`);
}

/// The one thing that stays behind a button in a trusted project: an agent turn on the person's
/// own files, which spends steps and is theirs to start.
async function handoff(project: string, setDoing: (m: string) => void, fresh: { warnings: Warning[] }, errors: string[]): Promise<string> {
  const items = forAgent(fresh.warnings);
  if (!items.length || fresh.warnings.some(blocks)) return "";
  setDoing("Sending the remaining setup work to a lane…");
  const prompt = [
    "Set this project up for agent work. Keel's readiness check found:",
    ...items.map((w) => `- ${w.title}: ${w.detail}${w.recipe ? `\n  What the check looks for: ${w.recipe}` : ""}`),
    "",
    "Fix each one in the repository. Do not touch anything under .claude/hooks or .claude/settings.json.",
  ].join("\n");
  const lane = repairLane(project, "Project setup");
  useStore.getState().select(lane);
  const ok = await useStore.getState().send(lane, prompt);
  if (!ok) { errors.push(useStore.getState().lanes[lane]?.error || "Could not send the remaining setup work to the lane."); return ""; }
  useStore.setState((s) => ({ handed: { ...s.handed, [project]: { lane, ids: items.map((w) => w.id) } } }));
  return `${items.length} item${items.length === 1 ? "" : "s"} handed to the agent in “${useStore.getState().lanes[lane]?.title ?? "the lane"}”. They leave this list when its turn ends and the check passes.`;
}

/// Ask the daemon to run its setup pass again, failures forgotten.
async function retrySetup(project: string) {
  const ep = await useStore.getState().ensure(project);
  if (ep) void post(ep, "/api/setup/run").catch(() => undefined);
}

/// The popover's one button: what an agent still has to do, a retry, or today's installer.
function HeadButton({ trusted, left, running, handedBusy, blocked, failed, installable, next, run, retry, go }: { trusted: boolean; left: number; running: boolean; handedBusy: boolean; blocked: boolean; failed: boolean; installable: boolean; next?: Warning; run: () => void; retry: () => void; go: (w: Warning) => void }) {
  if (trusted && left > 0) return (
    <button className="primary" disabled={running || handedBusy || blocked} onClick={run}
      title={blocked ? "Claude Code needs your attention: open Settings" : handedBusy ? "The agent is still working on what was handed to it" : "Opens a lane and asks the agent to fix the remaining readiness findings. This runs a turn: it spends steps and edits your own files."}>
      {running ? "Sending…" : handedBusy ? "Agent working…" : `Send ${left} to an agent`}
    </button>
  );
  if (trusted) return failed ? <button className="primary" onClick={retry} title="Run Keel's setup again">Retry</button> : null;
  return (
    <button className="primary" disabled={running || handedBusy} onClick={() => { if (installable || !next) run(); else go(next); }}
      title={handedBusy ? "The agent is still working on what was handed to it" : installable ? "Install the suggested plugins, add Keel's agent team, and ask the agent to fix the rest" : "Open the remaining items for review"}>
      {running ? "Installing…" : handedBusy ? "Agent working…" : installable ? "Install all" : next ? FIX[next.fix] : "Check again"}
    </button>
  );
}

/// One warning: its severity word, title, detail and the link that fixes it — from the record
/// when the automatic setup answers for it.
function SetupRow({ w, group, auto, handedBusy, go, retry, openGit }: { w: Warning; group: string; auto?: SetupRecord; handedBusy: boolean; go: (w: Warning) => void; retry: () => void; openGit: () => void }) {
  const row = auto && automatic(w) ? automaticRow(w, auto) : undefined;
  const sev = row ? row.kind : group === "session" ? "info" : blocks(w) || w.severity === "critical" ? "bad" : "warn";
  const word = row ? row.sev : group === "session" ? (handedBusy ? "With the agent" : "Still open") : blocks(w) ? "Blocks lanes" : SET_ASIDE.test(w.id) ? "Set aside" : w.severity === "critical" ? "Critical" : "Missing";
  const title = row?.sev === "Failed" && w.id.startsWith("plugin/") ? w.title.replace("is not installed", "didn't install") : w.title;
  return (
    <div className="setup-item">
      <div className="setup-title">
        <span className={`setup-sev ${sev}`}>{word}</span>
        <span>{title}</span>
      </div>
      <div className="small muted">{row?.detail ?? w.detail}</div>
      {row?.sev === "Failed" ? <button className="link small" onClick={retry}>Retry →</button>
        : row?.sev === "Yours" ? <button className="link small" onClick={openGit}>Open Git →</button>
        : <button className="link small" onClick={() => go(w)}>{FIX[w.fix]} →</button>}
    </div>
  );
}

/// What is missing from a project's agent setup, the one place each is fixed — and one button that
/// does every part of it Keel can do on its own. In a trusted project Keel does those parts on its
/// own, and the button is only for what needs an agent.
export function SetupList({ project, close }: { project: string; close: () => void; showTab?: (t: "readiness") => void }) {
  const warnings = useStore((s) => s.setup[project]) ?? [];
  const trusted = useStore((s) => !!s.trusted[project]);
  const record = useStore((s) => s.setupRun[project]);
  const auto = trusted && recordShown(record) ? record : undefined;
  const handed = useStore((s) => s.handed[project]);
  const handedLane = useStore((s) => (handed ? s.lanes[handed.lane] : undefined));
  const handedBusy = useHandedBusy(project);
  const runKey = `setup:${project}`;
  const run = useRun<string>(runKey);
  const doing = run?.state;
  const withAgent = new Set(handedLane ? handed!.ids : []);
  const groupOf = (w: Warning) => (withAgent.has(w.id) ? "session" : scope(w));
  const installable = !trusted && warnings.some((w) => w.id.startsWith("plugin/") || w.id.startsWith("agent/") || (w.fix === "readiness" && !SET_ASIDE.test(w.id)));
  const left = trusted ? forAgent(warnings) : [];
  const next = warnings.find(blocks) ?? warnings[0];
  const go = (w: Warning) => {
    close();
    if (w.fix === "settings") useStore.setState({ settings: true, extensions: false, creating: false });
    else if (w.fix === "extensions") useStore.setState({ extensions: true, settings: false, creating: false, ...(w.id === "team/update" ? { extensionsSection: "agents" as const } : {}) });
    else {
      // From the sidebar the project may not be the one on screen: go to it, then to its tab.
      useStore.getState().select(laneIn(project));
      useStore.setState({ settings: false, extensions: false, creating: false, panelTab: "readiness" });
    }
  };
  const openGit = () => {
    close();
    useStore.getState().select(laneIn(project));
    useStore.setState({ settings: false, extensions: false, creating: false, panelTab: "git" });
  };

  const retry = () => void retrySetup(project);

  if (!warnings.length && !run && !auto) return <div className="small muted" style={{ padding: 8 }}>Nothing missing in this project's setup.</div>;
  const asideCount = warnings.filter((w) => SET_ASIDE.test(w.id)).length;
  const eyebrow = auto?.phase === "running" ? "Setup · running" : auto?.phase === "done" ? "Setup · done" : auto?.phase === "waiting" ? (auto.waiting?.on === "you" ? `Setup · ${auto.waiting.paths.length || 1} waiting on you` : "Setup · waiting") : `Setup · ${warnings.length} to look at`;
  const lines = auto && (auto.phase === "done" || auto.phase === "failed" || auto.phase === "waiting") ? resultLines(auto) : [];
  const failed = auto?.phase === "failed";
  return (
    <div className="setup-list">
      <div className="setup-head">
        <span className="eyebrow">{eyebrow}</span>
        <HeadButton trusted={trusted} left={left.length} running={!!run?.running} handedBusy={handedBusy} blocked={warnings.some(blocks)} failed={failed} installable={installable} next={next} run={() => void runSetup(project, trusted, warnings)} retry={retry} go={go} />
      </div>
      {auto?.phase === "running" && <div className="small setup-doing setup-status" role="status">{auto.step ?? "Setting up…"}</div>}
      {lines.length > 0 && (
        <div className="small setup-doing setup-result" role="status">
          <div>{auto!.trigger === "click" ? "Keel ran its setup when you asked." : auto!.team?.kind === "updated" ? "Keel updated its agent team on open." : "Keel set this project up on open."}</div>
          {lines.map((line) => <div key={line}>{line}</div>)}
          <button className="link small" onClick={openGit}>Open Git →</button>
        </div>
      )}
      {doing && <div className="small setup-doing" role="status" style={{ whiteSpace: "pre-wrap" }}>{doing}</div>}
      {asideCount > 0 && (
        <div className="small muted setup-aside">
          {asideCount} hook script{asideCount === 1 ? "" : "s"} or config file{asideCount === 1 ? "" : "s"} from the repository: Keel sets them aside before every agent run, so nothing runs them. Review them in Readiness.
        </div>
      )}
      {GROUPS.map(([group, label]) => {
        const items = warnings.filter((w) => groupOf(w) === group);
        if (!items.length) return null;
        return (
          <section key={group} className="setup-group" aria-label={label}>
            <div className="eyebrow setup-group-head">{label}</div>
            {items.map((w) => <SetupRow key={`${w.id}:${w.path ?? ""}`} w={w} group={group} auto={auto} handedBusy={handedBusy} go={go} retry={retry} openGit={openGit} />)}
          </section>
        );
      })}
    </div>
  );
}

/// The count, where a project is named: quiet when nothing is missing, one click to the list. In
/// a trusted project it also says what Keel did on its own, until that has been read once.
export function SetupBadge({ project, showTab }: { project: string; showTab?: (t: "readiness") => void }) {
  const warnings = useStore((s) => s.setup[project]);
  const findings = useStore((s) => s.readiness[project]?.open);
  const trusted = useStore((s) => !!s.trusted[project]);
  const record = useStore((s) => s.setupRun[project]);
  const run = useRun<string>(`setup:${project}`);
  const [at, setAt] = useState<DOMRect | null>(null);
  const read = recordShown(record) && record.phase === "done" && seen.get(project) === record.at;
  const auto = trusted && recordShown(record) && !read;
  if (!warnings?.length && !run && !auto) return null;
  const { label, color, aria } = chipState(trusted && !read ? record : undefined, warnings, trusted, !!run?.running);
  const title = auto && record.phase === "done" ? "Keel set this project up on open — click for what it did" : auto && record.phase === "running" ? "Keel is adding its agent team and suggested plugins to this project" : `${warnings?.length ?? 0} setup item${warnings?.length === 1 ? "" : "s"}${findings != null ? ` · ${findings} finding${findings === 1 ? "" : "s"} in Readiness` : ""}`;
  const closed = () => {
    setAt(null);
    if (recordShown(record) && record.phase === "done") seen.set(project, record.at);
  };
  return (
    <>
      <button
        className="setup-badge"
        style={{ color }}
        onClick={(e) => {
          e.stopPropagation();
          setAt(e.currentTarget.getBoundingClientRect());
        }}
        // The chip counts setup only; Readiness counts every finding. Saying both stops the two
        // numbers on screen reading as a disagreement.
        title={title}
        aria-label={aria}
      >
        <Icon name="octagon" size={13} /> {label}
      </button>
      {at && (
        <Floating anchor={at} kind="popover" width={380} onClose={closed}>
          <SetupList project={project} close={closed} showTab={showTab} />
        </Floating>
      )}
    </>
  );
}
