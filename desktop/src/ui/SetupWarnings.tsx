// The setup chip and its popover: only what stops a lane from working, and what Keel's own setup
// needs from the person. Suggestions live in Extensions and Readiness, not here.
import { useState } from "react";
import { useStore } from "../store";
import { post } from "../api";
import { blocks, chipState, recordShown, resultLines, scope, type SetupRecord, type Warning } from "../setup";
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

const GROUPS = [
  ["folder", "This folder"],
  ["global", "Global"],
] as const;

/// Whether the automatic setup's record needs the person: it failed, or it is waiting on their
/// uncommitted edit. Running, done and waiting on a turn are Keel's business, and say so in Git.
const needsYou = (r: SetupRecord) => r.phase === "failed" || (r.phase === "waiting" && r.waiting?.on === "you");

/// Ask the daemon to run its setup pass again, failures forgotten.
async function retrySetup(project: string) {
  const ep = await useStore.getState().ensure(project);
  if (ep) void post(ep, "/api/setup/run").catch(() => undefined);
}

/// One warning: its severity word, title, detail and the link that fixes it.
function SetupRow({ w, go }: { w: Warning; go: (w: Warning) => void }) {
  return (
    <div className="setup-item">
      <div className="setup-title">
        <span className={`setup-sev ${blocks(w) || w.severity === "critical" ? "bad" : "warn"}`}>{blocks(w) ? "Blocks lanes" : w.severity === "critical" ? "Critical" : "Check"}</span>
        <span>{w.title}</span>
      </div>
      <div className="small muted">{w.detail}</div>
      <button className="link small" onClick={() => go(w)}>{FIX[w.fix]} →</button>
    </div>
  );
}

/// What stops a lane in this project, the one place each is fixed, and what Keel's own setup did
/// when it needs the person. No install button and no hand-off to an agent: a first run is not
/// the moment to spend steps on someone's files.
export function SetupList({ project, close }: { project: string; close: () => void; showTab?: (t: "readiness") => void }) {
  const warnings = useStore((s) => s.setup[project]) ?? [];
  const trusted = useStore((s) => !!s.trusted[project]);
  const record = useStore((s) => s.setupRun[project]);
  const auto = trusted && recordShown(record) ? record : undefined;
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
  const openGit = () => {
    close();
    useStore.getState().select(laneIn(project));
    useStore.setState({ settings: false, extensions: false, creating: false, panelTab: "git" });
  };
  const lines = auto && (auto.phase === "done" || auto.phase === "failed" || auto.phase === "waiting") ? resultLines(auto) : [];
  if (!warnings.length && !lines.length) return <div className="small muted" style={{ padding: 8 }}>Nothing is stopping this project's lanes.</div>;
  return (
    <div className="setup-list">
      <div className="setup-head">
        <span className="eyebrow">{auto?.phase === "failed" ? "Setup · failed" : auto && needsYou(auto) ? "Setup · waiting on you" : `Setup · ${warnings.length} to look at`}</span>
        {auto?.phase === "failed" && <button className="primary" onClick={() => void retrySetup(project)} title="Run Keel's setup again">Retry</button>}
      </div>
      {lines.length > 0 && (
        <div className="small setup-doing setup-result" role="status">
          {lines.map((line) => <div key={line}>{line}</div>)}
          <button className="link small" onClick={openGit}>Open Git →</button>
        </div>
      )}
      {GROUPS.map(([group, label]) => {
        const items = warnings.filter((w) => scope(w) === group);
        if (!items.length) return null;
        return (
          <section key={group} className="setup-group" aria-label={label}>
            <div className="eyebrow setup-group-head">{label}</div>
            {items.map((w) => <SetupRow key={`${w.id}:${w.path ?? ""}`} w={w} go={go} />)}
          </section>
        );
      })}
    </div>
  );
}

/// The chip, where a project is named: absent unless something stops a lane or Keel's own setup
/// needs the person. A brand-new project shows nothing.
export function SetupBadge({ project, showTab }: { project: string; showTab?: (t: "readiness") => void }) {
  const warnings = useStore((s) => s.setup[project]);
  const trusted = useStore((s) => !!s.trusted[project]);
  const record = useStore((s) => s.setupRun[project]);
  const [at, setAt] = useState<DOMRect | null>(null);
  const auto = trusted && recordShown(record) && needsYou(record) ? record : undefined;
  if (!warnings?.length && !auto) return null;
  const { label, color, aria, icon } = chipState(auto, warnings, trusted, false);
  const closed = () => setAt(null);
  return (
    <>
      <button
        className="setup-badge"
        style={{ color }}
        onClick={(e) => {
          e.stopPropagation();
          setAt(e.currentTarget.getBoundingClientRect());
        }}
        title={auto ? "Keel's setup needs you — click for what happened" : `${warnings?.length ?? 0} thing${warnings?.length === 1 ? "" : "s"} stopping lanes in this project`}
        aria-label={aria}
      >
        <Icon name={icon} size={13} /> {label}
      </button>
      {at && (
        <Floating anchor={at} kind="popover" width={380} onClose={closed}>
          <SetupList project={project} close={closed} showTab={showTab} />
        </Floating>
      )}
    </>
  );
}
