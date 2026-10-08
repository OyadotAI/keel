import { useState } from "react";
import { post } from "../api";
import { useStore } from "../store";
import { blocks, readSetup, type Warning } from "../setup";
import { install } from "../setup-install";
import { task, useRun } from "../runs";
import { Floating } from "./Menu";
import { Icon } from "./icons";

const FIX: Record<Warning["fix"], string> = { settings: "Open Settings", extensions: "Open Extensions", readiness: "Open Readiness" };

/// The lane in `project` to hand work to: the one on screen if it is this project's, its first
/// otherwise, a new one if it has none.
function laneIn(project: string): string {
  const s = useStore.getState();
  if (s.active && s.lanes[s.active]?.project === project) return s.active;
  const first = s.projects[project]?.lanes[0];
  return first ?? s.newLane(project);
}

/// The repository's own agent hooks and config: Keel sets them aside before every run, and an
/// agent asked to "fix" them would delete somebody's hooks. Known about, never auto-fixed.
const SET_ASIDE = /^security\/untrusted-agent-config$/;

/// What is missing from a project's agent setup, the one place each is fixed — and one button that
/// does every part of it Keel can do on its own.
export function SetupList({ project, close }: { project: string; close: () => void; showTab?: (t: "readiness") => void }) {
  const warnings = useStore((s) => s.setup[project]) ?? [];
  const runKey = `setup:${project}`;
  const run = useRun<string>(runKey);
  const doing = run?.state;
  const go = (w: Warning) => {
    close();
    if (w.fix === "settings") useStore.setState({ settings: true, extensions: false, creating: false });
    else if (w.fix === "extensions") useStore.setState({ extensions: true, settings: false, creating: false });
    else {
      // From the sidebar the project may not be the one on screen: go to it, then to its tab.
      useStore.getState().select(laneIn(project));
      useStore.setState({ settings: false, extensions: false, creating: false, panelTab: "readiness" });
    }
  };

  async function all() {
    await task(runKey, "Connecting to this project…", async (setDoing) => {
      const ep = await useStore.getState().ensure(project);
      if (!ep) throw new Error("Keel is not running for this project yet — try again in a moment.");
      const errors: string[] = [];
      const plugins = warnings.filter((w) => w.id.startsWith("plugin/") && w.marketplace);
      let installed = 0;
      for (const [i, w] of plugins.entries()) {
        setDoing(`Installing ${w.id.slice(7)} (${i + 1} of ${plugins.length})…`);
        const error = await install(ep, w.id.slice(7), w.marketplace!);
        if (error) errors.push(`${w.id.slice(7)}: ${error}`);
        else installed++;
      }
      if (warnings.some((w) => w.id.startsWith("agent/"))) {
        setDoing("Adding the agent team…");
        await post(ep, "/api/adopt", undefined, { commit: true }).catch((error) => errors.push(`Agent team: ${error}`));
      }
      // Re-read after adoption: do not ask an agent to fix findings already resolved.
      const fresh = await readSetup(ep, project);
      useStore.setState((s) => ({ setup: { ...s.setup, [project]: fresh.warnings } }));
      const left = fresh.warnings.filter((w) => w.fix === "readiness" && !SET_ASIDE.test(w.id) && !w.id.startsWith("agent/no-reviewers"));
      let handedOff = false;
      if (left.length && !fresh.warnings.some(blocks)) {
        setDoing("Sending the remaining setup work to a lane…");
        const prompt = ["Set this project up for agent work. Keel's readiness check found:", ...left.map((w) => `- ${w.title}: ${w.detail}`), "", "Fix each one in the repository. Do not touch anything under .claude/hooks or .claude/settings.json."].join("\n");
        const lane = laneIn(project);
        useStore.getState().select(lane);
        handedOff = await useStore.getState().send(lane, prompt);
        if (!handedOff) errors.push(useStore.getState().lanes[lane]?.error || "Could not send the remaining setup work to the lane.");
      }
      setDoing([
        plugins.length ? `${installed} of ${plugins.length} plugins installed.` : "",
        handedOff ? "Sent the remaining setup work to the lane (queued if it is busy)." : "",
        fresh.warnings.some(blocks) ? "Claude Code needs your attention: open Settings." : "",
        ...errors,
      ].filter(Boolean).join("\n") || "Done.");
    }, (error) => `Setup failed: ${error instanceof Error ? error.message : String(error)}`);
  }

  if (!warnings.length && !run) return <div className="small muted" style={{ padding: 8 }}>Nothing missing in this project's setup.</div>;
  const asideCount = warnings.filter((w) => SET_ASIDE.test(w.id)).length;
  return (
    <div className="setup-list">
      <div className="setup-head">
        <span className="eyebrow">Setup · {warnings.length} to look at</span>
        <button className="primary" disabled={!!run?.running} onClick={() => void all()} title="Install the suggested plugins, add Keel's agent team, and ask the agent to fix the rest">
          Install all
        </button>
      </div>
      {doing && <div className="small setup-doing" role="status" style={{ whiteSpace: "pre-wrap" }}>{doing}</div>}
      {asideCount > 0 && (
        <div className="small muted setup-aside">
          {asideCount} hook script{asideCount === 1 ? "" : "s"} or config file{asideCount === 1 ? "" : "s"} from the repository: Keel sets them aside before every agent run, so nothing runs them. Review them in Readiness.
        </div>
      )}
      {warnings.map((w) => (
        <div key={w.id} className="setup-item">
          <div className="setup-title">
            <span className={`setup-sev ${blocks(w) ? "bad" : w.severity === "critical" ? "bad" : "warn"}`}>{blocks(w) ? "Blocks lanes" : SET_ASIDE.test(w.id) ? "Set aside" : w.severity === "critical" ? "Critical" : "Missing"}</span>
            <span>{w.title}</span>
          </div>
          <div className="small muted">{w.detail}</div>
          <button className="link small" onClick={() => go(w)}>
            {FIX[w.fix]} →
          </button>
        </div>
      ))}
    </div>
  );
}

/// The count, where a project is named: quiet when nothing is missing, one click to the list.
export function SetupBadge({ project, showTab }: { project: string; showTab?: (t: "readiness") => void }) {
  const warnings = useStore((s) => s.setup[project]);
  const run = useRun<string>(`setup:${project}`);
  const [at, setAt] = useState<DOMRect | null>(null);
  if (!warnings?.length && !run) return null;
  const blocking = warnings?.some((w) => w.severity === "critical");
  return (
    <>
      <button
        className="setup-badge"
        style={{ color: blocking ? "var(--del)" : "var(--warn)" }}
        onClick={(e) => {
          e.stopPropagation();
          setAt(e.currentTarget.getBoundingClientRect());
        }}
        title={`${warnings?.length ?? 0} thing${warnings?.length === 1 ? "" : "s"} missing from this project's setup`}
        aria-label={`${warnings?.length ?? 0} setup warnings`}
      >
        <Icon name="octagon" size={13} /> {run?.running ? "Setting up…" : warnings?.length ? `${warnings.length} to set up` : "Setup result"}
      </button>
      {at && (
        <Floating anchor={at} kind="popover" width={380} onClose={() => setAt(null)}>
          <SetupList project={project} close={() => setAt(null)} showTab={showTab} />
        </Floating>
      )}
    </>
  );
}
