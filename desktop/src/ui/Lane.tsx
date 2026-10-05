import { lazy, Suspense, useMemo, useState, type ReactNode } from "react";
import { Virtuoso } from "react-virtuoso";
import { post } from "../api";
import { useStore } from "../store";
import { QuietLine, stateOf, TurnView } from "./Turn";
import { Git } from "./Git";
import { Review } from "./Review";
import { Jobs } from "./Jobs";
import { Preview } from "./Preview";
import { Readiness } from "./Readiness";
import { Icon } from "./icons";
import { label } from "../keys";
import { Tabs } from "./kit";

const Terminal = lazy(() => import("./Terminal").then((m) => ({ default: m.Terminal })));

export type Tab = "turns" | "git" | "review" | "jobs" | "preview" | "readiness";
export const TABS: Tab[] = ["turns", "git", "review", "jobs", "preview", "readiness"];

/// Every lane opened this session, its agent running in its own terminal. Only the active one is
/// shown; the rest stay mounted (hidden, not unmounted), because a terminal that closes ends the
/// agent in it and switching lanes must never stop a turn.
export function Agents() {
  const opened = useStore((s) => s.opened);
  const active = useStore((s) => s.active);
  const showing = useStore((s) => !s.settings && !s.creating && !s.extensions);
  return (
    <Suspense fallback={<div className="terminal-state muted">Loading the terminal…</div>}>
      {opened.map((l) => (
        <Terminal key={l} lane={l} agent hidden={!showing || l !== active} />
      ))}
    </Suspense>
  );
}

/// A shell in the lane's checkout, under the agent: for the command you want to run yourself.
export function Shell({ lane, close }: { lane: string; close: () => void }) {
  return (
    <div className="shell">
      <div className="shell-bar">
        <Icon name="terminal" size={13} />
        <span>Shell</span>
        <span className="spacer" />
        <button className="ghost" onClick={close} aria-label="Close the shell" title={`Close the shell (${label({ key: "j" })})`}>
          <Icon name="x" size={14} />
        </button>
      </div>
      <Suspense fallback={null}>
        <Terminal lane={lane} onExit={close} />
      </Suspense>
    </div>
  );
}

/// What Keel shows beside the agent: its turns, read from the transcript, and the machine's side.
export function SidePanel({ lane, tab, setTab, expanded, toggleExpanded, close, ask }: { lane: string; tab: Tab; setTab: (t: Tab) => void; expanded: boolean; toggleExpanded: () => void; close: () => void; ask: (what: "merge" | "discard") => void }) {
  const running = useStore((s) => s.lanes[lane]?.jobs.filter((j) => !j.finished).length ?? 0);
  const changed = useStore((s) => s.lanes[lane]?.git?.changes.length ?? 0);
  const ready = useStore((s) => {
    const p = s.lanes[lane]?.project;
    return p ? s.readiness[p] : undefined;
  });
  const names: Record<Tab, string> = { turns: "Turns", git: "Git", review: "Review", jobs: "Jobs", preview: "Preview", readiness: "Readiness" };
  // Counts as badges, so a tab says there is something there without being opened.
  const badge: Partial<Record<Tab, ReactNode>> = {
    git: changed ? <span className="tab-count">{changed}</span> : null,
    jobs: running ? <span className="tab-badge live">{running}</span> : null,
    readiness: ready?.open ? <span className={`tab-badge${ready.critical ? " critical" : ""}`}>{ready.open}</span> : null,
  };

  return (
    <aside className="side-panel" aria-label="Side panel">
      <Tabs tabs={TABS.map((id, i) => ({ id, label: (
            <>
              {names[id]} {badge[id]}
            </>
          ),
          title: `${names[id]} (⌘⌥${i + 1})`,
        }))} value={tab} onChange={setTab}>
        <span className="spacer" />
        <button className="ghost" onClick={toggleExpanded} aria-label={expanded ? "Put the panel back beside the terminal" : "Expand the panel"} title={expanded ? "Restore (⌘⌥⇧I)" : "Expand (⌘⌥⇧I)"}>
          <Icon name="maximize" size={14} />
        </button>
        <button className="ghost" onClick={close} aria-label="Close the panel" title="Close the panel (⌘⌥I)">
          <Icon name="x" size={14} />
        </button>
      </Tabs>
      <div className="tab-body">
        {tab === "turns" ? <Turns lane={lane} /> : tab === "git" ? <Git lane={lane} /> : tab === "review" ? <Review lane={lane} ask={ask} /> : tab === "jobs" ? <Jobs lane={lane} /> : tab === "preview" ? <Preview lane={lane} /> : <Readiness lane={lane} />}
      </div>
    </aside>
  );
}

/// The conversation as turns, newest first — what each one asked, wrote, ran, and how its checks
/// went.
function Turns({ lane }: { lane: string }) {
  const count = useStore((s) => s.lanes[lane]?.conv.turns.length ?? 0);
  const agent = useStore((s) => s.lanes[lane]?.agent);
  const loading = useStore((s) => !!s.lanes[lane]?.known && !s.lanes[lane]?.loaded);
  const rewound = useStore((s) => s.lanes[lane]?.rewound);
  if (agent === "codex")
    return <div className="panel-empty">Codex lanes have no turn timeline yet — Keel can't read Codex transcripts. Changes, Jobs and Preview still work.</div>;
  if (count === 0)
    return (
      <div className="panel-empty">
        <h3>{loading ? "Reading the conversation…" : "Nothing has run yet"}</h3>
        {!loading && "Type in the terminal. Each turn shows up here with the files it changed, the commands it ran and the project's checks."}
      </div>
    );
  return <TurnList lane={lane} count={count} rewound={rewound} />;
}

type Item = { kind: "turn"; index: number } | { kind: "quiet"; indices: number[] };

/// Newest first. A run of turns that answered and changed nothing is one quiet group rather than
/// a row each — they are the conversation, which the terminal already shows. Only the newest
/// handful are drawn until asked: a 300-turn session does not build 300 cards to show the last.
function TurnList({ lane, count, rewound }: { lane: string; count: number; rewound?: { number: number; undo: string } }) {
  const [all, setAll] = useState(false);
  // A string, so the selector's answer is equal when nothing that shapes the list changed.
  const shape = useStore((s) => (s.lanes[lane]?.conv.turns ?? []).map((t) => (stateOf(t, false) === "quiet" ? "q" : "t")).join(""));
  const items = useMemo(() => {
    const out: Item[] = [];
    for (let i = shape.length - 1; i >= 0; i--) {
      if (shape[i] === "q") {
        const last = out[out.length - 1];
        if (last?.kind === "quiet") last.indices.push(i);
        else out.push({ kind: "quiet", indices: [i] });
      } else out.push({ kind: "turn", index: i });
    }
    return out;
  }, [shape]);
  const shown = all ? items.length : Math.min(items.length, 6);
  return (
    <div style={{ display: "flex", flexDirection: "column", height: "100%" }}>
      {rewound && <Rewound lane={lane} number={rewound.number} undo={rewound.undo} />}
      <Virtuoso
        className="turns"
        totalCount={shown + (shown < items.length ? 1 : 0)}
        itemContent={(i) =>
          i === shown ? (
            <button className="more-turns" onClick={() => setAll(true)}>
              Show {count - items.slice(0, shown).reduce((n, it) => n + (it.kind === "quiet" ? it.indices.length : 1), 0)} earlier turns
            </button>
          ) : items[i].kind === "turn" ? (
            <TurnView lane={lane} index={(items[i] as { index: number }).index} />
          ) : (
            <QuietGroup lane={lane} indices={(items[i] as { indices: number[] }).indices} />
          )
        }
        computeItemKey={(i) => (i === shown ? "earlier" : items[i].kind === "turn" ? `t${(items[i] as { index: number }).index}` : `q${(items[i] as { indices: number[] }).indices.at(-1)}`)}
      />
    </div>
  );
}

function QuietGroup({ lane, indices }: { lane: string; indices: number[] }) {
  const [open, setOpen] = useState(false);
  if (indices.length === 1)
    return (
      <div className="quiet-group">
        <QuietLine lane={lane} index={indices[0]} />
      </div>
    );
  return (
    <div className="quiet-group">
      <button className="quiet-head" aria-expanded={open} onClick={() => setOpen(!open)}>
        <Icon name={open ? "chevron-down" : "chevron-right"} size={12} />
        {indices.length} replies — no changes
      </button>
      {open && indices.map((i) => <QuietLine key={i} lane={lane} index={i} />)}
    </div>
  );
}

function Rewound({ lane, number, undo }: { lane: string; number: number; undo: string }) {
  const dismiss = () => useStore.setState((s) => ({ lanes: { ...s.lanes, [lane]: { ...s.lanes[lane], rewound: undefined } } }));
  async function undoIt() {
    const l = useStore.getState().lanes[lane];
    const ep = l && useStore.getState().projects[l.project]?.endpoint;
    if (!ep) return;
    try {
      await post(ep, "/api/git/restore", { tree: undo }, { wt: l.wt });
      dismiss();
    } catch (e) {
      // Kept: this bar is the only place the undo is held.
      useStore.setState((s) => ({ lanes: { ...s.lanes, [lane]: { ...s.lanes[lane], error: `Could not undo the rewind: ${e instanceof Error ? e.message : e}. Try again when nothing is writing the tree.` } } }));
    }
  }
  return (
    <div className="panel-note small" style={{ background: "color-mix(in srgb, var(--accent) 8%, transparent)", display: "flex", gap: 8, alignItems: "center" }}>
      <span style={{ flex: 1 }}>Files were rewound to before turn {number}.</span>
      <button className="link small" onClick={undoIt}>
        Undo
      </button>
      <button className="ghost" onClick={dismiss} aria-label="Dismiss">
        <Icon name="x" size={12} />
      </button>
    </div>
  );
}
