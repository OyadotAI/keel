import { useEffect, useState } from "react";
import { get, post, type Endpoint } from "../api";
import { useStore } from "../store";

interface Finding {
  id: string;
  severity: "critical" | "high" | "medium" | "low";
  title: string;
  detail: string;
  path?: string | null;
  fix: { kind: "automatic" | "assisted" | "manual"; description: string };
}

interface Report {
  score: number;
  findings: Finding[];
  ignored: string[];
}

const ORDER = { critical: 0, high: 1, medium: 2, low: 3 } as const;

/// How ready the project is for agent work and production, and what to do about each gap. Read
/// when the tab is open: the scan walks the repository, so it never runs behind a glance.
export function Readiness({ lane }: { lane: string }) {
  const project = useStore((s) => s.lanes[lane]?.project);
  const busy = useStore((s) => !!(s.lanes[lane]?.running));
  const [ep, setEp] = useState<Endpoint>();
  const [report, setReport] = useState<Report>();
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
          readiness · {findings.length} to look at{report.ignored.length ? ` · ${report.ignored.length} set aside` : ""}
        </span>
        <button className="link small" onClick={() => read(ep)}>
          Check again
        </button>
      </div>
      {note && <div className="small panel-note">{note}</div>}
      {failed && <div className="small panel-note error">{failed}</div>}
      {findings.length === 0 ? (
        <div className="panel-empty muted">Nothing to fix. The project is ready for agent work.</div>
      ) : (
        <div className="findings">
          {findings.map((f) => (
            <div key={f.id + (f.path ?? "")} className={`finding sev-${f.severity}`}>
              <div className="finding-title">
                <span className="sev">{f.severity}</span> {f.title}
              </div>
              <div className="small muted">{f.detail}</div>
              {f.path && <div className="small"><code>{f.path}</code></div>}
              <div className="small">{f.fix.description}</div>
              <div className="card-actions">
                {f.id === "agent/no-reviewers" ? (
                  <button className="primary" disabled={busy} onClick={adopt} title="Write the agent team and the lifecycle into this project, never over a file">
                    Add the team
                  </button>
                ) : f.fix.kind !== "manual" ? (
                  <button disabled={busy} onClick={() => fix(f)} title="Send this to the lane as a prompt">
                    Ask the agent to fix it
                  </button>
                ) : null}
                <button className="link small" onClick={() => ignore(f)}>
                  Set aside
                </button>
              </div>
            </div>
          ))}
        </div>
      )}
    </div>
  );
}
