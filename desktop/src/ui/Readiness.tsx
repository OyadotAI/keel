import { useEffect, useMemo, useState } from "react";
import { get, post, type Endpoint } from "../api";
import { useStore } from "../store";

interface Finding {
  id: string;
  severity: "critical" | "high" | "medium" | "low" | "info";
  title: string;
  detail: string;
  path?: string | null;
  fix: { kind: "automatic" | "assisted" | "manual"; description: string };
}

type FindingScope = "session" | "folder" | "global";

interface Report {
  score: number;
  findings: Finding[];
  ignored: string[];
}

const ORDER = { critical: 0, high: 1, medium: 2, low: 3, info: 4 } as const;

function FindingCard({ finding, busy, onAdopt, onFix, onIgnore }: {
  finding: Finding;
  busy: boolean;
  onAdopt: () => void;
  onFix: (finding: Finding) => void;
  onIgnore: (finding: Finding) => void;
}) {
  return (
    <div className={`finding sev-${finding.severity}`}>
      <div className="finding-title">
        <span className="sev">{finding.severity}</span> {finding.title}
      </div>
      <div className="small muted">{finding.detail}</div>
      {finding.path && <div className="small"><code>{finding.path}</code></div>}
      <div className="small">{finding.fix.description}</div>
      <div className="card-actions">
        {finding.id === "agent/no-reviewers" ? (
          <button className="primary" onClick={onAdopt} title="Write the agent team and the lifecycle into this project, never over a file">
            Add the team
          </button>
        ) : finding.fix.kind !== "manual" ? (
          <button onClick={() => onFix(finding)} title={busy ? "The lane is mid-turn: this runs when it ends" : "Send this to the lane as a prompt"}>
            {busy ? "Queue for the agent" : "Ask the agent to fix it"}
          </button>
        ) : null}
        <button className="link small" onClick={() => onIgnore(finding)}>
          Set aside
        </button>
      </div>
    </div>
  );
}

/// How ready the project is for agent work and production, and what to do about each gap. Read
/// when the tab is open: the scan walks the repository, so it never runs behind a glance.
export function Readiness({ lane }: { lane: string }) {
  const project = useStore((s) => s.lanes[lane]?.project);
  const handedIds = useStore((s) => (project ? s.handed[project]?.ids : undefined));
  const handed = useMemo(() => new Set(handedIds ?? []), [handedIds]);
  // Mid-turn the buttons queue rather than grey out: `send` already holds a prompt until the turn
  // ends, and a disabled button on every card said nothing about why.
  const busy = useStore((s) => !!(s.lanes[lane]?.running));
  // The setup read (on `state.changed`, which a writing turn's end sends) replaces this object; a
  // fixed finding then leaves the list without anyone pressing "Check again".
  const stale = useStore((s) => (project ? s.readiness[project] : undefined));
  const [ep, setEp] = useState<Endpoint>();
  const [report, setReport] = useState<Report>();
  const [scopeFilter, setScopeFilter] = useState<FindingScope | "all">("all");
  const [error, setError] = useState<string>();
  const [note, setNote] = useState<string>();
  // An action that failed is a line above the list, not the list gone.
  const [failed, setFailed] = useState<string>();

  const read = (e: Endpoint) =>
    get<{ scan: Report }>(e, "/api/state").then(
      (s) => setReport(s.scan),
      (x) => setError(String(x)),
    );
  useEffect(() => {
    if (!project) return;
    useStore
      .getState()
      .ensure(project)
      .then((e) => {
        setEp(e);
        void read(e);
      }, (x) => setError(String(x)));
  }, [project]);
  useEffect(() => {
    if (ep && stale) void read(ep);
    // eslint-disable-next-line react-hooks/exhaustive-deps -- re-read when the store's copy is replaced
  }, [stale]);

  if (error)
    return (
      <div className="panel-empty">
        <p className="error">Could not read the project: {error}</p>
        {ep && (
          <button
            onClick={() => {
              setError(undefined);
              void read(ep);
            }}
          >
            Try again
          </button>
        )}
      </div>
    );
  if (!report || !ep) return <div className="panel-empty muted">Checking the project…</div>;
  const findings = [...report.findings].sort((a, b) => ORDER[a.severity] - ORDER[b.severity]);
  const scopeOf = (finding: Finding): FindingScope => {
    if (handed.has(finding.id)) return "session";
    if (finding.id.startsWith("license/") || finding.id.startsWith("claude/")) return "global";
    return "folder";
  };
  const groups: [FindingScope, string][] = [["session", "This session"], ["folder", "This folder"], ["global", "Global"]];
  const groupCounts = new Map(groups.map(([group]) => [group, findings.filter((finding) => scopeOf(finding) === group).length]));
  const visibleGroups = scopeFilter === "all" ? groups : groups.filter(([group]) => group === scopeFilter);

  async function ignore(f: Finding) {
    setFailed(undefined);
    await post(ep!, "/api/readiness/ignore", { id: f.id, ignored: true, why: "" }).catch((e) => setFailed(`Could not set that aside: ${e instanceof Error ? e.message : e}`));
    void read(ep!);
  }
  async function adopt() {
    setFailed(undefined);
    try {
      const r = await post<{ written: string[]; committed?: string | null }>(ep!, "/api/adopt", undefined, { commit: true });
      setNote(`Added ${r.written.length} files${r.committed ? ` in commit ${r.committed}` : ""}.`);
    } catch (e) {
      setFailed(`Could not add the team: ${e instanceof Error ? e.message : e}`);
    }
    void read(ep!);
  }
  function fix(f: Finding) {
    const where = f.path ? ` (${f.path})` : "";
    void useStore
      .getState()
      .send(lane, `Fix the readiness finding \`${f.id}\`${where}: ${f.title}.\n\n${f.detail}\n\nSuggested fix: ${f.fix.description}`);
  }

  return (
    <div className="readiness">
      <div className="panel-head">
        <strong>{report.score}</strong>
        <span className="muted small">
          / 100 · {findings.length} to look at{report.ignored.length ? ` · ${report.ignored.length} set aside` : ""}
        </span>
        <div className="segmented readiness-scope-controls" role="group" aria-label="Filter findings by scope">
          <button aria-pressed={scopeFilter === "all"} onClick={() => setScopeFilter("all")}>
            All {findings.length}
          </button>
          {groups.map(([group, label]) => (
            <button key={group} aria-pressed={scopeFilter === group} onClick={() => setScopeFilter(group)}>
              {label} {groupCounts.get(group)}
            </button>
          ))}
        </div>
        <button className="link small" onClick={() => read(ep)}>
          Check again
        </button>
      </div>
      {note && <div className="small panel-note">{note}</div>}
      {failed && <div className="small panel-note error">{failed}</div>}
      <div className="findings">
        {visibleGroups.map(([group, label]) => {
          const items = findings.filter((f) => scopeOf(f) === group);
          return (
            <section key={group} className="readiness-group" aria-label={label}>
              <div className="eyebrow setup-group-head">{label}</div>
              {items.length ? (
                items.map((f) => (
                  <FindingCard key={f.id + (f.path ?? "")} finding={f} busy={busy} onAdopt={adopt} onFix={fix} onIgnore={ignore} />
                ))
              ) : (
                <div className="small muted readiness-empty">
                  {findings.length === 0 ? "Nothing to fix in this scope." : "No findings for this scope."}
                </div>
              )}
            </section>
          );
        })}
      </div>
    </div>
  );
}
