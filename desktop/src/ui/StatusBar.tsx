import { useEffect, useState } from "react";
import { useStore, type Lane } from "../store";
import { count } from "./Turn";
import { Icon } from "./icons";
import { label } from "../keys";
import { SetupBadge } from "./SetupWarnings";
import type { Tab } from "./Lane";

const clock = (s: number) => (s < 60 ? `${s}s` : `${Math.floor(s / 60)}m ${String(s % 60).padStart(2, "0")}s`);

const VERBS: [RegExp, string][] = [
  [/^Bash$/, "Running"],
  [/^(Grep|Glob|WebSearch)$/, "Searching"],
  [/^(Read|LS|WebFetch)$/, "Reading"],
  [/^Write$/, "Writing"],
  [/^(Edit|MultiEdit)$/, "Editing"],
  [/^(Task|Agent)$/, "Agent —"],
];

/// What the agent is doing now, in one place — ported from the Swift app's working bar. Read off
/// the lane's open turn: a waiting question first, then the checks, then the call still running.
/// Unprefixed, so the sidebar row can use it under the lane's own name.
export function activity(l: Lane): { label: string; since?: string; tone: "idle" | "live" | "warn" } {
  if (l.pending.length) return { label: l.pending.length > 1 ? `paused — ${l.pending.length} questions waiting` : "waiting for you", tone: "warn" };
  const t = l.conv.turns.at(-1);
  if (t?.gate?.status === "running") return { label: `Checking — ${t.gate.command ?? "the project's checks"}`, tone: "live" };
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

export function StatusBar({ openTab, panel, togglePanel, sidebar, toggleSidebar }: { openTab: (tab: Tab) => void; panel: boolean; togglePanel: () => void; sidebar: boolean; toggleSidebar: () => void }) {
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
  const cost = l?.conv.turns.reduce((sum, t) => sum + (t.usage?.cost ?? 0), 0) ?? 0;
  // Summed per kind and shown apart: every turn re-reads the cached context, so one total of
  // everything counted the same conversation once per turn and meant nothing.
  const sum = { input: 0, output: 0, cache_read: 0, cache_write: 0, turns: 0 };
  let context = 0;
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
  const git = l?.git;
  const elapsed = now.since ? Math.floor((Date.now() - Date.parse(now.since)) / 1000) : undefined;
  return (
    <footer className="statusbar">
      <button className="sb-icon" onClick={toggleSidebar} aria-label={sidebar ? "Hide projects" : "Show projects"} aria-pressed={sidebar} title={`Toggle projects (${label({ key: "e", shift: true })})`}><Icon name="folder" size={15} /></button>
      {l && (
        <button className="sb-branch" onClick={() => openTab("git")} title="Open Git">
          <Icon name="branch" size={14} />
          <span className="sb-mono">{git ? (git.is_repo ? (git.branch ?? "detached") : "not a git repository") : l.wt ? `keel/${l.wt}` : "…"}</span>
          {git && git.changes.length > 0 && <span className="sb-pill">{git.changes.length} changed</span>}
        </button>
      )}
      {l && now.label && <span className="sb-sep" />}
      {l && now.label && (
        <button className={`sb-now ${now.tone}`} title="What the agent is doing">
          {now.tone === "warn" ? <Icon name="hand" size={14} /> : <span className={now.tone === "live" ? "busy-dot" : "sb-dot"} />}
          <span className="sb-agent">{l.agent === "codex" ? "Codex" : "Claude"}</span>
          <span className="sb-label">{elsewhere ? "running in another terminal — Keel is following" : now.label.replace(/^\S+ · /, "")}</span>
          {!elsewhere && elapsed !== undefined && Number.isFinite(elapsed) && <span className="clock">{clock(elapsed)}</span>}
        </button>
      )}
      <span className="spacer" />
      {tokens > 0 && (
        <button className="sb-usage" title={`${detail}\n\nClick to copy`} onClick={() => void navigator.clipboard.writeText(detail)}>
          {context > 0 && (
            <span>
              <span className="sb-key">Context</span> <span className="sb-mono">{count(context)}</span>
            </span>
          )}
          <span>
            <span className="sb-key">In</span> <span className="sb-mono">{count(sum.input + sum.cache_write)}</span>
          </span>
          <span>
            <span className="sb-key">Out</span> <span className="sb-mono">{count(sum.output)}</span>
          </span>
          {cost > 0 && <span className="sb-mono">${cost.toFixed(2)}</span>}
        </button>
      )}
      {l && <SetupBadge project={l.project} showTab={() => openTab("readiness")} />}
      {l &&
        (trusted ? (
          <button className="sb-trusted" onClick={() => useStore.setState({ settings: true })} title="Commands here run without asking. Click to change.">
            <Icon name="shield" size={14} /> Trusted
          </button>
        ) : (
          <button onClick={() => useStore.setState({ settings: true })} title="Every command outside the allowlist asks first. Trusting the project lets the agent run them without asking.">
            <Icon name="shield" size={14} /> Trust this project…
          </button>
        ))}
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
      {l && (
        <button className={`sb-icon ${panel ? "on" : ""}`} onClick={togglePanel} aria-pressed={panel} aria-label={panel ? "Hide panel" : "Show panel"} title={`${panel ? "Hide" : "Show"} the panel — changes, git, preview (${label({ key: "i", alt: true })})`}>
          <Icon name="panel" size={15} />
        </button>
      )}
    </footer>
  );
}
