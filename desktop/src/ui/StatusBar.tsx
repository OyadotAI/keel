import { useEffect, useState, type ReactNode } from "react";
import { AGENT_NAME, useStore, type Agent, type Lane } from "../store";
import { count } from "./Turn";
import { Icon } from "./icons";
import { label } from "../keys";
import { SetupBadge } from "./SetupWarnings";
import { until } from "./Settings";
import type { Tab } from "./Lane";
import { contextMatches } from "../context";

const clock = (s: number) => (s < 60 ? `${s}s` : `${Math.floor(s / 60)}m ${String(s % 60).padStart(2, "0")}s`);

const VERBS: [RegExp, string][] = [
  [/^Bash$/, "Running"],
  [/^(Grep|Glob|WebSearch)$/, "Searching"],
  [/^(Read|LS|WebFetch)$/, "Reading"],
  [/^Write$/, "Writing"],
  [/^(Edit|MultiEdit)$/, "Editing"],
  [/^(Task|Agent)$/, "Agent —"],
  [/^mcp__keel__browser_/, "Browser —"],
];

/// What the agent is doing now, in one place — ported from the Swift app's working bar. Read off
/// the lane's open turn: a waiting question first, then the checks, then the call still running.
/// Unprefixed, so the sidebar row can use it under the lane's own name.
/// "pm", "pm and designer", "3 agents": the names of agents still working, or how many.
const names = (agents: string[]) => agents.length > 2 ? `${agents.length} agents` : agents.map((a) => a || "an agent").join(" and ");

export function activity(l: Lane): { label: string; since?: string; tone: "idle" | "live" | "warn" } {
  if (l.pending.length) return { label: l.pending.length > 1 ? `paused — ${l.pending.length} questions waiting` : "waiting for you", tone: "warn" };
  const t = l.conv.turns.at(-1);
  if (t?.gate?.status === "running") return { label: `Checking — ${t.gate.command ?? "the project's checks"}`, tone: "live" };
  const background = Object.values(l.conv.background ?? {});
  if (!l.running && background.length) return { label: `${names(background)} working in the background`, tone: "live" };
  if (!l.running) return { label: "idle", tone: "idle" };
  const call = t && Object.values(t.calls).reverse().find((c) => c.state === "running" && !c.parent);
  if (call) {
    const verb = VERBS.find(([re]) => re.test(call.tool))?.[1] ?? call.tool;
    return { label: `${verb} ${call.subject.slice(0, 60)}`, since: t?.startedAt, tone: "live" };
  }
  return { label: t && Object.keys(t.blocks).length ? "Writing…" : "Thinking…", since: t?.startedAt, tone: "live" };
}

function useNow(lane?: string): { label: string; since?: string; tone: "idle" | "live" | "warn" } {
  const l = useStore((s) => (lane ? s.lanes[lane] : undefined));
  if (!l) return { label: "", tone: "idle" };
  const a = activity(l);
  return { ...a, label: `${l.agent} · ${a.label}` };
}

/// The license in force and when it ends; the key itself is changed in Settings.
function LicenseBadge() {
  const license = useStore((s) => s.license);
  if (!license) return null;
  return (
    <button onClick={() => useStore.setState({ settings: true })} title={license.licensee ? `Licensed to ${license.licensee}. Click to change the key.` : `Free tier: ${license.cap} agent steps a day, reset at 00:00 UTC. Click to add a license key.`}>
      {license.licensee ? `Licensed · ${until(license.expires)}` : `Free · ${license.steps}/${license.cap} today`}
    </button>
  );
}

function usageSummary(l: Lane) {
  const cost = l?.conv.turns.reduce((sum, t) => sum + (t.usage?.cost ?? 0), 0) ?? 0;
  // Summed per kind and shown apart: every turn re-reads the cached context, so one total of
  // everything counted the same conversation once per turn and meant nothing.
  const sum = { input: 0, output: 0, cache_read: 0, cache_write: 0, turns: 0 };
  let context: number | undefined;
  for (const t of l?.conv.turns ?? []) {
    if (!t.usage) continue;
    sum.input += t.usage.input;
    sum.output += t.usage.output;
    sum.cache_read += t.usage.cache_read;
    sum.cache_write += t.usage.cache_write;
    sum.turns += 1;
    if (t.usage.context) context = t.usage.context;
  }
  const tokens = sum.input + sum.output + sum.cache_write;
  const inspection = contextMatches(l?.context, l?.agent ?? "", l?.known ? l.session : undefined) ? l?.context : undefined;
  if (inspection?.used != null) context = inspection.used;
  else if (inspection?.compacted_at) context = undefined;
  const percentage = context != null && inspection?.capacity ? Math.round(context / inspection.capacity * 100) : undefined;
  const detail = [
    `${sum.turns} turns with usage`,
    `fresh input ${sum.input.toLocaleString()}`,
    `cache writes ${sum.cache_write.toLocaleString()}`,
    `cache reads ${sum.cache_read.toLocaleString()}`,
    `output ${sum.output.toLocaleString()}`,
    context ? `context at the last turn ${context.toLocaleString()}` : "",
    cost ? `cost $${cost.toFixed(4)}` : "",
  ]
    .filter(Boolean)
    .join("\n");
  return { context, percentage, tokens, sum, cost, detail };
}

function SessionUsage({ lane, open }: { lane: Lane; open: () => void }) {
  const { context, percentage, tokens, sum, cost, detail } = usageSummary(lane);
  return <button className="sb-usage" title={`${detail}\n\nInspect context, memory, and skills`} onClick={open}>
          <span><span className="sb-key">Context</span> <span className="sb-mono">{percentage != null ? `${percentage}%` : context != null ? count(context) : "—"}</span></span>
          {tokens > 0 && (<>
          <span>
            <span className="sb-key">In</span> <span className="sb-mono">{count(sum.input + sum.cache_write)}</span>
          </span>
          <span>
            <span className="sb-key">Out</span> <span className="sb-mono">{count(sum.output)}</span>
          </span>
          {cost > 0 && <span className="sb-mono">${cost.toFixed(2)}</span>}
          </>)}
        </button>;
}

function CurrentActivity({ now, elsewhere }: { now: ReturnType<typeof useNow>; elsewhere: boolean }) {
  const elapsed = now.since ? Math.floor((Date.now() - Date.parse(now.since)) / 1000) : undefined;
  return <button className={`sb-now ${now.tone}`} title="What the agent is doing">
    {now.tone === "warn" ? <Icon name="hand" size={14} /> : <span className={now.tone === "live" ? "busy-dot" : "sb-dot"} />}
    <span className="sb-agent">{AGENT_NAME[now.label.split(" · ")[0] as Agent] ?? "Claude"}</span>
    <span className="sb-label">{elsewhere ? "running in another terminal — Keel is following" : now.label.replace(/^\S+ · /, "")}</span>
    {!elsewhere && elapsed !== undefined && Number.isFinite(elapsed) && <span className="clock">{clock(elapsed)}</span>}
  </button>;
}

function branchLabel(lane: Lane): string {
  if (!lane.git) return lane.wt ? `keel/${lane.wt}` : "…";
  return lane.git.is_repo ? lane.git.branch ?? "detached" : "not a git repository";
}

export function StatusBar({ openTab, panel, togglePanel, sidebar, toggleSidebar, layout }: { openTab: (tab: Tab) => void; panel: boolean; togglePanel: () => void; sidebar: boolean; toggleSidebar: () => void; layout?: ReactNode }) {
  const lane = useStore((s) => s.active);
  const l = useStore((s) => (s.active ? s.lanes[s.active] : undefined));
  const project = useStore((s) => (l ? s.projects[l.project] : undefined));
  const trusted = useStore((s) => (l ? !!s.trusted[l.project] : false));
  const now = useNow(lane);
  const elsewhere = !!l?.elsewhere;
  // One shared tick, only while something runs. At rest no interval exists.
  const [, tick] = useState(0);
  useEffect(() => {
    if (now.tone !== "live") return;
    const t = setInterval(() => tick((n) => n + 1), 500);
    return () => clearInterval(t);
  }, [now.tone]);
  const git = l?.git;
  return (
    <footer className="statusbar">
      <div className="statusbar-details">
      <button className="sb-icon" onClick={toggleSidebar} aria-label={sidebar ? "Hide projects" : "Show projects"} aria-pressed={sidebar} title={`Toggle projects (${label({ key: "e", shift: true })})`}><Icon name="folder" size={15} /></button>
      {l && (
        <button className="sb-branch" onClick={() => openTab("git")} title="Open Git">
          <Icon name="branch" size={14} />
          <span className="sb-mono">{branchLabel(l)}</span>
          {git && git.changes.length > 0 && <span className="sb-pill">{git.changes.length} changed</span>}
        </button>
      )}
      {l && now.label && <span className="sb-sep" />}
      {l && now.label && <CurrentActivity now={now} elsewhere={elsewhere} />}
      <span className="spacer" />
      {l && <SessionUsage lane={l} open={() => openTab("context")} />}
      {l && <SetupBadge project={l.project} showTab={() => openTab("readiness")} />}
      {l &&
        (trusted ? (
          <button className="sb-trusted" onClick={() => useStore.setState({ settings: true })} title="Commands and MCP tools here run without asking. Click to change.">
            <Icon name="shield" size={14} /> Trusted
          </button>
        ) : (
          <button onClick={() => useStore.setState({ settings: true })} title="Every command outside the allowlist asks first. Trusting the project lets the agent run them without asking.">
            <Icon name="shield" size={14} /> Trust this project…
          </button>
        ))}
      <LicenseBadge />
      {project?.error ? (
        <button className="bad" onClick={() => l && useStore.getState().load(l.id)}>
          Keel stopped for this project — Restart
        </button>
      ) : project && !project.endpoint && project.starting ? (
        <button className="warn">Starting Keel…</button>
      ) : null}
      <span className="sb-sep" />
      <button className="sb-shortcuts" onClick={() => useStore.setState({ shortcuts: true })} title="Every keyboard shortcut">
        Shortcuts <kbd>{label({ key: "/" })}</kbd>
      </button>
      </div>
      {layout}
      {l && (
        <button className="workspace-toggle" onClick={togglePanel} aria-label={panel ? "Close workspace" : "Open workspace"} aria-expanded={panel} aria-controls={panel ? "workspace-panel" : undefined} title={`${panel ? "Close" : "Open"} workspace · Changes, Git, preview and readiness (${label({ key: "i", alt: true })})`}>
          <Icon name="panel" size={14} />
          <span>Workspace</span>
        </button>
      )}
    </footer>
  );
}
