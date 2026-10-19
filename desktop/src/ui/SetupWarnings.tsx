import { useState } from "react";
import { agentsIn } from "../team";
import { useStore } from "../store";
import { blocks, readSetup, scope, type Warning } from "../setup";
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

/// What is missing from a project's agent setup, the one place each is fixed — and one button that
/// does every part of it Keel can do on its own.
export function SetupList({ project, close }: { project: string; close: () => void; showTab?: (t: "readiness") => void }) {
  const warnings = useStore((s) => s.setup[project]) ?? [];
  const handed = useStore((s) => s.handed[project]);
  const handedLane = useStore((s) => (handed ? s.lanes[handed.lane] : undefined));
  const handedBusy = useHandedBusy(project);
  const runKey = `setup:${project}`;
  const run = useRun<string>(runKey);
  const doing = run?.state;
  const withAgent = new Set(handedLane ? handed!.ids : []);
  const groupOf = (w: Warning) => (withAgent.has(w.id) ? "session" : scope(w));
  const installable = warnings.some((w) => w.id.startsWith("plugin/") || w.id.startsWith("agent/") || (w.fix === "readiness" && !SET_ASIDE.test(w.id)));
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

  async function all() {
    await task(runKey, "Connecting to this project…", async (setDoing) => {
      const ep = await useStore.getState().ensure(project);
      if (!ep) throw new Error("Keel is not running for this project yet — try again in a moment.");
      const refresh = async () => {
        const fresh = await readSetup(ep, project);
        useStore.setState((s) => ({
          setup: { ...s.setup, [project]: fresh.warnings },
          readiness: fresh.readiness ? { ...s.readiness, [project]: fresh.readiness } : s.readiness,
          license: fresh.license ?? s.license,
        }));
        return fresh;
      };
      const { installed, total, team, errors } = await installSetup(ep, warnings, setDoing, async () => {
        window.dispatchEvent(new Event("keel:subagents-changed"));
        await refresh();
      });
      // Re-read after adoption: do not ask an agent to fix findings already resolved.
      setDoing("Checking what is installed…");
      const fresh = await refresh();
      if (team && fresh.warnings.some((w) => w.id === "agent/no-reviewers"))
        errors.push("The agent team is still incomplete. Review the skipped files above or open Subagents in Extensions.");
      const left = fresh.warnings.filter((w) => w.fix === "readiness" && !SET_ASIDE.test(w.id) && !w.id.startsWith("agent/no-reviewers"));
      let handedOff = false;
      let handedTo = "";
      if (left.length && !fresh.warnings.some(blocks)) {
        setDoing("Sending the remaining setup work to a lane…");
        const prompt = [
          "Set this project up for agent work. Keel's readiness check found:",
          ...left.map((w) => `- ${w.title}: ${w.detail}${w.recipe ? `\n  What the check looks for: ${w.recipe}` : ""}`),
          "",
          "Fix each one in the repository. Do not touch anything under .claude/hooks or .claude/settings.json.",
        ].join("\n");
        const lane = repairLane(project, "Project setup");
        useStore.getState().select(lane);
        handedOff = await useStore.getState().send(lane, prompt);
        if (handedOff) useStore.setState((s) => ({ handed: { ...s.handed, [project]: { lane, ids: left.map((w) => w.id) } } }));
        else errors.push(useStore.getState().lanes[lane]?.error || "Could not send the remaining setup work to the lane.");
        if (handedOff) handedTo = `${left.length} item${left.length === 1 ? "" : "s"} handed to the agent in “${useStore.getState().lanes[lane]?.title ?? "the lane"}”. They leave this list when its turn ends and the check passes.`;
      }
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

  if (!warnings.length && !run) return <div className="small muted" style={{ padding: 8 }}>Nothing missing in this project's setup.</div>;
  const asideCount = warnings.filter((w) => SET_ASIDE.test(w.id)).length;
  return (
    <div className="setup-list">
      <div className="setup-head">
        <span className="eyebrow">Setup · {warnings.length} to look at</span>
        <button
          className="primary"
          disabled={!!run?.running || handedBusy}
          onClick={() => { if (installable || !next) void all(); else go(next); }}
          title={handedBusy ? "The agent is still working on what was handed to it" : installable ? "Install the suggested plugins, add Keel's agent team, and ask the agent to fix the rest" : "Open the remaining items for review"}
        >
          {run?.running ? "Installing…" : handedBusy ? "Agent working…" : installable ? "Install all" : next ? FIX[next.fix] : "Check again"}
        </button>
      </div>
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
            {items.map((w) => (
              <div key={`${w.id}:${w.path ?? ""}`} className="setup-item">
                <div className="setup-title">
                  <span className={`setup-sev ${group === "session" ? "info" : blocks(w) || w.severity === "critical" ? "bad" : "warn"}`}>
                    {group === "session" ? (handedBusy ? "With the agent" : "Still open") : blocks(w) ? "Blocks lanes" : SET_ASIDE.test(w.id) ? "Set aside" : w.severity === "critical" ? "Critical" : "Missing"}
                  </span>
                  <span>{w.title}</span>
                </div>
                <div className="small muted">{w.detail}</div>
                <button className="link small" onClick={() => go(w)}>
                  {FIX[w.fix]} →
                </button>
              </div>
            ))}
          </section>
        );
      })}
    </div>
  );
}

/// What the chip says and in what colour. A team update alone is an offer, not a problem, so it
/// reads as one: its own words, in the quiet colour.
function badge(warnings: Warning[] | undefined, running: boolean): { label: string; color: string } {
  const onlyUpdate = warnings?.length === 1 && warnings[0].id === "team/update";
  const reviewOnly = !!warnings?.length && warnings.every((w) => SET_ASIDE.test(w.id));
  const blocking = warnings?.some((w) => w.severity === "critical");
  const label = running ? "Setting up…" : onlyUpdate ? "Team update" : warnings?.length ? `${warnings.length} ${reviewOnly ? "to review" : "to set up"}` : "Setup result";
  return { label, color: blocking ? "var(--del)" : onlyUpdate ? "var(--dim)" : "var(--warn)" };
}

/// The count, where a project is named: quiet when nothing is missing, one click to the list.
export function SetupBadge({ project, showTab }: { project: string; showTab?: (t: "readiness") => void }) {
  const warnings = useStore((s) => s.setup[project]);
  const findings = useStore((s) => s.readiness[project]?.open);
  const run = useRun<string>(`setup:${project}`);
  const [at, setAt] = useState<DOMRect | null>(null);
  if (!warnings?.length && !run) return null;
  const { label, color } = badge(warnings, !!run?.running);
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
        title={`${warnings?.length ?? 0} setup item${warnings?.length === 1 ? "" : "s"}${findings != null ? ` · ${findings} finding${findings === 1 ? "" : "s"} in Readiness` : ""}`}
        aria-label={`${warnings?.length ?? 0} setup warnings`}
      >
        <Icon name="octagon" size={13} /> {label}
      </button>
      {at && (
        <Floating anchor={at} kind="popover" width={380} onClose={() => setAt(null)}>
          <SetupList project={project} close={() => setAt(null)} showTab={showTab} />
        </Floating>
      )}
    </>
  );
}
