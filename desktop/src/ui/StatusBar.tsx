import { restart, useUpdate } from "../update";
import { Dialog } from "./kit";
import { useEffect, useState } from "react";
import { useStore } from "../store";
import { count } from "./Turn";
import { Icon } from "./icons";
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
function useNow(lane?: string): { label: string; since?: string; tone: "idle" | "live" | "warn" } {
  const l = useStore((s) => (lane ? s.lanes[lane] : undefined));
  if (!l) return { label: "", tone: "idle" };
  const name = l.agent;
  if (l.pending.length) return { label: `${name} · ${l.pending.length > 1 ? `paused — ${l.pending.length} questions waiting` : "waiting for you"}`, tone: "warn" };
  const t = l.conv.turns.at(-1);
  if (t?.gate?.status === "running") return { label: `${name} · Checking — ${t.gate.command ?? "the project's checks"}`, tone: "live" };
  if (!l.running) return { label: `${name} · idle`, tone: "idle" };
  const call = t && Object.values(t.calls).reverse().find((c) => c.state === "running" && !c.parent);
  if (call) {
    const verb = VERBS.find(([re]) => re.test(call.tool))?.[1] ?? call.tool;
    return { label: `${name} · ${verb} ${call.subject.slice(0, 60)}`, since: t?.startedAt, tone: "live" };
  }
  return { label: `${name} · ${t && Object.keys(t.blocks).length ? "Writing…" : "Thinking…"}`, since: t?.startedAt, tone: "live" };
}

export function StatusBar({ openTab }: { openTab: (tab: Tab) => void }) {
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
      {l && (
        <button onClick={() => openTab("git")} title="Open Git">
          <Icon name="branch" size={12} />
          {git ? (git.is_repo ? (git.branch ?? "detached") : "not a git repository") : l.wt ? `keel/${l.wt}` : "…"}
          {git && git.changes.length > 0 && <span>· {git.changes.length} changed</span>}
        </button>
      )}
      {l && now.label && (
        <button className={now.tone === "warn" ? "warn" : now.tone === "live" ? "live" : ""} title="What the agent is doing">
          {now.tone === "warn" && <Icon name="hand" size={12} />}
          {elsewhere ? `${l?.agent} · running in another terminal — Keel is following` : now.label}
          {!elsewhere && elapsed !== undefined && Number.isFinite(elapsed) && <span className="clock">· {clock(elapsed)}</span>}
        </button>
      )}
      <span className="spacer" />
      {tokens > 0 && (
        <button title={detail} className="mono" onClick={() => void navigator.clipboard.writeText(detail)}>
          {context ? `ctx ${count(context)} · ` : ""}
          {count(sum.input + sum.cache_write)} in · {count(sum.output)} out
          {cost > 0 ? ` · $${cost.toFixed(2)}` : ""}
        </button>
      )}
      {l && <SetupBadge project={l.project} showTab={() => openTab("readiness")} />}
      {l &&
        (trusted ? (
          <button className="warn" onClick={() => useStore.setState({ settings: true })} title="Commands here run without asking. Click to change.">
            <Icon name="shield" size={12} /> Trusted
          </button>
        ) : (
          <button onClick={() => useStore.setState({ settings: true })} title="Every command outside the allowlist asks first. Trusting the project lets the agent run them without asking.">
            <Icon name="shield" size={12} /> Trust this project…
          </button>
        ))}
      <UpdateButton />
      {project?.error ? (
        <button className="bad" onClick={() => l && useStore.getState().load(l.id)}>
          Keel stopped for this project — Restart
        </button>
      ) : project && !project.endpoint && project.starting ? (
        <button className="warn">Starting Keel…</button>
      ) : null}
    </footer>
  );
}

/// The one line an update gets: present when it is downloaded and ready, absent otherwise. A
/// restart ends every agent the app runs, so it says how many before it does it.
function UpdateButton() {
  const u = useUpdate();
  const running = useStore((s) => Object.values(s.lanes).filter((l) => l.running).length);
  const [asking, setAsking] = useState(false);
  if (u.state === "failed" && u.why.startsWith("The update could not install"))
    return (
      <button className="bad" onClick={() => useStore.setState({ settings: true })} title={u.why}>
        Update failed — see Settings
      </button>
    );
  if (u.state !== "ready") return null;
  return (
    <>
      <button className="update" onClick={() => (running ? setAsking(true) : void restart())} title={`Keel ${u.version} is downloaded`}>
        Restart to update
      </button>
      {asking && (
        <Dialog title={`Restart to install Keel ${u.version}?`} onClose={() => setAsking(false)}>
          <p>
            {running} lane{running === 1 ? " is" : "s are"} running a turn, and restarting stops {running === 1 ? "it" : "them"}. The conversations stay in History and resume after the restart.
          </p>
          <div className="actions">
            <button className="primary" onClick={() => void restart()}>
              Restart now
            </button>
            <button onClick={() => setAsking(false)}>Later</button>
          </div>
        </Dialog>
      )}
    </>
  );
}
