import { useState } from "react";
import { post, url } from "../api";
import { useStore } from "../store";
import { blocks, readSetup, type Warning } from "../setup";
import { open as openStream } from "../streams";
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

/// A plugin install, to its end. The CLI streams; this waits for it to finish either way.
function install(ep: { port: number; token: string }, name: string, marketplace: string): Promise<boolean> {
  return new Promise((done) => {
    let ok = false;
    openStream(
      url(ep, "/api/plugins/install", { name, marketplace, commit: true }),
      ep.token,
      (events) => {
        for (const e of events) if (e.event === "done") ok = String(e.data) === "0";
      },
      () => done(ok),
    );
  });
}

/// The repository's own agent hooks and config: Keel sets them aside before every run, and an
/// agent asked to "fix" them would delete somebody's hooks. Known about, never auto-fixed.
const SET_ASIDE = /^security\/untrusted-agent-config$/;

/// What is missing from a project's agent setup, the one place each is fixed — and one button that
/// does every part of it Keel can do on its own.
export function SetupList({ project, close }: { project: string; close: () => void; showTab?: (t: "readiness") => void }) {
  const warnings = useStore((s) => s.setup[project]) ?? [];
  const [doing, setDoing] = useState<string>();
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
    const ep = await useStore.getState().ensure(project).catch(() => undefined);
    if (!ep) return setDoing("Keel is not running for this project yet — try again in a moment.");
    const plugins = warnings.filter((w) => w.id.startsWith("plugin/") && w.marketplace);
    let failed = 0;
    for (const [i, w] of plugins.entries()) {
      setDoing(`Installing ${w.id.slice(7)} (${i + 1} of ${plugins.length})…`);
      if (!(await install(ep, w.id.slice(7), w.marketplace!))) failed++;
    }
    // Keel's own team — agents, the lifecycle, the instructions — written only where missing.
    if (warnings.some((w) => w.id.startsWith("agent/"))) {
      setDoing("Adding the agent team…");
      await post(ep, "/api/adopt", undefined, { commit: true }).catch(() => failed++);
    }
    // The rest is the project's to change, and the agent in it is who changes it.
    const left = warnings.filter((w) => w.fix === "readiness" && !SET_ASIDE.test(w.id) && !w.id.startsWith("agent/no-reviewers"));
    if (left.length) {
      const prompt = ["Set this project up for agent work. Keel's readiness check found:", ...left.map((w) => `- ${w.title}: ${w.detail}`), "", "Fix each one in the repository. Do not touch anything under .claude/hooks or .claude/settings.json."].join("\n");
      const lane = laneIn(project);
      useStore.getState().select(lane);
      void useStore.getState().send(lane, prompt);
    }
    const fresh = await readSetup(ep, project).catch(() => undefined);
    if (fresh) useStore.setState((s) => ({ setup: { ...s.setup, [project]: fresh.warnings } }));
    const needsYou = warnings.some(blocks);
    setDoing(
      [
        plugins.length ? `${plugins.length - failed} of ${plugins.length} plugins installed.` : "",
        left.length ? "Asked the agent to fix the rest — watch its terminal." : "",
        needsYou ? "Claude Code itself needs you: open Settings." : "",
        failed ? `${failed} step${failed === 1 ? "" : "s"} failed — see Extensions.` : "",
      ]
        .filter(Boolean)
        .join(" ") || "Done.",
    );
  }

  if (!warnings.length) return <div className="small muted" style={{ padding: 8 }}>Nothing missing in this project's setup.</div>;
  const asideCount = warnings.filter((w) => SET_ASIDE.test(w.id)).length;
  return (
    <div className="setup-list">
      <div className="setup-head">
        <span className="eyebrow">Setup · {warnings.length} to look at</span>
        <button className="primary" disabled={!!doing && doing.endsWith("…")} onClick={() => void all()} title="Install the suggested plugins, add Keel's agent team, and ask the agent to fix the rest">
          Install all
        </button>
      </div>
      {doing && <div className="small setup-doing">{doing}</div>}
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
  const [at, setAt] = useState<DOMRect | null>(null);
  if (!warnings?.length) return null;
  const blocking = warnings.some((w) => w.severity === "critical");
  return (
    <>
      <button
        className="setup-badge"
        style={{ color: blocking ? "var(--del)" : "var(--warn)" }}
        onClick={(e) => {
          e.stopPropagation();
          setAt(e.currentTarget.getBoundingClientRect());
        }}
        title={`${warnings.length} thing${warnings.length === 1 ? "" : "s"} missing from this project's setup`}
        aria-label={`${warnings.length} setup warnings`}
      >
        <Icon name="octagon" size={13} /> {warnings.length} to set up
      </button>
      {at && (
        <Floating anchor={at} kind="popover" width={380} onClose={() => setAt(null)}>
          <SetupList project={project} close={() => setAt(null)} showTab={showTab} />
        </Floating>
      )}
    </>
  );
}
